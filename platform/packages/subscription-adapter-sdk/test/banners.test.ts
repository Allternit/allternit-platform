import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { createBannerClassifier, type BannerPattern } from "../src/index";
import { fixturePage, launchBrowser } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

const pack: BannerPattern[] = [
  { kind: "limit_banner", pattern: /approaching your (usage )?limit/i },
  { kind: "limit_banner", pattern: /limit reached/i },
  { kind: "hard_error", pattern: /something went wrong/i },
  { kind: "slow_mode", pattern: /slower responses? (for|during)/i },
  { kind: "reset_notice", pattern: /resets? at/i },
];

describe("banner classification (§A3.1 detectBanners)", () => {
  it("classifies both fixture banner variants as limit_banner", async () => {
    const page = await fixturePage(browser, "limit-banner.html");
    const classifier = createBannerClassifier(pack);
    const soft = await page.getByTestId("fw-banner-soft").innerText();
    const hard = await page.getByTestId("fw-banner-hard").innerText();
    expect(classifier.classify(soft)).toEqual({ kind: "limit_banner", raw_excerpt: soft });
    expect(classifier.classify(hard)).toEqual({ kind: "limit_banner", raw_excerpt: hard });
    await page.close();
  });

  it("returns null for non-matching text", () => {
    const classifier = createBannerClassifier(pack);
    expect(classifier.classify("Everything is fine.")).toBeNull();
  });

  it("first matching pattern wins", () => {
    const classifier = createBannerClassifier(pack);
    const hit = classifier.classify("Limit reached. Your usage limit resets at midnight.");
    expect(hit?.kind).toBe("limit_banner"); // limit_banner pattern precedes reset_notice
  });

  it("raw_excerpt is pre-truncated to 500 chars", () => {
    const classifier = createBannerClassifier(pack);
    const long = `Limit reached. ${"x".repeat(1000)}`;
    const hit = classifier.classify(long);
    expect(hit?.raw_excerpt).toHaveLength(500);
  });
});
