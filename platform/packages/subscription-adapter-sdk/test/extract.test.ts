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
