import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { SelectorPack, createResolver, extractLastAssistantTurn } from "../src/index";
import { fixturePage, launchBrowser, makeResolver } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("extractLastAssistantTurn (§A3.1)", () => {
  it("converts the complete fixture to markdown with fenced code and citation link", async () => {
    const page = await fixturePage(browser, "complete.html");
    const md = await extractLastAssistantTurn(page, makeResolver(page));
    expect(md).toContain("### Summary");
    expect(md).toContain("Here is the full result for your request.");
    expect(md).toContain("```ts\nconst answer: number = 42;\n```");
    expect(md).toContain("- first supporting point");
    expect(md).toContain("- second supporting point");
    expect(md).toContain("[[1] example reference](https://example.com/ref-1)");
    await page.close();
  });

  it("extracts plain paragraphs from the streaming fixture", async () => {
    const page = await fixturePage(browser, "streaming.html");
    const md = await extractLastAssistantTurn(page, makeResolver(page));
    expect(md).toBe("The answer so far is");
    await page.close();
  });

  it("extracts the LAST assistant turn on a multi-turn thread", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply"><p>first answer</p></div><div class="reply"><p>second answer</p></div>`);
    const pack = SelectorPack.fromYaml(
      ["response:", "  critical: true", "  strategies:", "    - { css: '.reply' }"].join("\n")
    );
    expect(await extractLastAssistantTurn(page, createResolver(page, pack))).toBe("second answer");
    await page.close();
  });

  it("keeps paragraph breaks inside nested wrappers (ChatGPT inline document block)", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply">
      <p>Here's a 400-word story:</p>
      <div class="doc"><div class="doc-head"><span>The Lighthouse Cat</span><button aria-label="Copy">copy</button></div>
        <div class="doc-body"><p>Milo had lived in the lighthouse.</p><p>He was a small gray cat.</p><p>Most people laughed.</p></div>
      </div></div>`);
    const pack = SelectorPack.fromYaml(
      ["response:", "  critical: true", "  strategies:", "    - { css: '.reply' }"].join("\n")
    );
    expect(await extractLastAssistantTurn(page, createResolver(page, pack))).toBe(
      "Here's a 400-word story:\n\nThe Lighthouse Cat\n\nMilo had lived in the lighthouse.\n\nHe was a small gray cat.\n\nMost people laughed."
    );
    await page.close();
  });

  it("drops ChatGPT's suggested follow-ups: buttons and their aria-hidden measuring copy (live 2026-09-28)", async () => {
    const page = await browser.newPage();
    // Mirrors the live DOM under [data-markdown-text-style='assistant-message'].
    await page.setContent(`<div class="reply">
      <div data-testid="chatgpt-writing-block"><p>Rivers bend because flowing water wears away the land.</p></div>
      <div class="relative mb-4 min-w-0 overflow-visible"><div class="relative min-w-0 px-3 text-base">
        <div class="flex min-w-0 flex-col items-start">
          <button class="relative flex group/suggested-followup"><svg></svg><span>Make the paragraph closer to 80 words</span></button>
          <button class="relative flex group/suggested-followup"><svg></svg><span>Explain how water speed changes</span></button>
        </div>
        <div class="pointer-events-none invisible absolute hidden" aria-hidden="true">
          <div class="flex"><svg></svg><span class="whitespace-nowrap">Make the paragraph closer to 80 words</span></div>
          <div class="flex"><svg></svg><span class="whitespace-nowrap">Explain how water speed changes</span></div>
        </div>
      </div></div></div>`);
    const pack = SelectorPack.fromYaml(
      ["response:", "  critical: true", "  strategies:", "    - { css: '.reply' }"].join("\n")
    );
    expect(await extractLastAssistantTurn(page, createResolver(page, pack))).toBe(
      "Rivers bend because flowing water wears away the land."
    );
    await page.close();
  });

  it("keeps a wrapper that holds both the reply and the follow-ups", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply"><div class="wrap">
      <p>The answer.</p>
      <div><button class="group/suggested-followup"><span>Tell me more</span></button></div>
    </div></div>`);
    const pack = SelectorPack.fromYaml(
      ["response:", "  critical: true", "  strategies:", "    - { css: '.reply' }"].join("\n")
    );
    expect(await extractLastAssistantTurn(page, createResolver(page, pack))).toBe("The answer.");
    await page.close();
  });

  it("tolerates keepNames-transpiled helpers: __name resolves in the page", async () => {
    const page = await browser.newPage();
    await page.setContent(`<div class="reply"><p>x</p></div>`);
    const pack = SelectorPack.fromYaml(
      ["response:", "  critical: true", "  strategies:", "    - { css: '.reply' }"].join("\n")
    );
    await extractLastAssistantTurn(page, createResolver(page, pack));
    // What tsx/esbuild keepNames emits inside the serialized function body.
    expect(await page.evaluate("typeof __name(function f() {}, 'f')")).toBe("function");
    await page.close();
  });
});
