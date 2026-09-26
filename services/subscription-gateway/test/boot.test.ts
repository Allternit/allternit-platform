import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { statSync } from "node:fs";
import { boot, type RunningGateway } from "../src/main.js";
import { KeychainUnavailable, type KeychainBackend } from "../src/security/keychain.js";
import {
  cleanupDir,
  fakeKeychain,
  tmpStateDir,
  udsRequest,
} from "./helpers.js";

let dir: string;
let gateway: RunningGateway | null = null;

beforeEach(() => {
  dir = tmpStateDir();
});

afterEach(async () => {
  if (gateway) await gateway.close();
  gateway = null;
  cleanupDir(dir);
});

describe("boot", () => {
  it("refuses to start without the local keychain (D3)", async () => {
    const down: KeychainBackend = {
      available: () => {
        throw new KeychainUnavailable("keychain cli missing");
      },
      get: () => null,
      set: () => {},
    };
    const lines: string[] = [];
    let exitCode: number | null = null;
    await expect(
      boot({
        env: { SUBS_GATEWAY_STATE_DIR: dir },
        keychain: down,
        logger: (l) => lines.push(l),
        exit: (code) => {
          exitCode = code;
        },
      })
    ).rejects.toThrow(KeychainUnavailable);
    expect(exitCode).toBe(1);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("subscription-gateway");
  });

  it("boots on UDS with a working keychain and serves health", async () => {
    const lines: string[] = [];
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir },
      keychain: fakeKeychain(),
      logger: (l) => lines.push(l),
    });
    expect(statSync(gateway.config.udsPath).mode & 0o777).toBe(0o600);
    expect(lines.some((l) => l.includes(`unix:${gateway!.config.udsPath}`))).toBe(true);

    const res = await udsRequest(gateway.config.udsPath, { path: "/v1/health" });
    expect(res.status).toBe(200);
    expect((res.body as { ok: boolean; name: string }).name).toBe("subscription-gateway");

    await gateway.close();
    gateway = null; // closed already; afterEach should not double-close
  });

  it("TCP stays off unless SUBS_GATEWAY_TCP=1", async () => {
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir },
      keychain: fakeKeychain(),
      logger: () => {},
    });
    expect(gateway.servers).toHaveLength(1);
  });

  it("wires the worker layer (pool + supervisor + drain) without launching anything at boot", async () => {
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir },
      keychain: fakeKeychain(),
      logger: () => {},
    });
    expect(typeof gateway.pool.runtimeFor).toBe("function");
    expect(typeof gateway.supervisor.isReady).toBe("function");
    // Activation is lazy: no account → no runtime, no browser.
    expect(gateway.pool.runtimeFor({ provider: "none", account_id: "none" })).toBeNull();
    // close() stops the drain and shuts the pool down cleanly.
    await gateway.close();
    gateway = null;
  });
});
