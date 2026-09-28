import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import {
  createHeartbeat,
  extractCounterBadge,
  extractPartialArtifacts,
  extractStepList,
  watchStreamingGrowth,
  type ProgressHeartbeat,
} from "../src/index";
import { fixturePage, launchBrowser, makeResolver } from "./helpers";
import type { ProviderId } from "@allternit/subscription-fabric-contracts";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("progress extractors (D11)", () => {
  it("extractStepList emits one progress event per step with fraction from done/total", async () => {
    const page = await fixturePage(browser, "research-mid-run.html");
    const resolver = makeResolver(page);
    const emitted: unknown[] = [];
    const events = await extractStepList(page, resolver, (e) => emitted.push(e));
    expect(events).toHaveLength(5);
    expect(events[0]).toEqual({ t: "progress", label: "Plan research questions", fraction: 0.6 });
    expect(events[3].label).toBe("Cross-check claims");
    expect(events.every((e) => e.fraction === 0.6)).toBe(true);
    expect(emitted).toEqual(events);
    await page.close();
  });

  it("extractCounterBadge reads '12 sources' as a label without a fraction", async () => {
    const page = await fixturePage(browser, "research-mid-run.html");
    const resolver = makeResolver(page);
    const event = await extractCounterBadge(page, resolver);
    expect(event).toEqual({ t: "progress", label: "12 sources" });
    await page.close();
  });

  it("extractCounterBadge parses 'N/M' into a fraction", async () => {
    const page = await fixturePage(browser, "slides-mid-run.html");
    const resolver = makeResolver(page);
    const event = await extractCounterBadge(page, resolver);
    expect(event).toEqual({ t: "progress", label: "2/5 slides", fraction: 0.4 });
    await page.close();
  });

  it("extractPartialArtifacts emits artifact.partial refs for present thumbnails", async () => {
    const page = await fixturePage(browser, "slides-mid-run.html");
    const resolver = makeResolver(page);
    const emitted: unknown[] = [];
    const events = await extractPartialArtifacts(page, resolver, {
      provider: "fixture-web" as ProviderId,
      emit: (e) => emitted.push(e),
    });
    expect(events).toHaveLength(2);
    expect(events[0]).toEqual({
      t: "artifact.partial",
      ref: {
        provider: "fixture-web",
        provider_artifact_id: "fw-deck-1-slide-1",
        provider_url: "https://cdn.fixture-web.test/decks/fw-deck-1/slide-1.png",
        provider_url_expires_at: null,
      },
    });
    expect(events[1].ref.provider_artifact_id).toBe("fw-deck-1-slide-2");
    expect(emitted).toEqual(events);
    await page.close();
  });

  it("watchStreamingGrowth emits on response-text length deltas only", async () => {
    const page = await fixturePage(browser, "streaming.html");
    const resolver = makeResolver(page);
    const emitted: unknown[] = [];
    const watcher = watchStreamingGrowth(page, resolver, (e) => emitted.push(e));

    expect(await watcher.sample()).toBeNull(); // baseline
    expect(await watcher.sample()).toBeNull(); // no change

    await page.evaluate(() => {
      const el = document.querySelector("[data-testid='fw-response'] p");
      if (el) el.textContent = "The answer so far is taking shape";
    });
    const event = await watcher.sample();
    expect(event).not.toBeNull();
    expect(event?.label).toMatch(/^streaming \(\+\d+ chars\)$/);
    expect(emitted).toEqual([event]);
    await page.close();
  });

  it("heartbeat fires with growing elapsed_s and carries last_change_at", () => {
    const emitted: ProgressHeartbeat[] = [];
    let t = 1_700_000_000_000;
    const hb = createHeartbeat((e) => emitted.push(e), 15000, {
      now: () => t,
      lastChangeAt: () => t - 4000,
    });
    hb.tick();
    t += 15_000;
    hb.tick();
    t += 15_000;
    hb.tick();
    expect(emitted.map((e) => e.elapsed_s)).toEqual([0, 15, 30]);
    expect(emitted[2].t).toBe("progress.heartbeat");
    expect(emitted[2].last_change_at).toBe(new Date(1_700_000_000_000 + 30_000 - 4000).toISOString());
  });

  it("heartbeat start/stop drives real interval ticks with a small intervalMs", async () => {
    const emitted: ProgressHeartbeat[] = [];
    const hb = createHeartbeat((e) => emitted.push(e), 30);
    hb.start();
    await new Promise((r) => setTimeout(r, 100));
    hb.stop();
    expect(emitted.length).toBeGreaterThanOrEqual(2);
    expect(emitted[emitted.length - 1].elapsed_s).toBeGreaterThan(emitted[0].elapsed_s);
  });
});
