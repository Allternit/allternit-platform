/**
 * DTCG importer: color / typography / radius / spacing subset. Resolves `{a.b}`
 * aliases and `$type` inheritance, and emits `--vp-<path>` custom properties.
 * Values that could break out of a declaration are dropped and reported.
 * Same rules as the importer in allternit-ai `src/lib/vendor-packs/dtcg.ts`, so a
 * pack that imports there imports here identically.
 */
import type { DtcgImport, ImportedToken, TokenKind } from "./types.js";

const TYPE_TO_KIND: Record<string, TokenKind> = {
  color: "color",
  fontFamily: "typography",
  fontWeight: "typography",
  dimension: "spacing",
  number: "typography",
};

const GROUP_TO_KIND: Record<string, TokenKind> = {
  color: "color",
  colors: "color",
  font: "typography",
  fonts: "typography",
  typography: "typography",
  radius: "radius",
  radii: "radius",
  borderRadius: "radius",
  spacing: "spacing",
  space: "spacing",
};

const UNSAFE = /[;{}<>\\]|url\s*\(|expression\s*\(|@import|\/\*/i;
const COLOR = /^(#[0-9a-f]{3,8}|(rgb|hsl|oklch|oklab|lab|lch|color)a?\([^()]*\)|[a-z]+)$/i;
const DIMENSION = /^-?\d*\.?\d+(px|rem|em|%)?$/;
const FONT_WEIGHT = /^(\d{3}|normal|bold|lighter|bolder)$/i;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

interface Leaf {
  path: string[];
  raw: unknown;
  type?: string;
}

function collect(node: Record<string, unknown>, path: string[], inheritedType: string | undefined, out: Leaf[]): void {
  const type = typeof node.$type === "string" ? node.$type : inheritedType;
  if ("$value" in node) {
    out.push({ path, raw: node.$value, type });
    return;
  }
  for (const [k, v] of Object.entries(node)) {
    if (k.startsWith("$") || !isObj(v)) continue;
    collect(v, [...path, k], type, out);
  }
}

function stringify(raw: unknown): string | null {
  if (typeof raw === "string") return raw.trim();
  if (typeof raw === "number") return String(raw);
  if (isObj(raw) && typeof raw.value === "number" && typeof raw.unit === "string") return `${raw.value}${raw.unit}`;
  if (Array.isArray(raw) && raw.every((x) => typeof x === "string")) {
    return (raw as string[]).map((f) => (/[\s,]/.test(f) ? `"${f.replace(/"/g, "")}"` : f)).join(", ");
  }
  return null;
}

export const cssVarName = (path: string[]): string =>
  "--vp-" + path.map((p) => p.replace(/[^a-zA-Z0-9]+/g, "-").replace(/^-|-$/g, "").toLowerCase()).join("-");

export function importDtcg(doc: unknown): DtcgImport {
  const issues: string[] = [];
  if (!isObj(doc)) return { tokens: [], vars: {}, issues: ["tokens: expected an object"] };

  const leaves: Leaf[] = [];
  collect(doc, [], undefined, leaves);
  const byPath = new Map(leaves.map((l) => [l.path.join("."), l]));

  const resolve = (leaf: Leaf, seen: Set<string>): string | null => {
    const s = stringify(leaf.raw);
    if (s === null) return null;
    const alias = /^\{([^{}]+)\}$/.exec(s);
    if (!alias) return s;
    const key = alias[1];
    if (seen.has(key)) {
      issues.push(`${leaf.path.join(".")}: alias cycle at {${key}}`);
      return null;
    }
    const target = byPath.get(key);
    if (!target) {
      issues.push(`${leaf.path.join(".")}: unknown alias {${key}}`);
      return null;
    }
    return resolve(target, new Set(seen).add(key));
  };

  const tokens: ImportedToken[] = [];
  for (const leaf of leaves) {
    const name = leaf.path.join(".");
    const kind = GROUP_TO_KIND[leaf.path[0]] ?? (leaf.type ? TYPE_TO_KIND[leaf.type] : undefined);
    if (!kind) {
      issues.push(`${name}: not a color, typography, radius or spacing token — skipped`);
      continue;
    }
    const value = resolve(leaf, new Set([name]));
    if (value === null) {
      if (!issues.some((i) => i.startsWith(`${name}:`))) issues.push(`${name}: unsupported value — skipped`);
      continue;
    }
    if (UNSAFE.test(value)) {
      issues.push(`${name}: value contains unsafe CSS — skipped`);
      continue;
    }
    const valid =
      kind === "color"
        ? COLOR.test(value)
        : kind === "radius" || kind === "spacing"
          ? DIMENSION.test(value)
          : leaf.type === "fontWeight"
            ? FONT_WEIGHT.test(value)
            : leaf.type === "number" || leaf.type === "dimension"
              ? DIMENSION.test(value)
              : value.length > 0 && value.length <= 200;
    if (!valid) {
      issues.push(`${name}: "${value}" is not a valid ${leaf.type ?? kind} value — skipped`);
      continue;
    }
    tokens.push({ path: leaf.path, kind, cssVar: cssVarName(leaf.path), value });
  }

  const vars: Record<string, string> = {};
  for (const t of tokens) vars[t.cssVar] = t.value;
  return { tokens, vars, issues };
}
