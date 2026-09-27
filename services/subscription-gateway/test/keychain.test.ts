import { describe, expect, it } from "vitest";
import {
  getOrCreateMasterKey,
  KeychainUnavailable,
  requireKeychain,
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
