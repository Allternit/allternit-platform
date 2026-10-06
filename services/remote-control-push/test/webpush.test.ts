// Run: pnpm test (node --test; Node 22.6+ strips the types).
import { test } from "node:test";
import assert from "node:assert/strict";
import { base64UrlToBytes, bytesToBase64Url, encryptWebPush, importServerKeys, sanitizeActions, sanitizeData } from "../src/webpush.ts";

// RFC 8291 Appendix A.
const V = {
  plaintext: "When I grow up, I want to be a watermelon",
  asPublic: "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
  asPrivate: "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw",
  uaPublic: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
  salt: "DGv6ra1nlYgDCS1FRnbzlw",
  auth: "BTBZMqHH6r4Tts7J_aSIgg",
  body:
    "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
};

test("matches the RFC 8291 test vector", async () => {
  const serverKeys = await importServerKeys(V.asPublic, V.asPrivate);
  const out = await encryptWebPush(new TextEncoder().encode(V.plaintext), V.uaPublic, V.auth, {
    salt: base64UrlToBytes(V.salt),
    serverKeys,
  });
  assert.equal(bytesToBase64Url(out), V.body);
});

test("fresh keys and salt per message; header carries rs 4096 and the server key", async () => {
  const a = await encryptWebPush(new TextEncoder().encode("hi"), V.uaPublic, V.auth);
  const b = await encryptWebPush(new TextEncoder().encode("hi"), V.uaPublic, V.auth);
  assert.notEqual(bytesToBase64Url(a), bytesToBase64Url(b));
  assert.equal(new DataView(a.buffer, a.byteOffset).getUint32(16), 4096);
  assert.equal(a[20], 65);
  assert.equal(a[21], 0x04);
  // header 86 + plaintext 2 + delimiter 1 + tag 16
  assert.equal(a.byteLength, 86 + 2 + 1 + 16);
});

test("rejects a bad subscription key and an oversized payload", async () => {
  await assert.rejects(encryptWebPush(new Uint8Array(1), "AAAA", V.auth), /p256dh/);
  await assert.rejects(encryptWebPush(new Uint8Array(5000), V.uaPublic, V.auth), /too large/);
});

test("actions: at most two short buttons", () => {
  assert.deepEqual(
    sanitizeActions([{ action: "approve", title: "Approve" }, { action: "open", title: "Open" }, { action: "x", title: "X" }]),
    [{ action: "approve", title: "Approve" }, { action: "open", title: "Open" }],
  );
  assert.equal(sanitizeActions("nope"), undefined);
  assert.equal(sanitizeActions([{ action: 1, title: "x" }]), undefined);
});

test("data: flat scalars only, bounded", () => {
  const data = { kind: "factory.approval", approvalId: "ap1", dagId: "d", nodeId: "n", code: "ABC234", actionUrl: "/api/factory/approvals/ap1/push-action", openUrl: "/factory/nodes/d/n", nested: { a: 1 } };
  const out = sanitizeData(data);
  assert.equal(out?.kind, "factory.approval");
  assert.equal(out?.code, "ABC234");
  assert.equal("nested" in (out ?? {}), false);
  assert.equal(sanitizeData({ big: "x".repeat(3000) }), undefined);
  assert.equal(sanitizeData([1]), undefined);
});
