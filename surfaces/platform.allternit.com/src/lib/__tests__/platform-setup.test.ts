import { describe, expect, it } from "vitest";
import {
  blockStates,
  buildSnippet,
  describePlan,
  edgeFrom,
  litBlocks,
  maskKey,
  paidPlans,
  readMetadataRecord,
  rippleDelays,
  shouldOfferSetup,
  stepperStates,
} from "../platform-setup";

describe("shouldOfferSetup", () => {
  const empty = { record: null, orgCount: 0, gatewayKeys: 0, scopedKeys: 0 };
  it("offers setup only to a sure-new account", () => {
    expect(shouldOfferSetup(empty)).toBe(true);
  });
  it("never offers when anything is unknown or present", () => {
    expect(shouldOfferSetup({ ...empty, record: { status: "skipped", at: "x" } })).toBe(false);
    expect(shouldOfferSetup({ ...empty, orgCount: 1 })).toBe(false);
    expect(shouldOfferSetup({ ...empty, orgCount: null })).toBe(false);
    expect(shouldOfferSetup({ ...empty, gatewayKeys: null })).toBe(false);
    expect(shouldOfferSetup({ ...empty, scopedKeys: 2 })).toBe(false);
  });
});

describe("maskKey", () => {
  it("shows a server prefix but never a full token", () => {
    expect(maskKey("ak-7f3c")).toBe("ak-7f3c…••••");
    const masked = maskKey("ak-7f3c9e1d2b4a6c8e0f1a3b5c7d9e1f2a");
    expect(masked).toBe("ak-7f3c9…••••");
    expect(masked).not.toContain("0f1a3b");
    expect(maskKey(null)).toBe("••••••••");
  });
});

describe("buildSnippet", () => {
  it("targets api.allternit.com/v1 with the given key and model", () => {
    for (const lang of ["curl", "python", "ts"] as const) {
      const s = buildSnippet({ lang, model: "together/llama", key: "KEY" });
      expect(s).toContain("api.allternit.com/v1");
      expect(s).toContain("KEY");
      expect(s).toContain('"together/llama"');
    }
    expect(buildSnippet({ lang: "curl", model: "m", key: "k" })).toContain("/v1/chat/completions");
  });
});

describe("progress", () => {
  it("lights blocks per step with one coral lead until done", () => {
    expect(litBlocks(1)).toBe(6);
    const b = blockStates(1);
    expect(b.filter((x) => x === "on")).toHaveLength(5);
    expect(b[5]).toBe("lead");
    expect(blockStates(6).every((x) => x === "on")).toBe(true);
  });
  it("marks stepper states", () => {
    expect(stepperStates(3).map((s) => s.state)).toEqual(["done", "done", "now", "", ""]);
  });
});

describe("plans", () => {
  it("uses only API numbers and drops the free plan", () => {
    const plans = paidPlans([
      { id: "super", label: "Super", price_usd: 100, monthly_credits_usd: 110 },
      { id: "free", label: "Free", price_usd: 0, monthly_credits_usd: 0 },
      { id: "plus", label: "Plus", price_usd: 20, monthly_credits_usd: 22 },
    ]);
    expect(plans.map((p) => p.id)).toEqual(["plus", "super"]);
    expect(describePlan(plans[0])).toBe("$20/month · $22 monthly credits");
  });
});

describe("helpers", () => {
  it("reads only valid metadata records", () => {
    expect(readMetadataRecord({ platformSetup: { status: "completed", at: "t" } })?.status).toBe("completed");
    expect(readMetadataRecord({ platformSetup: { status: "nope" } })).toBeNull();
    expect(readMetadataRecord(undefined)).toBeNull();
  });
  it("computes edges and ripple distances", () => {
    expect(edgeFrom(0, 0, 3, 4).len).toBe(5);
    const d = rippleDelays(3, 3, 1, 1);
    expect(d[4]).toBe(0);
    expect(Math.max(...d)).toBe(1);
  });
});
