import { describe, expect, it } from "vitest";
import {
  expiryFromDate,
  isValidCidr,
  keyStatus,
  normalizeCidr,
  parseCidrList,
  validateExpiry,
} from "../governance";

const NOW = new Date("2026-11-12T12:00:00Z");
const inDays = (d: number) => new Date(NOW.getTime() + d * 86400000).toISOString();

describe("keyStatus", () => {
  it("is Active with no expiry", () => {
    expect(keyStatus({ expires_at: null }, NOW)).toEqual({ state: "active", daysLeft: null, label: "Active" });
    expect(keyStatus({}, NOW).label).toBe("Active");
  });
  it("is Expired at or past the expiry", () => {
    expect(keyStatus({ expires_at: inDays(-1) }, NOW)).toMatchObject({ state: "expired", label: "Expired" });
    expect(keyStatus({ expires_at: NOW.toISOString() }, NOW).state).toBe("expired");
  });
  it("counts days left and flags keys expiring soon", () => {
    expect(keyStatus({ expires_at: inDays(3) }, NOW)).toMatchObject({ state: "expiring", daysLeft: 3, label: "Expires in 3 days" });
    expect(keyStatus({ expires_at: inDays(1) }, NOW).label).toBe("Expires in 1 day");
    expect(keyStatus({ expires_at: inDays(60) }, NOW)).toMatchObject({ state: "active", label: "Expires in 60 days" });
  });
  it("rounds a part day up so a live key never reads 0 days", () => {
    const soon = new Date(NOW.getTime() + 3600_000).toISOString();
    expect(keyStatus({ expires_at: soon }, NOW).label).toBe("Expires in 1 day");
  });
});

describe("isValidCidr", () => {
  it("accepts IPv4 and IPv6 ranges and bare addresses", () => {
    for (const v of ["10.0.0.0/8", "203.0.113.7", "0.0.0.0/0", "2001:db8::/32", "::1", "fe80::1/128"]) {
      expect(isValidCidr(v), v).toBe(true);
    }
  });
  it("rejects bad input", () => {
    for (const v of ["", "256.1.1.1", "10.0.0/8", "10.0.0.0/33", "10.0.0.0/", "abc", "1.2.3.4/-1", "2001:db8:::1", "2001:db8::/129", "01.2.3.4", "1:2:3:4:5:6:7:8:9"]) {
      expect(isValidCidr(v), v).toBe(false);
    }
  });
});

describe("parseCidrList", () => {
  it("splits, normalizes, dedupes and reports invalid entries", () => {
    const r = parseCidrList("10.0.0.1, 10.0.0.1/32\n2001:db8::1 bogus 192.168.0.0/16");
    expect(r.cidrs).toEqual(["10.0.0.1/32", "2001:db8::1/128", "192.168.0.0/16"]);
    expect(r.invalid).toEqual(["bogus"]);
    expect(normalizeCidr("1.2.3.4")).toBe("1.2.3.4/32");
  });
});

describe("validateExpiry", () => {
  it("requires an expiry when the policy says so", () => {
    expect(validateExpiry(null, { require_key_expiry: true, max_key_ttl_days: null }, NOW)).toMatch(/requires/);
    expect(validateExpiry(null, { require_key_expiry: false, max_key_ttl_days: null }, NOW)).toBeNull();
  });
  it("caps lifetime at max_key_ttl_days and rejects the past", () => {
    const policy = { require_key_expiry: true, max_key_ttl_days: 30 };
    expect(validateExpiry(inDays(30), policy, NOW)).toBeNull();
    expect(validateExpiry(inDays(31), policy, NOW)).toMatch(/30 days/);
    expect(validateExpiry(inDays(-1), policy, NOW)).toMatch(/future/);
  });
  it("parses a custom date as end of that day", () => {
    expect(expiryFromDate("2026-12-01")).not.toBeNull();
    expect(expiryFromDate("12/01/2026")).toBeNull();
  });
});
