// AX selector format v1: role (+subrole) + label regex + optional ancestor path. Serialisable JSON, versioned.
// A selector never names coordinates or child indexes (those drift); it names what a person/AT would call the thing.
import { AxError, labelOf, walk, type AxNode } from "./types.js";

export const AX_SELECTOR_FORMAT = 1;
export type AxConfidence = "unverified" | "verified";
export type AxLabelField = "title" | "description" | "value" | "identifier";
export interface AxPathStep { role: string; subrole?: string; /** regex source over title+description */ label?: string }
export interface AxSelector {
  v: typeof AX_SELECTOR_FORMAT;
  role: string;
  subrole?: string;
  /** regex source (case-insensitive) tested against the fields in `labelFields` (default title+description). */
  label?: string;
  labelFields?: AxLabelField[];
  /** Ancestor chain, outermost first; steps must appear in this order among the node's ancestors (gaps allowed). */
  path?: AxPathStep[];
  /** Pick the nth match (document order); default all. */
  nth?: number;
}
export interface AxKeySpec { critical: boolean; confidence: AxConfidence; alternatives: AxSelector[] }
export interface AxSelectorPack {
  /** Selector FORMAT version this pack is written in. */
  format: number;
  /** The pack's own version; bump when the live UI drifts. */
  packVersion: string;
  bundleId: string;
  keys: Record<string, AxKeySpec>;
}

export function assertPackCompatible(p: AxSelectorPack) {
  if (p.format !== AX_SELECTOR_FORMAT) throw new AxError("selector_version", `AX selector pack ${p.packVersion} uses format ${p.format}; this build reads format ${AX_SELECTOR_FORMAT}`);
  for (const [k, spec] of Object.entries(p.keys)) for (const s of spec.alternatives) {
    if (s.v !== AX_SELECTOR_FORMAT) throw new AxError("selector_version", `AX selector ${k} is format ${s.v}; expected ${AX_SELECTOR_FORMAT}`);
    for (const re of [s.label, ...(s.path ?? []).map((x) => x.label)]) if (re) new RegExp(re, "i"); // throws on a bad regex
  }
}

const rx = (src: string) => new RegExp(src, "i");
function stepMatches(n: AxNode, s: { role: string; subrole?: string; label?: string }, fields?: AxLabelField[]): boolean {
  if (n.role !== s.role) return false;
  if (s.subrole && n.subrole !== s.subrole) return false;
  if (s.label) {
    const hay = fields ? fields.map((f) => n[f] ?? "").join(" ") : labelOf(n);
    if (!rx(s.label).test(hay)) return false;
  }
  return true;
}

/** All nodes matching the selector, document order. */
export function resolveSelector(root: AxNode, sel: AxSelector): AxNode[] {
  const out: AxNode[] = [];
  for (const { node, ancestors } of walk(root)) {
    if (!stepMatches(node, sel, sel.labelFields)) continue;
    if (sel.path?.length) {
      let i = 0;
      for (const a of ancestors) if (i < sel.path.length && stepMatches(a, sel.path[i])) i++;
      if (i < sel.path.length) continue;
    }
    out.push(node);
  }
  return sel.nth === undefined ? out : out.slice(sel.nth, sel.nth + 1);
}
/** First alternative that resolves wins (mirrors the CSS fallback lists in the DOM packs). */
export function resolveKey(root: AxNode, pack: AxSelectorPack, key: string): AxNode[] {
  const spec = pack.keys[key];
  if (!spec) throw new Error(`unknown AX selector key ${key}`);
  for (const alt of spec.alternatives) { const r = resolveSelector(root, alt); if (r.length) return r; }
  return [];
}
/** Union of several keys in document order (paths compare lexicographically). */
export function resolveKeys(root: AxNode, pack: AxSelectorPack, keys: string[]): Array<{ key: string; node: AxNode }> {
  const out = keys.flatMap((key) => resolveKey(root, pack, key).map((node) => ({ key, node })));
  const cmp = (a: number[], b: number[]) => { for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i]; return a.length - b.length; };
  return out.sort((x, y) => cmp(x.node.path, y.node.path));
}
/** Critical keys that resolve to nothing: drift evidence. */
export function missingCritical(root: AxNode, pack: AxSelectorPack): string[] {
  return Object.entries(pack.keys).filter(([k, s]) => s.critical && !resolveKey(root, pack, k).length).map(([k]) => k);
}
