import { contrastRatio, MIN_CONTRAST } from "./contrast.js";
import { cssVarName, importDtcg } from "./dtcg.js";
import type { DtcgDocument, LookPack, LookPackFinding } from "./types.js";

export * from "./types.js";
export { contrastRatio, parseColor, MIN_CONTRAST } from "./contrast.js";
export { importDtcg, cssVarName } from "./dtcg.js";

const ID_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** Token pairs checked for contrast: [foreground, background]. Pairs a pack does not define are skipped. */
export const CONTRAST_PAIRS: ReadonlyArray<readonly [string, string]> = [
  ["color.on-brand", "color.brand"],
  ["color.text", "color.surface"],
  ["color.text-muted", "color.surface"],
  ["color.brand", "color.surface"],
];

/** The Allternit default look: white surface, neutral ink, used when a pack defines nothing. */
export const ALLTERNIT_DEFAULT_LOOK: LookPack = {
  id: "allternit-default",
  name: "Allternit default",
  version: "1.0.0",
  tokens: {
    color: {
      $type: "color",
      surface: { $value: "#ffffff" },
      text: { $value: "#1f1e1d" },
      "text-muted": { $value: "#5f5e5b" },
      brand: { $value: "#1f1e1d" },
      "on-brand": { $value: "#ffffff" },
    },
    radius: { card: { $value: "10px" } },
    spacing: { sm: { $value: "8px" }, md: { $value: "12px" } },
    font: { body: { $type: "fontFamily", $value: ["system-ui", "sans-serif"] } },
  },
  dark: {
    color: {
      $type: "color",
      surface: { $value: "#1f1e1d" },
      text: { $value: "#f5f4f2" },
      "text-muted": { $value: "#b4b2ad" },
      brand: { $value: "#f5f4f2" },
      "on-brand": { $value: "#1f1e1d" },
    },
  },
};

const f = (severity: LookPackFinding["severity"], code: string, message: string, subject?: string): LookPackFinding => ({
  severity,
  code,
  message,
  subject,
});

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** Custom properties (`--vp-*`) for one token document; invalid tokens are left out. */
export function toCssVariables(tokens: DtcgDocument): Record<string, string> {
  return importDtcg(tokens).vars;
}

/** A CSS rule for the pack, scoped to `[data-look-pack="<id>"]` (light) and its dark override. */
export function toCss(pack: LookPack, options: { selector?: string; darkSelector?: string } = {}): string {
  const id = pack.id.replace(/[^a-z0-9-]/g, "");
  const selector = options.selector ?? `[data-look-pack="${id}"]`;
  const block = (sel: string, vars: Record<string, string>) =>
    Object.keys(vars).length
      ? `${sel} {\n${Object.entries(vars).map(([k, v]) => `  ${k}: ${v};`).join("\n")}\n}`
      : "";
  const light = block(selector, toCssVariables(pack.tokens));
  const dark = pack.dark ? block(options.darkSelector ?? `[data-theme="dark"] ${selector}`, toCssVariables(pack.dark)) : "";
  return [light, dark].filter(Boolean).join("\n");
}

/** Contrast findings: a warning for every defined pair under 4.5:1 (light and dark). */
export function checkContrast(pack: LookPack): LookPackFinding[] {
  const out: LookPackFinding[] = [];
  const modes: Array<[string, Record<string, string>]> = [["light", toCssVariables(pack.tokens)]];
  if (pack.dark) modes.push(["dark", { ...modes[0][1], ...toCssVariables(pack.dark) }]);
  for (const [mode, vars] of modes) {
    for (const [fg, bg] of CONTRAST_PAIRS) {
      const fgVar = vars[cssVarName(fg.split("."))];
      const bgVar = vars[cssVarName(bg.split("."))];
      if (!fgVar || !bgVar) continue;
      const ratio = contrastRatio(fgVar, bgVar);
      if (ratio === null) {
        out.push(f("info", "contrast.unparsed", `${mode}: ${fg} on ${bg} could not be measured (${fgVar} / ${bgVar}).`, fg));
      } else if (ratio < MIN_CONTRAST) {
        out.push(f("warning", "contrast.low", `${mode}: ${fg} on ${bg} is ${ratio.toFixed(2)}:1; aim for at least ${MIN_CONTRAST}:1.`, fg));
      }
    }
  }
  return out;
}

/** Structural + token validation. Errors block use; warnings (contrast) do not. */
export function validateLookPack(pack: unknown): LookPackFinding[] {
  if (!isObj(pack)) return [f("error", "pack.shape", "A look pack must be an object.")];
  const out: LookPackFinding[] = [];
  if (typeof pack.id !== "string" || !ID_RE.test(pack.id)) {
    out.push(f("error", "pack.id", "id must be lowercase letters, digits and single dashes."));
  }
  if (typeof pack.name !== "string" || !pack.name.trim()) out.push(f("error", "pack.name", "name is required."));
  if (!isObj(pack.tokens)) {
    out.push(f("error", "pack.tokens", "tokens must be a DTCG object."));
    return out;
  }
  for (const [label, doc] of [["tokens", pack.tokens], ["dark", pack.dark]] as const) {
    if (doc === undefined) continue;
    if (!isObj(doc)) {
      out.push(f("error", "pack.dark", `${label} must be a DTCG object.`));
      continue;
    }
    const imported = importDtcg(doc);
    for (const issue of imported.issues) out.push(f("error", "token.invalid", issue, label));
    if (imported.tokens.length === 0) out.push(f("error", "token.none", `${label} defines no usable tokens.`, label));
  }
  if (!out.some((x) => x.severity === "error")) out.push(...checkContrast(pack as unknown as LookPack));
  return out;
}
