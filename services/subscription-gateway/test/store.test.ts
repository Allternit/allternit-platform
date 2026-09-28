import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { openDatabase, runMigrations, type Db } from "../src/store/db.js";
import {
  appendEvent,
  getAccount,
  getTask,
  insertAttempt,
  insertTask,
  listAttempts,
  listEvents,
  outboxAck,
  outboxEnqueue,
  outboxFetchUndelivered,
  outboxMarkDelivered,
  updateTaskStatus,
  upsertAccount,
} from "../src/store/queries.js";
import type { Account, Task } from "@allternit/subscription-fabric-contracts";

const MIGRATIONS_DIR = fileURLToPath(
  new URL("../src/store/migrations/", import.meta.url)
);

const EXPECTED_TABLES = [
  "accounts",
  "quota_pools",
  "tasks",
  "task_attempts",
  "artifacts",
  "thread_mappings",
  "events",
  "caller_outbox",
  "adapter_stats",
  "tokens",
  "quota_pool_meta",
  "adapter_breakers",
  "route_rejections",
  "migrations",
];

function freshDb(): Db {
  return openDatabase(":memory:", { migrationsDir: MIGRATIONS_DIR });
}

function tableNames(db: Db): string[] {
  return (
    db
      .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
      .all() as { name: string }[]
  ).map((r) => r.name);
}

function sampleTask(overrides: Partial<Task> = {}): Task {
  const now = new Date().toISOString();
  return {
    task_id: "task-1",
    idempotency_key: "idem-1",
    capability: "chat.create",
    capability_version: 1,
    requester: { kind: "bot", id: "caller-a", bot_id: "bot-1" },
    thread_id: null,
    project_id: null,
    parent_task_id: null,
    prompt: "hello",
    inputs: [{ type: "text", name: "note", content: "hi" }],
    options: { model_class: "standard" },
    routing: {
      mode: "auto",
      allow_fallback: true,
      allow_metered: false,
      allow_thread_migration: false,
    },
    constraints: {
      sensitivity: "normal",
      deadline_at: null,
      max_metered_usd: null,
      required_export_format: null,
    },
    approval_id: null,
    priority: "normal",
    status: "queued",
    status_detail: null,
    route_decision: null,
    attempts: [],
    result: null,
    error: null,
    created_at: now,
    updated_at: now,
    completed_at: null,
    ...overrides,
  } as Task;
}

describe("migrations", () => {
  it("fresh boot creates the full schema", () => {
    const db = freshDb();
    for (const t of EXPECTED_TABLES) {
      expect(tableNames(db)).toContain(t);
    }
  });

  it("is idempotent — running twice is a no-op", () => {
    const db = freshDb();
    runMigrations(db, MIGRATIONS_DIR);
    const versions = db
      .prepare("SELECT version FROM migrations ORDER BY version")
      .all() as { version: number }[];
    expect(versions).toEqual([{ version: 1 }, { version: 2 }, { version: 3 }, { version: 4 }]);
    // a task inserted before re-running migrations survives
    insertTask(db, sampleTask());
    runMigrations(db, MIGRATIONS_DIR);
    expect(getTask(db, "task-1")?.task_id).toBe("task-1");
  });

  it("enables WAL and foreign keys on a file-backed database", () => {
    // :memory: databases cannot use WAL — check pragmas against a real file.
    const dir = mkdtempSync(join(tmpdir(), "subs-gateway-test-"));
    const db = openDatabase(join(dir, "state.db"), {
      migrationsDir: MIGRATIONS_DIR,
    });
    expect(
      (db.pragma("journal_mode") as { journal_mode: string }[])[0].journal_mode
    ).toBe("wal");
    expect(
      (db.pragma("foreign_keys") as { foreign_keys: number }[])[0].foreign_keys
    ).toBe(1);
    db.close();
    rmSync(dir, { recursive: true, force: true });
  });
});

