// D3 — structural local-only guarantee: the daemon refuses to boot without
// the local macOS Keychain. Node has no keychain API, so the real backend
// shells out to /usr/bin/security via execFileSync.
// D15 note: on the Windows Sessions image a DPAPI-backed store fills this role.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";

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
