import { describe, expect, it } from "vitest";
import { Webhook } from "standardwebhooks";
import {
  EventsErrorCode,
  SecretError,
  VerifyError,
  canonicalJson,
  eventEnvelope,
  gapEnvelope,
  isPublicIp,
  parseSecret,
  sign,
  signHeaders,
  subscriptionId,
  terminatedEnvelope,
  validateCallbackUrl,
  validateWebhook,
  verificationEnvelope,
  verify,
  verifyRequest,
} from "../src/index.js";

// Reference vector from the Standard Webhooks / Svix docs (same as the Rust crate).
const SECRET = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
const MSG_ID = "msg_p5jXN8AQM9LWM0D4loKWxJek";
const TS = 1614265330;
const BODY = '{"test": 2432232314}';
const SIG = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";
const b64 = (n: number, fill = 0) => Buffer.alloc(n, fill).toString("base64");

describe("standard webhooks", () => {
  it("matches the reference vector", () => {
    const key = parseSecret(SECRET);
    expect(sign(key, MSG_ID, TS, BODY)).toBe(SIG);
    expect(() => verify(key, MSG_ID, String(TS), SIG, BODY, TS + 10)).not.toThrow();
  });

  it("agrees with the standardwebhooks reference library both ways", () => {
    const key = parseSecret(SECRET);
    const ts = Math.floor(Date.now() / 1000);
    const headers = signHeaders(key, MSG_ID, ts, BODY);
    expect(new Webhook(SECRET).verify(BODY, headers)).toEqual({ test: 2432232314 });
    const theirs = new Webhook(SECRET).sign(MSG_ID, new Date(ts * 1000), BODY);
    expect(() => verify(key, MSG_ID, String(ts), theirs, BODY)).not.toThrow();
    expect(verifyRequest(key, new Headers(headers), BODY)).toEqual({ msgId: MSG_ID, timestamp: ts });
  });

  it("rejects tamper, expiry and bad timestamps; accepts a rotation list", () => {
    const key = parseSecret(SECRET);
    const kind = (f: () => void) => {
      try {
        f();
        return "ok";
      } catch (e) {
        return (e as VerifyError).kind;
      }
    };
    expect(kind(() => verify(key, MSG_ID, String(TS), SIG, "{}", TS))).toBe("no_matching_signature");
    expect(kind(() => verify(key, MSG_ID, String(TS), SIG, BODY, TS + 301))).toBe("expired");
    expect(kind(() => verify(key, MSG_ID, "x", SIG, BODY, TS))).toBe("bad_timestamp");
    expect(kind(() => verify(key, MSG_ID, String(TS), `v1,AAAA ${SIG}`, BODY, TS))).toBe("ok");
  });

  it("enforces secret rules", () => {
    const kind = (s: string) => {
      try {
        return parseSecret(s).length;
      } catch (e) {
        return (e as SecretError).kind;
      }
    };
    expect(kind("abc")).toBe("missing_prefix");
    expect(kind("whsec_!!")).toBe("not_base64");
    expect(kind(`whsec_${b64(16)}`)).toBe("bad_length");
    expect(kind(`whsec_${b64(65)}`)).toBe("bad_length");
    expect(kind(`whsec_${b64(32, 7)}`)).toBe(32);
  });
});