describe("task queries", () => {
  it("insert/get round-trips a task", () => {
    const db = freshDb();
    const task = sampleTask();
    insertTask(db, task);
    expect(getTask(db, "task-1")).toEqual(task);
    expect(getTask(db, "missing")).toBeNull();
  });

  it("updates status with detail and completion time", () => {
    const db = freshDb();
    insertTask(db, sampleTask());
    const done = new Date().toISOString();
    updateTaskStatus(db, "task-1", "completed", {
      statusDetail: "done",
      completedAt: done,
    });
    const after = getTask(db, "task-1")!;
    expect(after.status).toBe("completed");
    expect(after.status_detail).toBe("done");
    expect(after.completed_at).toBe(done);
    expect(after.updated_at > after.created_at || after.updated_at >= after.created_at).toBe(true);
  });

  it("round-trips attempts through getTask", () => {
    const db = freshDb();
    insertTask(db, sampleTask());
    insertAttempt(db, "task-1", {
      attempt_no: 1,
      adapter_id: "adapter-x",
      adapter_version: "1",
      account_id: "acct-1",
      pool_key: "acct-1:chat",
      submission_state: "acknowledged",
      prompt_fingerprint: "fp-1",
      provider_thread_id: "thread-9",
      requested_model_class: "standard",
      observed_model: "model-a",
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "success",
      error: null,
    });
    expect(listAttempts(db, "task-1")).toHaveLength(1);
    expect(getTask(db, "task-1")!.attempts[0].submission_state).toBe(
      "acknowledged"
    );
  });
});

describe("event ledger", () => {
  it("assigns monotonic seq per task", () => {
    const db = freshDb();
    const a = appendEvent(db, { taskId: "t1", callerId: "c1", kind: "progress", payload: { label: "a" } });
    const b = appendEvent(db, { taskId: "t1", callerId: "c1", kind: "progress", payload: { label: "b" } });
    const other = appendEvent(db, { taskId: "t2", callerId: "c1", kind: "progress", payload: { label: "x" } });
    expect([a.seq, b.seq]).toEqual([1, 2]);
    expect(other.seq).toBe(1);
    expect(listEvents(db, "t1").map((e) => e.kind)).toEqual(["progress", "progress"]);
    expect(listEvents(db, "t1")[0].payload).toEqual({ label: "a" });
  });
});

describe("caller outbox (D12)", () => {
  it("enqueue → undelivered (ordered) → deliver → ack", () => {
    const db = freshDb();
    const e1 = appendEvent(db, { taskId: "t1", callerId: "owner", kind: "progress", payload: { n: 1 } });
    const e2 = appendEvent(db, { taskId: "t1", callerId: "owner", kind: "done", payload: { n: 2 } });
    const e3 = appendEvent(db, { taskId: "t2", callerId: "owner", kind: "done", payload: { n: 3 } });

    outboxEnqueue(db, e1.event_id, ["bot-1"]);
    outboxEnqueue(db, e2.event_id, ["bot-1"]);
    outboxEnqueue(db, e3.event_id, ["bot-1", "bot-2"]);

    // undelivered comes back in ledger insertion order, per caller
    expect(outboxFetchUndelivered(db, "bot-1").map((e) => e.event_id)).toEqual([
      e1.event_id,
      e2.event_id,
      e3.event_id,
    ]);
    expect(outboxFetchUndelivered(db, "bot-2").map((e) => e.event_id)).toEqual([
      e3.event_id,
    ]);

    // replay is idempotent on event_id — re-enqueue changes nothing
    outboxEnqueue(db, e1.event_id, ["bot-1"]);
    expect(outboxFetchUndelivered(db, "bot-1")).toHaveLength(3);

    outboxMarkDelivered(db, e1.event_id, "bot-1");
    expect(outboxFetchUndelivered(db, "bot-1").map((e) => e.event_id)).toEqual([
      e2.event_id,
      e3.event_id,
    ]);

    outboxAck(db, e2.event_id, "bot-1");
    const row = db
      .prepare(
        "SELECT delivered_at, acked_at FROM caller_outbox WHERE event_id = ? AND caller_id = ?"
      )
      .get(e2.event_id, "bot-1") as { delivered_at: string | null; acked_at: string | null };
    expect(row.delivered_at).not.toBeNull();
    expect(row.acked_at).not.toBeNull();
    // acked events are already delivered, so they leave the undelivered set
    expect(outboxFetchUndelivered(db, "bot-1").map((e) => e.event_id)).toEqual([
      e3.event_id,
    ]);
  });
});

describe("account queries", () => {
  it("upsert/get round-trips, and upsert updates", () => {
    const db = freshDb();
    const account: Account = {
      account_id: "acct-1",
      provider: "provider-x",
      label: "Personal",
      plan: "plus",
      plan_observed_at: null,
      profile_ref: "profiles/acct-1",
      session_health: "ready",
      enabled: true,
    };
    upsertAccount(db, account);
    expect(getAccount(db, "acct-1")).toEqual(account);

    upsertAccount(db, { ...account, session_health: "degraded", enabled: false });
    const after = getAccount(db, "acct-1")!;
    expect(after.session_health).toBe("degraded");
    expect(after.enabled).toBe(false);
    expect(getAccount(db, "missing")).toBeNull();
  });
});
