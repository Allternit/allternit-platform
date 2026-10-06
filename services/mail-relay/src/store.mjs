// Customer domains and their DKIM keys, kept in one JSON file under DATA_DIR.
// The agent mail worker owns which user a domain belongs to; this store only
// holds what the mail server needs: is the domain allowed, is it verified, and
// the key that signs its mail.

import { generateKeyPairSync, randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export const DKIM_SELECTOR = "allternit";

const HOSTNAME_RE = /^(?=.{4,253}$)(?!-)([a-z0-9-]{1,63}\.)+[a-z]{2,63}$/;

/** Lower-cased, trailing dot removed; `null` when it isn't a plain domain name. */
export function normalizeDomain(input) {
  const host = String(input ?? "").trim().toLowerCase().replace(/\.$/, "");
  if (!HOSTNAME_RE.test(host)) return null;
  if (host.split(".").some((label) => label.startsWith("-") || label.endsWith("-"))) return null;
  return host;
}

export function createStore(dataDir) {
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  const file = join(dataDir, "domains.json");
  let domains = {};
  try {
    domains = JSON.parse(readFileSync(file, "utf8"));
  } catch {
    domains = {};
  }

  const save = () => {
    const tmp = `${file}.tmp`;
    writeFileSync(tmp, JSON.stringify(domains, null, 2), { mode: 0o600 });
    renameSync(tmp, file);
  };

  return {
    get(host) {
      return domains[host] ?? null;
    },
    list() {
      return Object.values(domains);
    },
    /** Adds the domain with a fresh DKIM key, or returns the existing entry unchanged. */
    add(host) {
      if (domains[host]) return domains[host];
      const { publicKey, privateKey } = generateKeyPairSync("rsa", {
        modulusLength: 2048,
        publicKeyEncoding: { type: "spki", format: "der" },
        privateKeyEncoding: { type: "pkcs8", format: "pem" },
      });
      domains[host] = {
        host,
        selector: DKIM_SELECTOR,
        dkimPublicKey: Buffer.from(publicKey).toString("base64"),
        dkimPrivateKey: privateKey,
        verifyToken: randomBytes(12).toString("hex"),
        verified: false,
        createdAt: new Date().toISOString(),
        checkedAt: null,
      };
      save();
      return domains[host];
    },
    setVerified(host, verified) {
      if (!domains[host]) return null;
      domains[host].verified = verified;
      domains[host].checkedAt = new Date().toISOString();
      save();
      return domains[host];
    },
    remove(host) {
      if (!domains[host]) return false;
      delete domains[host];
      save();
      return true;
    },
  };
}
