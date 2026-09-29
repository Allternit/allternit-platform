// §A3.1 — extractLastAssistantTurn: DOM → markdown (code blocks + citations).
import type { Page } from "playwright";
import type { SdkSelectorResolver } from "./selectors";

// The `response` locator resolves to the last assistant turn container.
export async function extractLastAssistantTurn(
  page: Page,
  resolver: SdkSelectorResolver,
  opts: { key?: string } = {}
): Promise<string> {
  const locator = await resolver.resolveLocator(opts.key ?? "response");
  // Runners that transpile with esbuild keepNames (tsx — how the gateway runs
  // on Sessions machines) wrap the named helpers below in __name(...), which
  // does not exist in the page. Define it as identity before serializing.
  await page.evaluate("globalThis.__name ??= (fn) => fn");
  const markdown = await locator.last().evaluate((root) => {
    function langOf(pre: Element): string {
      const code = pre.querySelector("code");
      const cls = (code?.getAttribute("class") ?? pre.getAttribute("class") ?? "") as string;
      const m = /language-([\w-]+)/.exec(cls);
      return m ? m[1] : "";
    }

    // Block-level tags. A container holding any of these is walked as blocks,
    // not flattened: ChatGPT nests replies in wrappers (e.g. its inline
    // document block: a title row + <p>s inside one <div>), and flattening
    // them ran paragraphs together ("remember.He was").
    const BLOCK_TAGS = new Set([
      "P", "PRE", "H1", "H2", "H3", "H4", "H5", "H6", "UL", "OL", "BLOCKQUOTE",
      "TABLE", "HR", "DIV", "SECTION", "ARTICLE", "HEADER", "FOOTER", "FIGURE",
    ]);
    // UI chrome inside a reply (copy/expand buttons, icons) is not content.
    const SKIP_TAGS = new Set(["BUTTON", "SVG", "STYLE", "SCRIPT", "TEMPLATE"]);
    // Also chrome: hidden layout clones (aria-hidden) and ChatGPT's suggested
    // follow-up prompts, which render inside the reply root — as buttons plus
    // an invisible aria-hidden measuring copy of the same text (live
    // 2026-09-28: "Make the paragraph closer to 80 words" ended a reply).
    function isChrome(el: Element): boolean {
      if (SKIP_TAGS.has(el.tagName.toUpperCase())) return true;
      if (el.getAttribute("aria-hidden") === "true") return true;
      const cls = typeof el.className === "string" ? el.className : "";
      if (cls.includes("suggested-followup")) return true;
      // A block holding the suggestions and no reply content (never a wrapper
      // that also holds the reply itself).
      return (
        el.querySelector("[class*='suggested-followup']") !== null &&
        el.querySelector("p, pre, ul, ol, table, blockquote, h1, h2, h3, h4, h5, h6, [data-testid='chatgpt-writing-block']") === null
      );
    }

    function inline(node: Node): string {
      if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? "";
      if (node.nodeType !== Node.ELEMENT_NODE) return "";
      const el = node as Element;
      const tag = el.tagName.toUpperCase();
      if (isChrome(el)) return "";
      if (tag === "BR") return "\n";
      if (tag === "CODE") return `\`${el.textContent ?? ""}\``;
      if (tag === "A") {
        const href = el.getAttribute("href") ?? "";
        const text = (el.textContent ?? "").trim() || href;
        return href ? `[${text}](${href})` : text;
      }
      if (tag === "STRONG" || tag === "B") return `**${children(el)}**`;
      if (tag === "EM" || tag === "I") return `*${children(el)}*`;
      return children(el);
    }

    function children(el: Element): string {
      return Array.from(el.childNodes).map(inline).join("");
    }

    function block(el: Element, listDepth: number): string {
      const tag = el.tagName.toUpperCase();
      if (tag === "PRE") {
        const code = (el.textContent ?? "").replace(/\n$/, "");
        return `\`\`\`${langOf(el)}\n${code}\n\`\`\``;
      }
      if (/^H[1-6]$/.test(tag)) {
        return `${"#".repeat(Number(tag[1]))} ${children(el).trim()}`;
      }
      if (tag === "UL" || tag === "OL") {
        return Array.from(el.children)
          .filter((li) => li.tagName.toUpperCase() === "LI")
          .map((li, i) => {
            const marker = tag === "UL" ? "-" : `${i + 1}.`;
            return `${"  ".repeat(listDepth)}${marker} ${children(li).trim()}`;
          })
          .join("\n");
      }
      if (tag === "BLOCKQUOTE") {
        return children(el)
          .trim()
          .split("\n")
          .map((l) => `> ${l}`)
          .join("\n");
      }
      if (tag !== "P" && Array.from(el.children).some((c) => BLOCK_TAGS.has(c.tagName.toUpperCase()))) {
        return blocks(el);
      }
      return children(el).trim();
    }

    // Children as markdown blocks: each block element is its own block; runs
    // of inline nodes between them are grouped into one paragraph.
    function blocks(el: Element): string {
      const out: string[] = [];
      let run = "";
      const flush = (): void => {
        if (run.trim()) out.push(run.trim());
        run = "";
      };
      for (const node of Array.from(el.childNodes)) {
        const child = node as Element;
        if (node.nodeType === Node.ELEMENT_NODE && isChrome(child)) continue;
        if (node.nodeType === Node.ELEMENT_NODE && BLOCK_TAGS.has(child.tagName.toUpperCase())) {
          flush();
          out.push(block(child, 0));
        } else {
          run += inline(node);
        }
      }
      flush();
      return out.filter((s) => s.length > 0).join("\n\n");
    }

    return blocks(root);
  });
  return markdown.trim();
}
