// D3 — structural local-only guarantee: the daemon refuses to boot without a
// local secret store. D3-refined (HARDENING D15, 2026-09-26): the boundary is
// single-tenant ownership + Allternit-provisioned containment, not physical
// locality — on an "Allternit Sessions" machine a keychain-equivalent store
// fills this role. Backends: macOS Keychain (Node has no keychain API, so the
// real backend shells out to /usr/bin/security via execFileSync) or a 0600
// file under the state dir (Sessions machines, selected via config).
// D15 note: on the Windows Sessions image a DPAPI-backed store fills this role.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  accessSync,
  chmodSync,
  constants,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

export const KEYCHAIN_SERVICE = "com.allternit.subscription-gateway";
const SECURITY_CLI = "/usr/bin/security";
const MASTER_KEY_ACCOUNT = "master-key";

export class KeychainUnavailable extends Error {
  override readonly name = "KeychainUnavailable";
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
  }
}

export interface KeychainBackend {
  available(): boolean;
  get(account: string): string | null; // null = no such item
  set(account: string, value: string): void;
}

function asUnavailable(err: unknown, action: string): KeychainUnavailable {
  return new KeychainUnavailable(
    `local keychain unavailable while trying to ${action} — refusing to run (D3)`,
    { cause: err }
  );
}

export function createMacOSKeychainBackend(): KeychainBackend {
  return {
    available(): boolean {
      return existsSync(SECURITY_CLI);
    },
    get(account: string): string | null {
      try {
        const out = execFileSync(
          SECURITY_CLI,
          ["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account, "-w"],
          { stdio: ["ignore", "pipe", "pipe"] }
        );
        return out.toString("utf8").replace(/\r?\n$/, "");
      } catch (err) {
        // 44 = errSecItemNotFound — a miss, not an outage.
        if (typeof err === "object" && err !== null && (err as { status?: unknown }).status === 44) {
          return null;
        }
        throw asUnavailable(err, `read keychain item "${account}"`);
      }
    },
    set(account: string, value: string): void {
      try {
        execFileSync(
          SECURITY_CLI,
          ["add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", account, "-w", value],
          { stdio: ["ignore", "ignore", "pipe"] }
        );
      } catch (err) {
        throw asUnavailable(err, `write keychain item "${account}"`);
      }
    },
  };
}

export type KeychainBackendKind = "keychain" | "file";

// File backend — the keychain-equivalent secret store for Sessions machines
// (D15: Linux/Windows guests have no macOS Keychain). Secrets live in a 0600
// JSON file under the state dir; writes are atomic (tmp + rename). A corrupt
// store is an outage, never a silent clobber.
// HONEST CAVEAT: values are plaintext at rest, protected by filesystem
// permissions only — encrypt-at-rest via the master key (§A6.3) is a
// deliberate follow-up, not shipped with this backend.
export function createFileKeychainBackend(opts: { stateDir: string }): KeychainBackend {
  const file = join(opts.stateDir, "keychain.json");
  const readStore = (): Record<string, string> => {
    if (!existsSync(file)) return {};
    let raw: string;
    try {
      raw = readFileSync(file, "utf8");
    } catch (err) {
      throw new KeychainUnavailable(
        `local secret store unavailable while trying to read ${file} — refusing to run (D3)`,
        { cause: err }
      );
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (err) {
      throw new KeychainUnavailable(
        `local secret store file ${file} is corrupt — refusing to run (D3)`,
        { cause: err }
      );
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new KeychainUnavailable(
        `local secret store file ${file} is corrupt — refusing to run (D3)`
      );
    }
    return parsed as Record<string, string>;
  };
  return {
    available(): boolean {
      try {
        mkdirSync(opts.stateDir, { recursive: true, mode: 0o700 });
        accessSync(opts.stateDir, constants.W_OK);
        return true;
      } catch {
        return false;
      }
    },
    get(account: string): string | null {
      const value = readStore()[account];
      return typeof value === "string" ? value : null;
    },
    set(account: string, value: string): void {
      const store = readStore();
      store[account] = value;
      const tmp = `${file}.${process.pid}.tmp`;
      try {
        writeFileSync(tmp, JSON.stringify(store, null, 2) + "\n", { mode: 0o600 });
        chmodSync(tmp, 0o600); // a stale tmp may pre-exist with a wider mode
        renameSync(tmp, file);
      } catch (err) {
        throw new KeychainUnavailable(
          `local secret store unavailable while trying to write item "${account}" — refusing to run (D3)`,
          { cause: err }
        );
      }
    },
  };
}

// Backend selection (D15): `file` on Sessions machines, `keychain` elsewhere.
// Selection only constructs; requireKeychain still enforces availability.
export function selectKeychainBackend(sel: {
  kind: KeychainBackendKind;
  stateDir: string;
}): KeychainBackend {
  return sel.kind === "file"
    ? createFileKeychainBackend({ stateDir: sel.stateDir })
    : createMacOSKeychainBackend();
}

export function createKeychain(backend?: KeychainBackend): KeychainBackend {
  return backend ?? createMacOSKeychainBackend();
}

// Boot gate (D3): main.ts calls this first and refuses to start on throw.
export function requireKeychain(backend?: KeychainBackend): KeychainBackend {
  const b = createKeychain(backend);
  if (!b.available()) {
    throw new KeychainUnavailable(
      "local keychain is not available on this machine — refusing to run (D3)"
    );
  }
  return b;
}

// 32 random bytes, base64 — future at-rest encryption key (§A6.3).
export function getOrCreateMasterKey(backend?: KeychainBackend): string {
  const b = requireKeychain(backend);
  const existing = b.get(MASTER_KEY_ACCOUNT);
  if (existing !== null) return existing;
  const key = randomBytes(32).toString("base64");
  b.set(MASTER_KEY_ACCOUNT, key);
  return key;
}
