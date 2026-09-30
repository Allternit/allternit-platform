import { describe, expect, it } from "vitest";
import {
  ALLTERNIT_DEFAULT_LOOK,
  checkContrast,
  contrastRatio,
  importDtcg,
  toCss,
  toCssVariables,
  validateLookPack,
  type LookPack,
} from "../src/index.js";

const pack = (over: Partial<LookPack> = {}): LookPack => ({
  id: "acme",
  name: "Acme",
  tokens: {
    color: {
      $type: "color",
      surface: { $value: "#ffffff" },
      text: { $value: "#111111" },
      brand: { $value: "#0b5fff" },
      "on-brand": { $value: "#ffffff" },
    },
    radius: { card: { $value: "8px" } },
  },
  ...over,
});

describe("toCssVariables", () => {
  it("emits --vp-* variables and resolves aliases", () => {
    const vars = toCssVariables({
      color: { $type: "color", base: { $value: "#0b5fff" }, brand: { $value: "{color.base}" } },
    });
    expect(vars["--vp-color-brand"]).toBe("#0b5fff");
  });

  it("drops values that could break out of a declaration", () => {
    const r = importDtcg({ color: { $type: "color", bad: { $value: "red; background: url(x)" } } });
    expect(r.vars).toEqual({});
    expect(r.issues.join()).toMatch(/unsafe/);
  });

  it("reports alias cycles and unknown aliases", () => {
    const r = importDtcg({ color: { $type: "color", a: { $value: "{color.b}" }, b: { $value: "{color.a}" }, c: { $value: "{nope}" } } });
    expect(r.issues.some((i) => /cycle/.test(i))).toBe(true);
    expect(r.issues.some((i) => /unknown alias/.test(i))).toBe(true);
  });
});

describe("toCss", () => {
  it("scopes light tokens to the pack id and dark to the theme", () => {
    const css = toCss(pack({ dark: { color: { $type: "color", surface: { $value: "#000000" } } } }));
    expect(css).toContain('[data-look-pack="acme"] {');
    expect(css).toContain("--vp-radius-card: 8px;");
    expect(css).toContain('[data-theme="dark"] [data-look-pack="acme"] {');
  });

  it("strips characters from the id that could escape the selector", () => {
    expect(toCss(pack({ id: 'a"]{x' }))).toContain('[data-look-pack="ax"]');
  });
});

describe("contrast", () => {
  it("computes the WCAG ratio", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 0);
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 1);
    expect(contrastRatio("oklch(0.5 0.1 200)", "#ffffff")).toBeNull();
  });

  it("warns under 4.5:1 and stays quiet above it", () => {
    expect(checkContrast(pack())).toEqual([]);
    const low = pack({
      tokens: { color: { $type: "color", surface: { $value: "#ffffff" }, text: { $value: "#999999" } } },
    });
    const findings = checkContrast(low);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({ severity: "warning", code: "contrast.low" });
  });

  it("checks dark overrides against the merged tokens", () => {
    const findings = checkContrast(pack({ dark: { color: { $type: "color", surface: { $value: "#111111" } } } }));
    expect(findings.some((x) => x.message.startsWith("dark:"))).toBe(true);
  });

  it("ships a default look with no contrast warnings", () => {
    expect(checkContrast(ALLTERNIT_DEFAULT_LOOK)).toEqual([]);
  });
});

describe("validateLookPack", () => {
  it("accepts a good pack", () => {
    expect(validateLookPack(pack()).filter((x) => x.severity === "error")).toEqual([]);
  });

  it("rejects bad ids, missing names and empty tokens", () => {
    const codes = validateLookPack({ id: "Bad Id", tokens: {} }).map((x) => x.code);
    expect(codes).toEqual(expect.arrayContaining(["pack.id", "pack.name", "token.none"]));
  });

  it("rejects non-objects", () => {
    expect(validateLookPack(null)[0].code).toBe("pack.shape");
  });
});
