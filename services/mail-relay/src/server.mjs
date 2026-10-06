// allternit-mail-relay: bot email on customer domains (support@acme.com).
//
// Runs next to Postfix on the mail host (mx.allternit.com):
//   - socketmap  127.0.0.1:2526  Postfix asks which domains/recipients it accepts
//   - smtp       127.0.0.1:2525  Postfix hands accepted mail here; it goes to the
//                                agent mail worker (POST /api/v1/relay/inbound)
//   - http       127.0.0.1:8025  the worker adds domains, checks DNS and sends
//                                (nginx serves it as https://mx.allternit.com/relay/)
//
// Outgoing mail is DKIM-signed here with the domain's own key and queued in
// the local Postfix, which delivers and retries.
//
// Env: RELAY_SECRET (shared with the worker), WORKER_URL, MX_HOST
// (default mx.allternit.com), DATA_DIR (default /var/lib/allternit-mail-relay),
// POSTFIX_HOST/POSTFIX_PORT (default 127.0.0.1:25).

import { createServer } from "node:http";
import { timingSafeEqual } from "node:crypto";
import { SMTPServer } from "smtp-server";
import nodemailer from "nodemailer";
import { createStore, normalizeDomain } from "./store.mjs";
import { checkDomain, makeResolver, requiredRecords } from "./dns-check.mjs";
import { startSocketmap } from "./socketmap.mjs";

const MAX_MESSAGE_BYTES = 25 * 1024 * 1024;
const RECIPIENT_CACHE_MS = 60_000;

/** Domain of a bare address or a "Name <addr>" form. */
export function domainOf(address) {
  const raw = String(address ?? "");
  const addr = (raw.match(/<([^<>]+)>\s*$/)?.[1] ?? raw).trim();
  const at = addr.lastIndexOf("@");
  return at > 0 ? normalizeDomain(addr.slice(at + 1)) : null;
}

function bearerOk(header, secret) {
  const got = Buffer.from(String(header ?? "").replace(/^Bearer\s+/i, ""));
  const want = Buffer.from(secret);
  return got.length === want.length && timingSafeEqual(got, want);
}

async function readJson(req, limit) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw Object.assign(new Error("body too large"), { status: 413 });
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  } catch {
    throw Object.assign(new Error("invalid json"), { status: 400 });
  }
}

const send = (res, status, body) => {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
};

/** Public view of a domain: never the private key. */
function domainView(entry, mxHost, check) {
  return {
    domain: entry.host,
    verified: entry.verified,
    checkedAt: entry.checkedAt,
    records: check?.records ?? requiredRecords(entry, mxHost).map((r) => ({ ...r, ok: null })),
  };
}

