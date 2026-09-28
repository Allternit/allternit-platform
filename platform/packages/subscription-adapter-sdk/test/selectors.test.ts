import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { SelectorNotFoundError, SelectorPack, createResolver } from "../src/index";
import { fixturePage, launchBrowser, loadPack, makeResolver } from "./helpers";
import type { DriftSignal } from "../src/index";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("selector registry (§A3.2)", () => {
  it("resolves a key at strategy 0 when the first strategy matches", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    const composer = await resolver.resolveLocator("composer");
    await expect(composer.first().getAttribute("data-testid")).resolves.toBe("fw-composer");
    expect(resolver.lastMatchedStrategy("composer")).toBe("0:role");
    await page.close();
  });

  it("honors file order: earlier matching strategy wins over a later one", async () => {
    const page = await fixturePage(browser, "idle.html");
    const pack = SelectorPack.fromYaml(
      [
        "dup:",
        "  critical: false",
        "  strategies:",
        "    - { testid: fw-user-menu }",
        "    - { css: '.fw-user-menu' }",
      ].join("\n")
    );
    const resolver = createResolver(page, pack);
    await resolver.resolveLocator("dup");
    expect(resolver.lastMatchedStrategy("dup")).toBe("0:testid");
    await page.close();
  });

  it("falls back to a later strategy and fires onDrift at index > 0", async () => {
    const page = await fixturePage(browser, "idle.html");
    const drift: DriftSignal[] = [];
    const resolver = makeResolver(page, (d) => drift.push(d));
    const send = await resolver.resolveLocator("send_button");
    await expect(send.first().innerText()).resolves.toBe("Send");
    expect(resolver.lastMatchedStrategy("send_button")).toBe("1:role");
    expect(drift).toEqual([{ key: "send_button", strategy_index: 1, total: 3 }]);
    await page.close();
  });

  it("records null telemetry for keys never resolved", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    expect(resolver.lastMatchedStrategy("stop_button")).toBeNull();
    await page.close();
  });

  it("throws a clear error naming the key when no strategy matches", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    await expect(resolver.resolveLocator("stop_button")).rejects.toThrow(SelectorNotFoundError);
    await expect(resolver.resolveLocator("stop_button")).rejects.toThrow(/stop_button/);
    await page.close();
  });

  it("throws for keys absent from the pack; tryResolveLocator returns null instead", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    await expect(resolver.resolveLocator("not_in_pack")).rejects.toThrow(/not_in_pack/);
    await expect(resolver.tryResolveLocator("not_in_pack")).resolves.toBeNull();
    await page.close();
  });

  it("contracts resolve() delegates to resolveLocator", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    const resolved = await resolver.resolve("logged_in_probe");
    expect(resolved).not.toBeNull();
    await page.close();
  });

  it("loads the fixture pack with the progress: key group", () => {
    const pack = loadPack();
    expect(pack.get("composer")?.critical).toBe(true);
    expect(pack.has("progress:steps")).toBe(true);
    expect(pack.has("progress:counter")).toBe(true);
    expect(pack.has("progress:partial_artifacts")).toBe(true);
  });
});
