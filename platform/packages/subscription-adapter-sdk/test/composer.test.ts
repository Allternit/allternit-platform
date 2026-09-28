import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { fillComposer, submit } from "../src/index";
import { fixturePage, launchBrowser, makeResolver } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("fillComposer (§A3.1)", () => {
  it("fills the contenteditable composer", async () => {
    const page = await fixturePage(browser, "idle.html");
    await fillComposer(page, makeResolver(page), "a short fixture prompt");
    const text = await page.getByTestId("fw-composer").innerText();
    expect(text).toBe("a short fixture prompt");
    await page.close();
  });

  it("fills the textarea composer variant", async () => {
    const page = await fixturePage(browser, "idle.html");
    await fillComposer(page, makeResolver(page), "textarea prompt text", {
      key: "composer_textarea",
    });
    const value = await page.getByTestId("fw-composer-textarea").inputValue();
    expect(value).toBe("textarea prompt text");
    await page.close();
  });

  it("fills >500 chars into contenteditable via insertText (no per-key typing)", async () => {
    const page = await fixturePage(browser, "idle.html");
    const longText = "fixture-prompt-".repeat(40); // 600 chars
    await fillComposer(page, makeResolver(page), longText);
    const text = await page.getByTestId("fw-composer").innerText();
    expect(text).toBe(longText);
    await page.close();
  });

  it("fills >500 chars into a textarea", async () => {
    const page = await fixturePage(browser, "idle.html");
    const longText = "fixture-prompt-".repeat(40);
    await fillComposer(page, makeResolver(page), longText, { key: "composer_textarea" });
    const value = await page.getByTestId("fw-composer-textarea").inputValue();
    expect(value).toBe(longText);
    await page.close();
  });
});

describe("submit (§A3.1)", () => {
  it("clicks the send button when enabled", async () => {
    const page = await fixturePage(browser, "idle.html");
    await submit(page, makeResolver(page));
    const flag = await page.evaluate(() => document.body.dataset.fwSubmitted);
    expect(flag).toBe("click");
    await page.close();
  });

  it("falls back to Enter when the send button is disabled and the pack allows it", async () => {
    const page = await fixturePage(browser, "streaming.html");
    await submit(page, makeResolver(page), { fallback: "enter" });
    const flag = await page.evaluate(() => document.body.dataset.fwSubmitted);
    expect(flag).toBe("enter");
    await page.close();
  });

  it("throws when the send button is unavailable and no fallback is configured", async () => {
    const page = await fixturePage(browser, "streaming.html");
    await expect(submit(page, makeResolver(page))).rejects.toThrow(/fallback/);
    await page.close();
  });
});
