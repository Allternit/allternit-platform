import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { attachFiles, fillComposer, submit, waitForSendReady } from "../src/index";
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

describe("attachFiles + waitForSendReady", () => {
  it("sets files on the image-only hidden input and waits out the upload", async () => {
    const page = await fixturePage(browser, "attach.html");
    const resolver = makeResolver(page);
    await attachFiles(
      resolver,
      [{ name: "photo.jpg", mimeType: "image/jpeg", buffer: Buffer.from("jpeg-bytes") }],
      { key: "file_input_image" }
    );
    expect(await page.locator("#fw-send").isEnabled()).toBe(false);
    expect(await waitForSendReady(resolver, { timeoutMs: 5000, pollIntervalMs: 50 })).toBe(true);
    const chip = page.locator("#fw-attachments span");
    expect(await chip.getAttribute("data-file-name")).toBe("photo.jpg");
    expect(await chip.getAttribute("data-file-size")).toBe("10");
    expect(await chip.getAttribute("data-from-input")).toBe("fw-file-image");
    await page.close();
  });

  it("throws when the file input is gone and times out on a stuck send button", async () => {
    const page = await fixturePage(browser, "idle.html");
    const resolver = makeResolver(page);
    await expect(
      attachFiles(resolver, [{ name: "p.png", mimeType: "image/png", buffer: Buffer.from("x") }], {
        key: "file_input_image",
      })
    ).rejects.toThrow(/file_input_image/);
    await page.evaluate(() => ((document.getElementById("fw-send") as HTMLButtonElement).disabled = true));
    expect(await waitForSendReady(resolver, { timeoutMs: 200, pollIntervalMs: 50 })).toBe(false);
    await page.close();
  });
});
