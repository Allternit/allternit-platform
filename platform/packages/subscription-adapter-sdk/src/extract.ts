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

    function inline(node: Node): string {
      if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? "";
      if (node.nodeType !== Node.ELEMENT_NODE) return "";
      const el = node as Element;
      const tag = el.tagName.toUpperCase();
      if (SKIP_TAGS.has(tag)) return "";
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
        if (node.nodeType === Node.ELEMENT_NODE && SKIP_TAGS.has(child.tagName.toUpperCase())) continue;
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
