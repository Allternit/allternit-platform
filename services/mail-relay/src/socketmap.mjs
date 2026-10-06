// Postfix socketmap lookups (http://www.postfix.org/socketmap_table.5.html).
// Postfix asks "<map> <key>" as a netstring and gets "OK <value>",
// "NOTFOUND " or "TEMP <reason>" back. Live lookups mean adding a customer
// domain needs no Postfix reload.
//
// Maps:
//   relay_domain <domain>   → OK when the domain is added and verified
//   recipient <address>     → OK when the agent mail worker has that mailbox
//   dkim_from <domain>      → OK when outgoing mail from it may be signed

import { createServer } from "node:net";

export function encodeNetstring(text) {
  const body = Buffer.from(text, "utf8");
  return Buffer.concat([Buffer.from(`${body.length}:`), body, Buffer.from(",")]);
}

/** Splits complete netstrings off the front of `buf`; returns them and the rest. */
export function decodeNetstrings(buf) {
  const out = [];
  let rest = buf;
  for (;;) {
    const colon = rest.indexOf(0x3a);
    if (colon <= 0 || colon > 10) break;
    const len = Number(rest.subarray(0, colon).toString("ascii"));
    if (!Number.isInteger(len) || len < 0 || len > 100_000) throw new Error("bad netstring length");
    const end = colon + 1 + len;
    if (rest.length < end + 1) break;
    if (rest[end] !== 0x2c) throw new Error("netstring missing comma");
    out.push(rest.subarray(colon + 1, end).toString("utf8"));
    rest = rest.subarray(end + 1);
  }
  return { messages: out, rest };
}

/** `lookup(map, key)` resolves to a value string (found), `null` (not found) or throws (temporary). */
export function startSocketmap({ host, port, lookup }) {
  const server = createServer((sock) => {
    let pending = Buffer.alloc(0);
    sock.on("data", async (chunk) => {
      pending = Buffer.concat([pending, chunk]);
      let parsed;
      try {
        parsed = decodeNetstrings(pending);
      } catch {
        sock.destroy();
        return;
      }
      pending = parsed.rest;
      for (const msg of parsed.messages) {
        const space = msg.indexOf(" ");
        const map = space > 0 ? msg.slice(0, space) : msg;
        const key = space > 0 ? msg.slice(space + 1) : "";
        let reply;
        try {
          const value = await lookup(map, key);
          reply = value == null ? "NOTFOUND " : `OK ${value}`;
        } catch (err) {
          reply = `TEMP ${String(err?.message ?? err).slice(0, 200)}`;
        }
        if (!sock.destroyed) sock.write(encodeNetstring(reply));
      }
    });
    sock.on("error", () => {});
  });
  return new Promise((resolve) => server.listen(port, host, () => resolve(server)));
}
