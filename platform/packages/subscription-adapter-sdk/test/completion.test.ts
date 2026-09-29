import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import {
  CompletionTimeout,
  SelectorPack,
  awaitCompletion,
  createCompletionTracker,
  createResolver,
  pageShape,
} from "../src/index";
import { fixturePage, launchBrowser, makeResolver } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("awaitCompletion (§A1 multi-signal detector)", () => {
  it("completes on the complete fixture", async () => {
    const page = await fixturePage(browser, "complete.html");
    const resolver = makeResolver(page);
    const result = await awaitCompletion(page, resolver, {
      stabilityMs: 300,
      pollIntervalMs: 25,
      timeoutMs: 5000,
    });
    expect(result.completed).toBe(true);
    expect(new Date(result.last_change_at).getTime()).toBeGreaterThan(0);
    await page.close();
  });

  it("does not complete while streaming: stop visible, send disabled, streaming node", async () => {
    const page = await fixturePage(browser, "streaming.html");
    const resolver = makeResolver(page);
    await expect(
      awaitCompletion(page, resolver, {
        stabilityMs: 150,
        pollIntervalMs: 25,
        timeoutMs: 400,
      })
    ).rejects.toThrow(CompletionTimeout);
    await page.close();
  });

  it("reports each signal on streaming vs complete fixtures", async () => {
    const streaming = await fixturePage(browser, "streaming.html");
    const t1 = createCompletionTracker(streaming, makeResolver(streaming), { stabilityMs: 100 });
    const s1 = (await t1.pollOnce()).signals;
    expect(s1).toEqual({
      stop_absent: false,
      send_enabled: false,
      stable: false,
      streaming_absent: false,
    });
    await streaming.close();

    const complete = await fixturePage(browser, "complete.html");
    const t2 = createCompletionTracker(complete, makeResolver(complete), { stabilityMs: 100 });
    await t2.pollOnce();
    await new Promise((r) => setTimeout(r, 150));
    const s2 = (await t2.pollOnce()).signals;
    expect(s2).toEqual({
      stop_absent: true,
      send_enabled: true,
      stable: true,
      streaming_absent: true,
    });
    await complete.close();
  });

  it("treats an absent streaming key in the pack as no streaming node", async () => {
    const page = await fixturePage(browser, "complete.html");
    const packText = [
      "response:",
      "  critical: true",
      "  strategies:",
      "    - { testid: fw-response }",
      "send_button:",
      "  critical: true",
      "  strategies:",
      "    - { testid: fw-send }",
      "stop_button:",
      "  critical: false",
      "  strategies:",
      "    - { testid: fw-stop }",
    ].join("\n");
    const resolver = createResolver(page, SelectorPack.fromYaml(packText));
    const result = await awaitCompletion(page, resolver, {
      stabilityMs: 100,
      pollIntervalMs: 25,
      timeoutMs: 3000,
    });
    expect(result.completed).toBe(true);
    await page.close();
  });
});

describe("ignoreSend (ChatGPT live UI: Send enabled, disabled, or absent depending on chat mode)", () => {
  const pack = SelectorPack.fromYaml(
    [
      "response:",
      "  critical: true",
      "  strategies:",
      "    - { css: '.reply' }",
      "send_button:",
      "  critical: true",
      "  strategies:",
      "    - { css: 'button.send' }",
    ].join("\n")
  );

  it("a disabled Send blocks completion by default; ignoreSend completes on the reply alone", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply">Hello there, friend</div><button class="send" disabled>Send</button>`);
    const strict = createCompletionTracker(page, createResolver(page, pack), { stabilityMs: 0 });
    expect((await strict.pollOnce()).complete).toBe(false);
    const lenient = createCompletionTracker(page, createResolver(page, pack), {
      stabilityMs: 0,
      ignoreSend: true,
    });
    expect((await lenient.pollOnce()).complete).toBe(true);
    await page.close();
  });

  it("ignoreSend completes with no Send at all (regular chat shows the voice button)", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply">Mango</div><button aria-label="Start Voice">v</button>`);
    const t = createCompletionTracker(page, createResolver(page, pack), { stabilityMs: 0, ignoreSend: true });
    expect((await t.pollOnce()).complete).toBe(true);
    await page.close();
  });

  it("ignoreSend still refuses to complete before any reply text exists", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply"></div>`);
    const t = createCompletionTracker(page, createResolver(page, pack), { stabilityMs: 0, ignoreSend: true });
    const r = await t.pollOnce();
    expect(r.signals.send_enabled).toBe(false);
    expect(r.complete).toBe(false);
    await page.close();
  });

  it("tracks the NEWEST reply: a change in the last turn resets stability", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply">old answer</div><div class="reply" id="new">a</div><button class="send">Send</button>`);
    let clock = 0;
    const t = createCompletionTracker(page, createResolver(page, pack), {
      stabilityMs: 100,
      now: () => clock,
    });
    await t.pollOnce();
    clock = 50;
    await page.evaluate(() => (document.getElementById("new")!.textContent = "ab"));
    await t.pollOnce();
    expect(t.lastChangeAt()).toBe(50);
    await page.close();
  });
});

describe("stall watchdog input (D11)", () => {
  it("stalled() trips after stallTimeoutS with no DOM change on a static fixture", async () => {
    const page = await fixturePage(browser, "static.html");
    const resolver = makeResolver(page);
    let t = 1_700_000_000_000;
    const tracker = createCompletionTracker(page, resolver, { now: () => t });
    await tracker.pollOnce();
    expect(tracker.lastChangeAt()).toBe(t);
    expect(tracker.stalled(90)).toBe(false);
    t += 91_000;
    expect(tracker.stalled(90)).toBe(true);
    expect(tracker.stalled(200)).toBe(false);
    await page.close();
  });

  it("pageShape: stall evidence names the markup, never the text", async () => {
    const page = await browser.newPage();
    await page.setContent(
      '<main><div data-message-author-role="user" data-message-id="m1">secret prompt</div>' +
        '<article data-turn="assistant" role="article"><p>secret reply</p></article></main>'
    );
    const shape = await pageShape(page);
    expect(shape).toContain("data-message-author-role=user");
    expect(shape).toContain("data-turn=assistant");
    expect(shape).not.toContain("secret");

    // Renamed markers: still described, still no text.
    await page.setContent(
      '<main><div data-user-message-bubble="true">secret prompt</div>' +
        '<section data-reply-stream="assistant"><span>secret reply</span></section></main>'
    );
    const renamed = await pageShape(page);
    expect(renamed).toContain("data-user-message-bubble=true");
    expect(renamed).toContain("data-reply-stream=assistant");
    expect(renamed).not.toContain("secret");
    await page.close();
  });

  it("a fresh DOM change resets the stall window", async () => {
    const page = await fixturePage(browser, "static.html");
    const resolver = makeResolver(page);
    let t = 1_700_000_000_000;
    const tracker = createCompletionTracker(page, resolver, { now: () => t });
    await tracker.pollOnce();
    t += 60_000;
    await page.evaluate(() => {
      const el = document.querySelector("[data-testid='fw-response'] p");
      if (el) el.textContent = "Static response body, updated.";
    });
    await tracker.pollOnce();
    expect(tracker.lastChangeAt()).toBe(t);
    expect(tracker.stalled(90)).toBe(false);
    await page.close();
  });
});
