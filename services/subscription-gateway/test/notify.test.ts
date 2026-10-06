import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { insertTask } from "../src/store/queries.js";
import {
  cleanupDir,
  makeDeps,
  sampleTask,
  tmpStateDir,
  type TestDeps,
} from "./helpers.js";

interface FetchCall {
  url: string;
  body: { body: string; from?: string };
}

function fetchRecorder(fail = false): { calls: FetchCall[]; impl: typeof fetch } {
  const calls: FetchCall[] = [];
  const impl = (async (url: string | URL, init?: { body?: string }) => {
    calls.push({ url: String(url), body: JSON.parse(init?.body ?? "{}") });
    if (fail) throw new Error("connection refused");
    return { ok: true, status: 200 } as Response;
  }) as unknown as typeof fetch;
  return { calls, impl };
}

describe("terminal-state notify (D12)", () => {
  let dir: string;
  let deps: TestDeps;
  let recorder: ReturnType<typeof fetchRecorder>;

  beforeEach(() => {
    dir = tmpStateDir();
    recorder = fetchRecorder();
    deps = makeDeps(dir, { fetchImpl: recorder.impl });
  });

  afterEach(() => {
    deps.cleanup();
    cleanupDir(dir);
  });

  it("bot requester → Factory peer message on completed", async () => {
    insertTask(deps.db, sampleTask({ task_id: "t-notify" }));
    deps.log.append({
      task_id: "t-notify",
      kind: "task.status",
      payload: { status: "completed" },
      callers: ["bot-1"],
    });
    await deps.notifier.drain();

    expect(recorder.calls).toHaveLength(1);
    expect(recorder.calls[0].url).toBe(
      "http://127.0.0.1:18013/api/factory/peers/bot-1/send"
    );
    expect(recorder.calls[0].body.from).toBe("subscription-gateway");
    expect(recorder.calls[0].body.body).toContain("t-notify");
    expect(recorder.calls[0].body.body).toContain("completed");
  });

  it("non-terminal statuses do not notify", async () => {
    insertTask(deps.db, sampleTask({ task_id: "t-live" }));
    deps.log.append({
      task_id: "t-live",
      kind: "task.status",
      payload: { status: "streaming" },
      callers: ["bot-1"],
    });
    await deps.notifier.drain();
    expect(recorder.calls).toHaveLength(0);
  });

  it("user requester → desktop drop file, no Factory peer call", async () => {
    insertTask(
      deps.db,
      sampleTask({ task_id: "t-user", requester: { kind: "user", id: "user-1" } })
    );
    deps.log.append({
      task_id: "t-user",
      kind: "task.status",
      payload: { status: "needs_user" },
      callers: ["user-1"],
    });
    await deps.notifier.drain();

    expect(recorder.calls).toHaveLength(0);
    const drops = readdirSync(join(dir, "notifications"));
    expect(drops).toHaveLength(1);
    const drop = JSON.parse(
      readFileSync(join(dir, "notifications", drops[0]), "utf8")
    );
    expect(drop.task_id).toBe("t-user");
    expect(drop.status).toBe("needs_user");
  });

  it("notify failure lands as notify.failed in the ledger, never throws", async () => {
    deps.cleanup();
    cleanupDir(dir);
    dir = tmpStateDir();
    recorder = fetchRecorder(true); // every send fails
    deps = makeDeps(dir, { fetchImpl: recorder.impl });

    insertTask(deps.db, sampleTask({ task_id: "t-fail" }));
    deps.log.append({
      task_id: "t-fail",
      kind: "task.status",
      payload: { status: "completed" },
      callers: ["bot-1"],
    });
    await deps.notifier.drain();

    const rows = deps.db
      .prepare("SELECT kind, payload FROM events WHERE kind = 'notify.failed'")
      .all() as { kind: string; payload: string }[];
    expect(rows).toHaveLength(1);
    const payload = JSON.parse(rows[0].payload);
    expect(payload.caller_id).toBe("bot-1");
    expect(payload.error).toContain("connection refused");
  });
});
