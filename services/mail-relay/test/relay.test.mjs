import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect } from "node:net";
import { createRelay, domainOf } from "../src/server.mjs";
import { decodeNetstrings, encodeNetstring, startSocketmap } from "../src/socketmap.mjs";
import { checkDomain, requiredRecords } from "../src/dns-check.mjs";
import { normalizeDomain } from "../src/store.mjs";

const SECRET = "s".repeat(40);
const MX = "mx.allternit.com";

function relayWith({ dns = {}, recipients = new Set(), sent = [] } = {}) {
  const resolver = {
    resolveMx: async (h) => (dns[`MX ${h}`] ?? []).map((exchange) => ({ exchange, priority: 10 })),
    resolveTxt: async (h) => (dns[`TXT ${h}`] ?? []).map((t) => [t]),
  };
  const fetchImpl = async (url, init = {}) => {
    if (url.includes("/api/v1/relay/recipient")) {
      const addr = new URL(url).searchParams.get("address");
      return new Response(JSON.stringify({ exists: recipients.has(addr) }), { status: 200 });
    }
    sent.push({ url, init });
    return new Response("{}", { status: 202 });
  };
  const mailer = { sendMail: async (m) => { sent.push({ mail: m }); return { messageId: "<x@acme.com>" }; } };
  return createRelay({ secret: SECRET, workerUrl: "https://mail.test", mxHost: MX, dataDir: mkdtempSync(join(tmpdir(), "relay-")), resolver, fetchImpl, mailer });
}

async function call(relay, method, path, body, auth = SECRET) {
  let status, payload;
  const req = (async function* () { if (body) yield Buffer.from(JSON.stringify(body)); })();
  Object.assign(req, { method, url: path, headers: { authorization: `Bearer ${auth}` } });
  const res = { writeHead: (s) => { status = s; }, end: (b) => { payload = JSON.parse(b); } };
  await relay.handleHttp(req, res);
  return { status, body: payload };
}

test("domains are normalised and reserved names refused", async () => {
  assert.equal(normalizeDomain(" Bots.Acme.COM. "), "bots.acme.com");
  assert.equal(normalizeDomain("not a domain"), null);
  assert.equal(normalizeDomain("-bad.com"), null);
  assert.equal(domainOf("Sam@Acme.com"), "acme.com");
  assert.equal(domainOf("Support <support@acme.com>"), "acme.com");
  const relay = relayWith();
  assert.equal((await call(relay, "PUT", "/relay/domains/bots.allternit.com")).body.error, "reserved_domain");
  assert.equal((await call(relay, "PUT", "/relay/domains/acme.com", null, "wrong")).status, 401);
});

test("adding a domain returns its four DNS records and no private key", async () => {
  const relay = relayWith();
  const { status, body } = await call(relay, "PUT", "/relay/domains/acme.com");
  assert.equal(status, 200);
  assert.equal(body.verified, false);
  assert.deepEqual(body.records.map((r) => `${r.type} ${r.host}`), ["MX acme.com", "TXT acme.com", "TXT allternit._domainkey.acme.com", "TXT _allternit.acme.com"]);
  assert.ok(!JSON.stringify(body).includes("PRIVATE KEY"));
  const again = await call(relay, "PUT", "/relay/domains/acme.com");
  assert.deepEqual(again.body.records, body.records, "adding twice keeps the same key");
});

test("the domain verifies only when every record is in place", async () => {
  const dns = {};
  const relay = relayWith({ dns });
  await call(relay, "PUT", "/relay/domains/acme.com");
  let check = await call(relay, "POST", "/relay/domains/acme.com/check");
  assert.equal(check.body.verified, false);
  const entry = relay.store.get("acme.com");
  for (const r of requiredRecords(entry, MX)) {
    if (r.type === "MX") dns[`MX ${r.host}`] = [`${MX}.`];
    else dns[`TXT ${r.host}`] = r.value.startsWith("v=spf1") ? ["v=spf1 include:_spf.google.com a:mx.allternit.com ~all"] : [r.value];
  }
  check = await call(relay, "POST", "/relay/domains/acme.com/check");
  assert.equal(check.body.verified, true, JSON.stringify(check.body.records));
  dns["TXT acme.com"] = ["v=spf1 a:mx.allternit.com ~all", "v=spf1 -all"];
  assert.equal((await checkDomain({ resolveMx: async () => [{ exchange: MX }], resolveTxt: async (h) => (dns[`TXT ${h}`] ?? []).map((t) => [t]) }, entry, MX)).verified, false, "two SPF records break SPF");
});

test("Postfix lookups accept only verified domains and existing mailboxes", async () => {
  const relay = relayWith({ recipients: new Set(["support@acme.com"]) });
  relay.store.add("acme.com");
  assert.equal(await relay.lookup("relay_domain", "acme.com"), null, "not verified yet");
  relay.store.setVerified("acme.com", true);
  assert.equal(await relay.lookup("relay_domain", "ACME.com"), "OK");
  assert.equal(await relay.lookup("recipient", "support@acme.com"), "OK");
  assert.equal(await relay.lookup("recipient", "nobody@acme.com"), null);
  assert.equal(await relay.lookup("recipient", "x@other.com"), null);
});

test("socketmap speaks Postfix netstrings", async () => {
  assert.deepEqual(decodeNetstrings(Buffer.from("5:hello,3:abc,2:x")).messages, ["hello", "abc"]);
  const server = await startSocketmap({ host: "127.0.0.1", port: 0, lookup: async (map, key) => (map === "relay_domain" && key === "acme.com" ? "OK" : null) });
  const { port } = server.address();
  const ask = (q) => new Promise((resolve) => {
    const s = connect(port, "127.0.0.1", () => s.write(encodeNetstring(q)));
    s.on("data", (d) => { resolve(decodeNetstrings(d).messages[0]); s.end(); });
  });
  assert.equal(await ask("relay_domain acme.com"), "OK OK");
  assert.equal(await ask("relay_domain other.com"), "NOTFOUND ");
  server.close();
});

test("sending needs a verified From domain and signs with its key", async () => {
  const sent = [];
  const relay = relayWith({ sent });
  relay.store.add("acme.com");
  const msg = { from: "Support <support@acme.com>", to: "a@b.com", subject: "Hi", text: "hello", attachments: [{ filename: "a.txt", type: "text/plain", content: Buffer.from("x").toString("base64") }] };
  assert.equal((await call(relay, "POST", "/relay/send", msg)).body.error, "domain_not_verified");
  relay.store.setVerified("acme.com", true);
  const ok = await call(relay, "POST", "/relay/send", msg);
  assert.equal(ok.status, 200);
  const mail = sent.at(-1).mail;
  assert.equal(mail.dkim.domainName, "acme.com");
  assert.equal(mail.dkim.keySelector, "allternit");
  assert.ok(mail.dkim.privateKey.includes("PRIVATE KEY"));
  assert.equal(mail.attachments[0].content.toString(), "x");
});