export function createRelay({ secret, workerUrl, mxHost = "mx.allternit.com", dataDir, postfix = { host: "127.0.0.1", port: 25 }, resolver = makeResolver(), fetchImpl = fetch, mailer }) {
  if (!secret || secret.length < 32) throw new Error("RELAY_SECRET must be at least 32 characters");
  const store = createStore(dataDir);
  const recipientCache = new Map();
  const transport = mailer ?? nodemailer.createTransport({ host: postfix.host, port: postfix.port, secure: false, ignoreTLS: true, pool: true, maxConnections: 2 });

  /** Does the agent mail worker have this mailbox? Cached briefly; worker errors are temporary. */
  async function recipientExists(address) {
    const key = address.toLowerCase();
    const hit = recipientCache.get(key);
    if (hit && hit.until > Date.now()) return hit.exists;
    const res = await fetchImpl(`${workerUrl}/api/v1/relay/recipient?address=${encodeURIComponent(key)}`, { headers: { authorization: `Bearer ${secret}` } });
    if (!res.ok) throw new Error(`worker answered ${res.status}`);
    const exists = Boolean((await res.json()).exists);
    recipientCache.set(key, { exists, until: Date.now() + RECIPIENT_CACHE_MS });
    return exists;
  }

  async function lookup(map, key) {
    if (map === "relay_domain") {
      const entry = store.get(normalizeDomain(key) ?? "");
      return entry?.verified ? "OK" : null;
    }
    if (map === "recipient") {
      const host = domainOf(key);
      if (!host || !store.get(host)?.verified) return null;
      return (await recipientExists(key)) ? "OK" : null;
    }
    return null;
  }

  async function deliverInbound(from, to, raw) {
    const res = await fetchImpl(`${workerUrl}/api/v1/relay/inbound`, {
      method: "POST",
      headers: { authorization: `Bearer ${secret}`, "content-type": "message/rfc822", "x-relay-from": from, "x-relay-to": to },
      body: raw,
    });
    if (!res.ok) throw new Error(`worker answered ${res.status}`);
  }

  const smtp = new SMTPServer({
    name: mxHost,
    banner: "allternit mail relay",
    authOptional: true,
    disabledCommands: ["AUTH", "STARTTLS"],
    size: MAX_MESSAGE_BYTES,
    logger: false,
    onConnect(session, cb) {
      // Only the local Postfix hands mail in.
      if (!["127.0.0.1", "::1", "::ffff:127.0.0.1"].includes(session.remoteAddress)) return cb(new Error("local only"));
      cb();
    },
    onRcptTo(address, _session, cb) {
      const host = domainOf(address.address);
      if (!host || !store.get(host)?.verified) return cb(Object.assign(new Error("relay not permitted"), { responseCode: 550 }));
      cb();
    },
    onData(stream, session, cb) {
      const chunks = [];
      stream.on("data", (c) => chunks.push(c));
      stream.on("end", async () => {
        if (stream.sizeExceeded) return cb(Object.assign(new Error("message too large"), { responseCode: 552 }));
        const raw = Buffer.concat(chunks);
        const from = session.envelope.mailFrom ? session.envelope.mailFrom.address : "";
        try {
          for (const rcpt of session.envelope.rcptTo) await deliverInbound(from, rcpt.address, raw);
          cb();
        } catch (err) {
          // Postfix keeps the message and retries.
          cb(Object.assign(new Error(`try again later: ${err.message}`), { responseCode: 451 }));
        }
      });
    },
  });

  async function handleHttp(req, res) {
    const url = new URL(req.url, "http://relay");
    const path = url.pathname.replace(/^\/relay/, "");
    if (path === "/health") return send(res, 200, { ok: true, domains: store.list().length });
    if (!bearerOk(req.headers.authorization, secret)) return send(res, 401, { error: "unauthorized" });

    const domainMatch = path.match(/^\/domains\/([^/]+)(\/check)?$/);
    if (domainMatch) {
      const host = normalizeDomain(decodeURIComponent(domainMatch[1]));
      if (!host) return send(res, 400, { error: "invalid_domain" });
      if (host === mxHost || host.endsWith(".allternit.com") || host === "allternit.com") return send(res, 400, { error: "reserved_domain" });
      if (req.method === "PUT" && !domainMatch[2]) return send(res, 200, domainView(store.add(host), mxHost));
      const entry = store.get(host);
      if (!entry) return send(res, 404, { error: "domain_not_found" });
      if (req.method === "GET" && !domainMatch[2]) return send(res, 200, domainView(entry, mxHost));
      if (req.method === "POST" && domainMatch[2]) {
        const check = await checkDomain(resolver, entry, mxHost);
        store.setVerified(host, check.verified);
        return send(res, 200, domainView(store.get(host), mxHost, check));
      }
      if (req.method === "DELETE" && !domainMatch[2]) {
        store.remove(host);
        return send(res, 200, { ok: true });
      }
      return send(res, 405, { error: "method_not_allowed" });
    }

    if (path === "/send" && req.method === "POST") {
      const m = await readJson(req, Math.ceil(MAX_MESSAGE_BYTES * 1.4));
      const host = domainOf(m.from);
      const entry = host ? store.get(host) : null;
      if (!entry?.verified) return send(res, 403, { error: "domain_not_verified" });
      if (!m.to || !m.subject) return send(res, 400, { error: "missing_fields" });
      const info = await transport.sendMail({
        from: m.from,
        to: m.to,
        subject: m.subject,
        text: m.text,
        html: m.html,
        headers: m.headers ?? {},
        attachments: (m.attachments ?? []).map((a) => ({ filename: a.filename, contentType: a.type, content: Buffer.from(a.content, "base64"), cid: a.contentId ?? undefined, contentDisposition: a.disposition })),
        dkim: { domainName: entry.host, keySelector: entry.selector, privateKey: entry.dkimPrivateKey },
      });
      return send(res, 200, { messageId: info.messageId });
    }
    return send(res, 404, { error: "not_found" });
  }

  const http = createServer((req, res) => {
    handleHttp(req, res).catch((err) => send(res, err.status ?? 500, { error: err.status ? err.message : "relay_error" }));
  });

  return { store, smtp, http, lookup, recipientExists, handleHttp };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const relay = createRelay({
    secret: process.env.RELAY_SECRET ?? "",
    workerUrl: (process.env.WORKER_URL ?? "").replace(/\/$/, ""),
    mxHost: process.env.MX_HOST ?? "mx.allternit.com",
    dataDir: process.env.DATA_DIR ?? "/var/lib/allternit-mail-relay",
    postfix: { host: process.env.POSTFIX_HOST ?? "127.0.0.1", port: Number(process.env.POSTFIX_PORT ?? 25) },
  });
  if (!process.env.WORKER_URL) throw new Error("WORKER_URL is required");
  await startSocketmap({ host: "127.0.0.1", port: 2526, lookup: relay.lookup });
  relay.smtp.listen(2525, "127.0.0.1");
  relay.http.listen(8025, "127.0.0.1");
  console.log("allternit-mail-relay: socketmap :2526, smtp :2525, http :8025");
}
