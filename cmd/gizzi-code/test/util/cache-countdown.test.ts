import { describe, expect, it } from "bun:test";
import { cacheCountdown } from "../../src/shared/util/cache-countdown";

describe("cache countdown", () => {
  const usage = { cacheTtlSeconds: 300, cacheExpiresAt: 400000, cacheRecacheTokens: 82000, inputTokens: 1000, cacheReadTokens: 9000 };
  it("moves from warm through warning to cold without a new response", () => {
    expect(cacheCountdown(usage, 100000)?.state).toBe("warm");
    expect(cacheCountdown(usage, 340000)?.state).toBe("warm");
    expect(cacheCountdown(usage, 340001)?.state).toBe("expiring");
    expect(cacheCountdown(usage, 400000)?.label).toContain("may re-cache 82k tokens");
    expect(cacheCountdown(usage, 500000)?.fraction).toBe(0);
  });
  it("omits unknown/invalid expiry and clamps clock skew", () => {
    expect(cacheCountdown(undefined, 0)).toBeUndefined();
    expect(cacheCountdown({ cacheReadTokens: 9000 }, 0)).toBeUndefined();
    expect(cacheCountdown({ ...usage, cacheTtlSeconds: NaN }, 0)).toBeUndefined();
    expect(cacheCountdown(usage, 0)?.fraction).toBe(1);
  });
  it("identifies per-request hit rate and avoids invented misses", () => {
    const label = cacheCountdown(usage, 100000)?.label;
    expect(label).toContain("last hit 90%");
    expect(label).not.toContain("misses");
    expect(cacheCountdown({ ...usage, cacheReadTokens: undefined }, 100000)?.label).not.toContain("hit");
  });
});

import { usageFromMessageInfo } from "../../src/runtime/server/routes/tool-frames";

describe("cache countdown bridge", () => {
  it("preserves the request TTL and timestamp and computes the input footprint", () => {
    const wire = usageFromMessageInfo({ tokens: { input: 1000, output: 20, cache: { read: 80000, write: 1000, ttlSeconds: 300, refreshedAt: 100000 } } });
    expect(wire?.cacheExpiresAt).toBe(400000);
    expect(wire?.cacheRecacheTokens).toBe(82000);
    expect(cacheCountdown(wire, 400000)?.state).toBe("cold");
  });
  it("does not invent expiry from cache reads alone", () => {
    const wire = usageFromMessageInfo({ tokens: { input: 1000, cache: { read: 80000 } } });
    expect(wire?.cacheTtlSeconds).toBeUndefined();
    expect(cacheCountdown(wire, Date.now())).toBeUndefined();
  });
});
