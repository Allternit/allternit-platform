// Tiny dependency-free HTML parser + selector engine. Grok Bot's renderer HTML is fetched over CDP as a string
// (outerHTML) and parsed here, so the exact same observation code runs against live pages and offline fixtures.
export interface El { tag: string; attrs: Record<string, string>; children: Array<El | string>; parent: El | null }

const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);
const RAW = new Set(["script", "style"]);
const ENT: Record<string, string> = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#39;": "'", "&nbsp;": " ", "&hellip;": "…" };
const decode = (s: string) => s.replace(/&(amp|lt|gt|quot|nbsp|hellip|#39);/g, (m) => ENT[m] ?? m);

export function parseHtml(html: string): El {
  const root: El = { tag: "#root", attrs: {}, children: [], parent: null };
  let cur = root;
  const re = /<!--[\s\S]*?-->|<!doctype[^>]*>|<\/([a-zA-Z][\w-]*)\s*>|<([a-zA-Z][\w-]*)((?:"[^"]*"|'[^']*'|[^>"'])*?)(\/?)>|[^<]+|</gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    if (m[1]) { // close tag: pop to matching ancestor if any
      const t = m[1].toLowerCase();
      for (let n: El | null = cur; n && n !== root; n = n.parent) if (n.tag === t) { cur = n.parent ?? root; break; }
    } else if (m[2]) {
      const tag = m[2].toLowerCase(), attrs: Record<string, string> = {};
      for (const a of m[3].matchAll(/([^\s=/>"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g)) attrs[a[1].toLowerCase()] = decode(a[2] ?? a[3] ?? a[4] ?? "");
      const el: El = { tag, attrs, children: [], parent: cur };
      cur.children.push(el);
      if (RAW.has(tag)) { const end = html.toLowerCase().indexOf(`</${tag}`, re.lastIndex); if (end >= 0) re.lastIndex = html.indexOf(">", end) + 1; }
      else if (!VOID.has(tag) && !m[4]) cur = el;
    } else if (!m[0].startsWith("<!") && m[0] !== "<") cur.children.push(decode(m[0]));
  }
  return root;
}

export function textOf(el: El): string {
  return el.children.map((c) => (typeof c === "string" ? c : c.tag === "br" ? "\n" : textOf(c))).join("").replace(/[ \t\r\f]+/g, " ").replace(/ ?\n ?/g, "\n").trim();
}
function walk(el: El, out: El[] = []): El[] { for (const c of el.children) if (typeof c !== "string") { out.push(c); walk(c, out); } return out; }

interface Simple { tag?: string; id?: string; classes: string[]; attrs: Array<{ k: string; op: string; v: string }> }
type Chain = Array<{ simple: Simple; comb: " " | ">" }>;

function parseSelector(sel: string): Chain[] {
  return sel.split(",").map((part) => {
    const chain: Chain = []; let comb: " " | ">" = " ";
    for (const tok of part.trim().match(/\[[^\]]*\]|[>]|[^\s>\[]+(?:\[[^\]]*\])*/g) ?? []) {
      if (tok === ">") { comb = ">"; continue; }
      const s: Simple = { classes: [], attrs: [] };
      for (const m of tok.matchAll(/^([\w*-]+)|#([\w-]+)|\.([\w-]+)|\[\s*([\w:-]+)\s*(?:([*^$~|]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\]]*?)))?\s*\]/g)) {
        if (m[1]) s.tag = m[1] === "*" ? undefined : m[1].toLowerCase();
        else if (m[2]) s.id = m[2]; else if (m[3]) s.classes.push(m[3]);
        else s.attrs.push({ k: m[4].toLowerCase(), op: m[5] ?? "", v: m[6] ?? m[7] ?? m[8] ?? "" });
      }
      chain.push({ simple: s, comb }); comb = " ";
    }
    return chain;
  });
}
function matchSimple(el: El, s: Simple): boolean {
  if (s.tag && el.tag !== s.tag) return false;
  if (s.id && el.attrs.id !== s.id) return false;
  if (s.classes.length) { const cl = (el.attrs.class ?? "").split(/\s+/); if (!s.classes.every((c) => cl.includes(c))) return false; }
  for (const a of s.attrs) {
    const v = el.attrs[a.k]; if (v === undefined) return false;
    if (a.op === "=" && v !== a.v) return false;
    if (a.op === "*=" && !v.includes(a.v)) return false;
    if (a.op === "^=" && !v.startsWith(a.v)) return false;
    if (a.op === "$=" && !v.endsWith(a.v)) return false;
    if (a.op === "~=" && !v.split(/\s+/).includes(a.v)) return false;
  }
  return true;
}
function matchChain(el: El, chain: Chain, i = chain.length - 1): boolean {
  if (i < 0) return true;
  if (!matchSimple(el, chain[i].simple)) return false;
  if (i === 0) return true;
  if (chain[i].comb === ">") return !!el.parent && el.parent.tag !== "#root" && matchChain(el.parent, chain, i - 1);
  for (let p = el.parent; p && p.tag !== "#root"; p = p.parent) if (matchChain(p, chain, i - 1)) return true;
  return false;
}
export function queryAll(root: El, selector: string): El[] {
  const chains = parseSelector(selector);
  return walk(root).filter((el) => chains.some((c) => c.length && matchChain(el, c)));
}
export const query = (root: El, selector: string): El | null => queryAll(root, selector)[0] ?? null;
export const allElements = (root: El): El[] => walk(root);
/** Accessible name approximation: aria-label, else visible text, else title. */
export const nameOf = (el: El): string => el.attrs["aria-label"] ?? (textOf(el) || el.attrs.title || "");
