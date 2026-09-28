import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { statSync } from "node:fs";
import { join } from "node:path";
import { boot, type RunningGateway } from "../src/main.js";
import { KeychainUnavailable, type KeychainBackend } from "../src/security/keychain.js";
import { openDatabase } from "../src/store/db.js";
import { getTask, insertAttempt, insertTask, updateTaskStatus } from "../src/store/queries.js";
import {
  cleanupDir,
  fakeKeychain,
  sampleTask,
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

  it("settles tasks the previous process left in flight before serving (no lane traffic needed)", async () => {
    // A prior process died mid-reply: task streaming, attempt acknowledged,
    // plus one that died between the two markSubmitted writes.
    const prior = openDatabase(join(dir, "state.db"));
    for (const [taskId, state] of [
      ["task-boot-ack", "acknowledged"],
      ["task-boot-unconfirmed", "sent_unconfirmed"],
    ] as const) {
      insertTask(prior, sampleTask({ task_id: taskId, status: "running" }));
      insertAttempt(prior, taskId, {
        attempt_no: 1,
        adapter_id: "chatgpt-web",
        adapter_version: "0.1.0",
        account_id: "acct-boot",
        pool_key: "chatgpt:acct-boot:chat-msgs",
        submission_state: state,
        prompt_fingerprint: "fp",
        provider_thread_id: state === "acknowledged" ? "thread-boot" : null,
        requested_model_class: null,
        observed_model: null,
        started_at: new Date().toISOString(),
        ended_at: null,
        outcome: "failed",
        error: null,
      });
    }
    updateTaskStatus(prior, "task-boot-ack", "streaming");
    prior.close();

    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir },
      keychain: fakeKeychain(),
      logger: () => {},
    });
    const ack = getTask(gateway.db, "task-boot-ack");
    expect(ack?.status).toBe("failed");
    expect(ack?.error?.class).toBe("stalled");
    expect(ack?.error?.retryable).toBe(false);
    expect(ack?.attempts[0].ended_at).not.toBeNull();
    // No browser at boot → never resubmitted, handed to the user.
    const unconfirmed = getTask(gateway.db, "task-boot-unconfirmed");
    expect(unconfirmed?.status).toBe("needs_user");
    expect(unconfirmed?.attempts[0].ended_at).not.toBeNull();
    // Nothing launched to do it.
    expect(gateway.pool.runtimeFor({ provider: "chatgpt", account_id: "acct-boot" })).toBeNull();
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
