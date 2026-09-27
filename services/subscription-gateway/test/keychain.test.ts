import { existsSync, mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { loadConfig } from "../src/config.js";
import {
  createFileKeychainBackend,
  getOrCreateMasterKey,
  KeychainUnavailable,
  requireKeychain,
  selectKeychainBackend,
  type KeychainBackend,
} from "../src/security/keychain.js";

function fakeBackend(initial: Record<string, string> = {}): KeychainBackend & {
  items: Record<string, string>;
  setCalls: number;
} {
  const state = { items: { ...initial }, setCalls: 0 };
  return {
    items: state.items,
    get setCalls() {
      return state.setCalls;
    },
    available: () => true,
    get: (account) => state.items[account] ?? null,
    set: (account, value) => {
      state.setCalls += 1;
      state.items[account] = value;
    },
  };
}

describe("keychain boot gate (D3)", () => {
  it("refuses to boot when the backend is unavailable", () => {
    const down: KeychainBackend = {
      available: () => false,
      get: () => null,
      set: () => {},
    };
    expect(() => requireKeychain(down)).toThrow(KeychainUnavailable);
    expect(() => getOrCreateMasterKey(down)).toThrow(KeychainUnavailable);
  });

  it("propagates KeychainUnavailable thrown by the backend", () => {
    const throwing: KeychainBackend = {
      available: () => {
        throw new KeychainUnavailable("cli spawn failed");
      },
      get: () => null,
      set: () => {},
    };
    expect(() => requireKeychain(throwing)).toThrow(KeychainUnavailable);
  });

  it("working backend → master key is 32 bytes base64 and stable across calls", () => {
    const backend = fakeBackend();
    const k1 = getOrCreateMasterKey(backend);
    expect(Buffer.from(k1, "base64")).toHaveLength(32);
    const k2 = getOrCreateMasterKey(backend);
    expect(k2).toBe(k1);
    expect(backend.setCalls).toBe(1); // written once, then read
  });

  it("reuses a pre-existing master key", () => {
    const backend = fakeBackend({ "master-key": "cHJlLWV4aXN0aW5nLWtleQ==" });
    expect(getOrCreateMasterKey(backend)).toBe("cHJlLWV4aXN0aW5nLWtleQ==");
    expect(backend.setCalls).toBe(0);
  });
});

function tmpStateDir(): string {
  return mkdtempSync(join(tmpdir(), "sgw-keychain-"));
}

describe("file keychain backend (D15 Sessions-machine store)", () => {
  it("is available on any writable state dir — no macOS Keychain needed", () => {
    const backend = createFileKeychainBackend({ stateDir: tmpStateDir() });
    expect(backend.available()).toBe(true);
  });

  it("get on a missing store file is a miss (null), not an outage", () => {
    const backend = createFileKeychainBackend({ stateDir: tmpStateDir() });
    expect(backend.get("cli-token")).toBeNull();
  });

  it("set → get roundtrip, persisted as a 0600 JSON file, stable across instances", () => {
    const stateDir = tmpStateDir();
    const backend = createFileKeychainBackend({ stateDir });
    backend.set("cli-token", "sgw_test_123");
    expect(backend.get("cli-token")).toBe("sgw_test_123");
    const file = join(stateDir, "keychain.json");
    expect(statSync(file).mode & 0o777).toBe(0o600);
    expect(JSON.parse(readFileSync(file, "utf8"))["cli-token"]).toBe("sgw_test_123");
    // a fresh instance over the same dir sees the same secrets (restart-safe)
    const reopened = createFileKeychainBackend({ stateDir });
    expect(reopened.get("cli-token")).toBe("sgw_test_123");
  });

  it("a corrupt store file is an outage, never a silent clobber", () => {
    const stateDir = tmpStateDir();
    writeFileSync(join(stateDir, "keychain.json"), "{not json", { mode: 0o600 });
    const backend = createFileKeychainBackend({ stateDir });
    expect(() => backend.get("cli-token")).toThrow(KeychainUnavailable);
    expect(() => backend.set("cli-token", "x")).toThrow(KeychainUnavailable);
    expect(readFileSync(join(stateDir, "keychain.json"), "utf8")).toBe("{not json");
  });

  it("master-key flow works over the file backend", () => {
    const stateDir = tmpStateDir();
    const k1 = getOrCreateMasterKey(createFileKeychainBackend({ stateDir }));
    expect(Buffer.from(k1, "base64")).toHaveLength(32);
    const k2 = getOrCreateMasterKey(createFileKeychainBackend({ stateDir }));
    expect(k2).toBe(k1);
  });
});

describe("secret-store backend selection (D3/D15)", () => {
  it("selects the file backend when configured for it", () => {
    const stateDir = tmpStateDir();
    const backend = selectKeychainBackend({ kind: "file", stateDir });
    backend.set("a", "b");
    expect(backend.get("a")).toBe("b");
  });

  it("selects the macOS Keychain backend when configured for it", () => {
    const backend = selectKeychainBackend({ kind: "keychain", stateDir: tmpStateDir() });
    expect(backend.available()).toBe(existsSync("/usr/bin/security"));
  });

  it("config defaults to the keychain backend and parses SUBS_GATEWAY_KEYCHAIN", () => {
    const stateDir = tmpStateDir();
    expect(loadConfig({ SUBS_GATEWAY_STATE_DIR: stateDir }).keychainBackend).toBe("keychain");
    expect(
      loadConfig({ SUBS_GATEWAY_STATE_DIR: stateDir, SUBS_GATEWAY_KEYCHAIN: "file" })
        .keychainBackend
    ).toBe("file");
  });

  it("config rejects an unknown SUBS_GATEWAY_KEYCHAIN value loudly", () => {
    expect(() =>
      loadConfig({ SUBS_GATEWAY_STATE_DIR: tmpStateDir(), SUBS_GATEWAY_KEYCHAIN: "dpapi" })
    ).toThrow(/SUBS_GATEWAY_KEYCHAIN/);
  });
});
