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

    function inline(node: Node): string {
      if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? "";
      if (node.nodeType !== Node.ELEMENT_NODE) return "";
      const el = node as Element;
      const tag = el.tagName.toUpperCase();
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
      return children(el).trim();
    }

    return Array.from(root.childNodes)
      .map((node) => {
        if (node.nodeType === Node.TEXT_NODE) return (node.textContent ?? "").trim();
        if (node.nodeType === Node.ELEMENT_NODE) return block(node as Element, 0);
        return "";
      })
      .filter((s) => s.length > 0)
      .join("\n\n");
  });
  return markdown.trim();
}
