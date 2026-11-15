import { describe, expect, it } from "vitest";
import { DEFAULT_AUTO_POLICY, resolveAutoModel, type PolicyCatalogModel } from "../model-auto-policy";

const catalog: PolicyCatalogModel[] = [
  { id: "fast-mini", owned_by: "a", quality_tier: "fast", pricing: { input_cents_per_1m: 10, output_cents_per_1m: 40 } },
  { id: "high-pricey", owned_by: "b", quality_tier: "high", pricing: { input_cents_per_1m: 300, output_cents_per_1m: 1500 } },
  { id: "high-value", owned_by: "c", quality_tier: "high", pricing: { input_cents_per_1m: 100, output_cents_per_1m: 400 } },
];

describe("resolveAutoModel with the default policy", () => {
  it("picks a model from a non-empty catalog (new account, nothing saved)", () => {
    expect(resolveAutoModel(catalog, DEFAULT_AUTO_POLICY)).toBe("high-value");
  });

  it("still picks a model when the catalog has no tier or price data", () => {
    expect(
      resolveAutoModel([{ id: "bare-1", owned_by: "x" }, { id: "bare-2", owned_by: "y" }], DEFAULT_AUTO_POLICY)
    ).toBe("bare-1");
  });

  it("returns null only for an empty catalog", () => {
    expect(resolveAutoModel([], DEFAULT_AUTO_POLICY)).toBeNull();
  });
});
