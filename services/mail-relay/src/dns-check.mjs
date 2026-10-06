// The DNS records a customer adds for their domain, and a live check of each.

import { Resolver } from "node:dns/promises";

/** Records the customer adds at their DNS provider. `host` is the full name. */
export function requiredRecords(entry, mxHost) {
  return [
    { type: "MX", host: entry.host, value: `10 ${mxHost}`, purpose: "Receives the bots' mail" },
    { type: "TXT", host: entry.host, value: `v=spf1 a:${mxHost} ~all`, purpose: "Allows Allternit to send as this domain" },
    { type: "TXT", host: `${entry.selector}._domainkey.${entry.host}`, value: `v=DKIM1; k=rsa; p=${entry.dkimPublicKey}`, purpose: "Signs the bots' mail" },
    { type: "TXT", host: `_allternit.${entry.host}`, value: `allternit-verify=${entry.verifyToken}`, purpose: "Proves you own the domain" },
  ];
}

const flat = (txt) => txt.map((chunks) => chunks.join(""));

/** Whether one record is in place. An SPF record that merges ours into an existing one counts. */
export async function checkRecord(resolver, record, mxHost) {
  try {
    if (record.type === "MX") {
      const mx = await resolver.resolveMx(record.host);
      return mx.some((r) => r.exchange.replace(/\.$/, "").toLowerCase() === mxHost);
    }
    const txt = flat(await resolver.resolveTxt(record.host));
    if (record.value.startsWith("v=spf1")) {
      const spf = txt.filter((t) => t.toLowerCase().startsWith("v=spf1"));
      return spf.length === 1 && spf[0].toLowerCase().includes(`a:${mxHost}`);
    }
    if (record.value.startsWith("v=DKIM1")) {
      const want = record.value.split("p=")[1];
      return txt.some((t) => t.replace(/\s+/g, "").includes(`p=${want}`));
    }
    return txt.includes(record.value);
  } catch {
    return false;
  }
}

export function makeResolver(servers = ["1.1.1.1", "8.8.8.8"]) {
  const resolver = new Resolver({ timeout: 4000, tries: 2 });
  resolver.setServers(servers);
  return resolver;
}

export async function checkDomain(resolver, entry, mxHost) {
  const records = requiredRecords(entry, mxHost);
  const results = await Promise.all(records.map(async (r) => ({ ...r, ok: await checkRecord(resolver, r, mxHost) })));
  return { records: results, verified: results.every((r) => r.ok) };
}
