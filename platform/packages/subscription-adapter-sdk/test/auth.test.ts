import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { detectAuthState, threadIdFromUrl } from "../src/index";
import { fixturePage, launchBrowser, makeResolver } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("detectAuthState (§A3.1)", () => {
  it("ready on the idle fixture (logged-in probe present)", async () => {
    const page = await fixturePage(browser, "idle.html");
    expect(await detectAuthState(page, makeResolver(page))).toBe("ready");
    await page.close();
  });

  it("auth_required on the logged-out fixture (probe absent)", async () => {
    const page = await fixturePage(browser, "logged-out.html");
    expect(await detectAuthState(page, makeResolver(page))).toBe("auth_required");
    await page.close();
  });

  it("auth_required on the challenge interstitial (probe absent)", async () => {
    const page = await fixturePage(browser, "challenge.html");
    expect(await detectAuthState(page, makeResolver(page))).toBe("auth_required");
    await page.close();
  });

  it("loggedOutUrlPattern short-circuits the probe", async () => {
    const page = await fixturePage(browser, "idle.html");
    expect(
      await detectAuthState(page, makeResolver(page), {
        loggedOutUrlPattern: /^about:blank$/,
      })
    ).toBe("auth_required");
    await page.close();
  });
});

describe("threadIdFromUrl (§A3.1)", () => {
  it("extracts the first capture group", () => {
    const pattern = /^https:\/\/fixture-web\.test\/c\/([\w-]+)/;
    expect(threadIdFromUrl("https://fixture-web.test/c/abc-123-def", pattern)).toBe("abc-123-def");
  });

  it("returns null when the pattern does not match", () => {
    const pattern = /^https:\/\/fixture-web\.test\/c\/([\w-]+)/;
    expect(threadIdFromUrl("https://fixture-web.test/settings", pattern)).toBeNull();
  });
});