describe("mcp events", () => {
  it("applies the callback URL rules", () => {
    for (const ok of ["https://receiver.example.com/cb/1", "https://chatgpt.com:443/x?y=1", "https://[2606:4700::1]/x", "https://8.8.8.8/x"]) {
      expect(() => validateCallbackUrl(ok), ok).not.toThrow();
    }
    for (const bad of [
      "http://example.com/x",
      "https://localhost/x",
      "https://a.localhost/x",
      "https://printer.local/x",
      "https://127.0.0.1/x",
      "https://10.1.2.3/x",
      "https://192.168.1.1:8443/x",
      "https://169.254.169.254/latest",
      "https://100.64.0.3/x",
      "https://[::1]/x",
      "https://[fd00::1]/x",
      "https://[fe80::1]/x",
      "https://[::ffff:10.0.0.1]/x",
      "https://user:pw@example.com/x",
      "https:///x",
    ]) {
      expect(() => validateCallbackUrl(bad), bad).toThrow();
    }
    expect(isPublicIp("::ffff:8.8.8.8")).toBe(true);
    expect(isPublicIp("2001:db8::1")).toBe(false);
  });

  it("validates the webhook delivery half", () => {
    const secret = `whsec_${b64(32, 1)}`;
    const v = validateWebhook({ mode: "webhook", url: "https://cb.example.com/1", secret }, true);
    expect(v.url).toBe("https://cb.example.com/1");
    expect(v.key?.length).toBe(32);
    expect(() => validateWebhook({ mode: "webhook", url: "https://cb.example.com/1" }, true)).toThrow(/secret is required/);
    expect(validateWebhook({ mode: "webhook", url: "https://cb.example.com/1" }, false).key).toBeUndefined();
    expect(() => validateWebhook({ mode: "poll" }, false)).toThrow(/only webhook/);
  });

  it("derives stable, key-order-free subscription ids", () => {
    const a = subscriptionId("u1", "https://x/cb", "n", { a: 1, b: { d: 2, c: 3 } });
    const b = subscriptionId("u1", "https://x/cb", "n", { b: { c: 3, d: 2 }, a: 1 });
    expect(a).toBe(b);
    expect(a).toMatch(/^sub_[0-9a-f]{24}$/);
    expect(a).not.toBe(subscriptionId("u2", "https://x/cb", "n", { a: 1, b: { d: 2, c: 3 } }));
    expect(subscriptionId("ab", "c", "n", {})).not.toBe(subscriptionId("a", "bc", "n", {}));
    expect(canonicalJson({ b: [{ z: 1, y: 2 }], a: null })).toBe('{"a":null,"b":[{"y":2,"z":1}]}');
  });

  it("builds envelopes and exposes the error codes", () => {
    expect(eventEnvelope("e1", "approval.requested", new Date(0), { x: 1 })).toEqual({
      eventId: "e1",
      name: "approval.requested",
      timestamp: "1970-01-01T00:00:00.000Z",
      data: { x: 1 },
      cursor: null,
    });
    expect(verificationEnvelope("c")).toEqual({ type: "verification", challenge: "c" });
    expect(gapEnvelope("k")).toEqual({ type: "gap", cursor: "k" });
    expect(terminatedEnvelope("sub_1", EventsErrorCode.Forbidden, "revoked")).toEqual({
      type: "terminated",
      subscriptionId: "sub_1",
      error: { code: -32012, message: "revoked" },
    });
    expect(Object.values(EventsErrorCode)).toEqual([-32011, -32012, -32013, -32014, -32015]);
  });
});

describe("deliverWebhook", () => {
  it("signs the exact body it sends", async () => {
    const { deliverWebhook, verifyRequest: v } = await import("../src/index.js");
    const key = parseSecret(SECRET);
    let seen: { url: string; init: RequestInit } | undefined;
    const fake = (async (url: string, init: RequestInit) => {
      seen = { url, init };
      return new Response(null, { status: 204 });
    }) as unknown as typeof fetch;
    const res = await deliverWebhook({ url: "https://cb.example.com/1", key, subscriptionId: "sub_1", msgId: "e1", body: gapEnvelope("c2"), fetch: fake });
    expect(res.status).toBe(204);
    const headers = new Headers(seen!.init.headers as Record<string, string>);
    expect(headers.get("x-mcp-subscription-id")).toBe("sub_1");
    expect(v(key, headers, seen!.init.body as string).msgId).toBe("e1");
    await expect(deliverWebhook({ url: "https://127.0.0.1/x", key, subscriptionId: "s", msgId: "m", body: gapEnvelope("c"), fetch: fake })).rejects.toThrow(/private/);
  });
});
