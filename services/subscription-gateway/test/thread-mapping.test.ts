// §S6 / HARDENING #7 — fabric thread → provider thread mapping: a threaded chat
// turn records (or advances) the mapping; chat.continue resolves the provider
// thread, divergence fingerprint and owning account from it.
import { createHash } from "node:crypto";
import { join } from "node:path";
import request from "supertest";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { EventLog } from "../src/events/log.js";
import { SseHub } from "../src/events/sse.js";
import { issueToken } from "../src/security/tokens.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getActiveThreadMapping, getTask, insertTask, recordThreadTurn } from "../src/store/queries.js";
import { runAttempt } from "../src/worker/worker.js";
import { HUMAN,
  cleanupDir,
  dummyResolver,
  fakeLease,
  makeDeps,
  sampleTask,
  scriptedAdapter,
  tmpStateDir,
  type TestDeps,
} from "./helpers.js";

let dir: string;
let db: Db;

beforeEach(() => {
  dir = tmpStateDir();
  db = openDatabase(":memory:");
});
afterEach(() => {
  db.close();
  cleanupDir(dir);
});

const turn = (over: Partial<Parameters<typeof recordThreadTurn>[1]> = {}) => ({
  thread_id: "fab-1",
  provider: "fixture-web",
  account_id: "acct-fw-1",
  adapter_id: "fixture-web",
  provider_thread_id: "pt-1",
  provider_url: "https://fixture-web.test/c/pt-1",
  last_turn_fingerprint: "fp-1",
  model_class: null,
  ...over,
});

describe("recordThreadTurn", () => {
  it("opens epoch 1, advances the same provider thread, and opens epoch 2 on a new provider thread", () => {
    const m1 = recordThreadTurn(db, turn());
    expect(m1).toMatchObject({ epoch: 1, last_synced_turn_index: 1, status: "active", on_divergence: "fail" });

    const m2 = recordThreadTurn(db, turn({ last_turn_fingerprint: "fp-2" }));
    expect(m2.mapping_id).toBe(m1.mapping_id);
    expect(m2).toMatchObject({ epoch: 1, last_synced_turn_index: 2, last_turn_fingerprint: "fp-2" });

    const m3 = recordThreadTurn(db, turn({ provider_thread_id: "pt-2", last_turn_fingerprint: "fp-3" }));
    expect(m3).toMatchObject({ epoch: 2, provider_thread_id: "pt-2", context_transfer_from: m1.mapping_id });
    expect(getActiveThreadMapping(db, "fab-1")?.mapping_id).toBe(m3.mapping_id);
  });
});

describe("worker records the mapping when a threaded chat turn completes", () => {
  it("fingerprint = sha256 of the extracted reply (what readThread computes)", async () => {
    const log = new EventLog(db, new SseHub());
    insertTask(
      db,
      sampleTask({
        task_id: "t-thread",
        thread_id: "fab-9",
        routing: {
          mode: "auto",
          provider: "fixture-web" as never,
          account_id: "acct-fw-1",
          allow_fallback: true,
          allow_metered: false,
          allow_thread_migration: false,
        },
      })
    );
    const adapter = scriptedAdapter({
      execute: async function* (_task, ctx) {
        await ctx.markSubmitted(null);
        await ctx.markSubmitted(null);
        yield { t: "submitted", provider_thread_id: null, provider_url: "https://fixture-web.test/" };
        await ctx.markSubmitted("pt-late"); // late thread-id capture
        yield { t: "done", outcome: "success", text: "Hello there friend" };
      },
    });
    await runAttempt(
      { db, log, artifactsDir: join(dir, "artifacts") },
      { taskId: "t-thread", adapter, accountId: "acct-fw-1", page: fakeLease(), makeResolver: () => dummyResolver() }
    );
    expect(getTask(db, "t-thread")?.status).toBe("completed");
    expect(getActiveThreadMapping(db, "fab-9")).toMatchObject({
      provider_thread_id: "pt-late",
      account_id: "acct-fw-1",
      last_turn_fingerprint: createHash("sha256").update("Hello there friend").digest("hex"),
    });
  });

  it("a stateless task (no thread_id) records nothing", async () => {
    const log = new EventLog(db, new SseHub());
    insertTask(db, sampleTask({ task_id: "t-stateless" }));
    const adapter = scriptedAdapter({
      execute: async function* (_task, ctx) {
        await ctx.markSubmitted(null);
        await ctx.markSubmitted("pt-x");
        yield { t: "done", outcome: "success", text: "x" };
      },
    });
    await runAttempt(
      { db, log, artifactsDir: join(dir, "artifacts") },
      { taskId: "t-stateless", adapter, accountId: "acct-fw-1", page: fakeLease(), makeResolver: () => dummyResolver() }
    );
    expect(db.prepare("SELECT COUNT(*) AS n FROM thread_mappings").get()).toEqual({ n: 0 });
  });
});

describe("POST /v1/tasks chat.continue on a fabric thread", () => {
  let deps: TestDeps;
  afterEach(() => deps.cleanup());

  it("fills provider_thread_id + fingerprint from the mapping and pins the owning account", async () => {
    deps = makeDeps(dir);
    recordThreadTurn(deps.db, turn({ last_turn_fingerprint: "fp-live" }));
    const tok = issueToken(deps.db, "bot-1", "bot", ["tasks:submit"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${tok}`)
      .send({ initiated_by: HUMAN, capability: "chat.continue", prompt: "and then?", thread_id: "fab-1" });
    expect(res.status).toBe(201);
    expect(res.body.options).toMatchObject({
      provider_thread_id: "pt-1",
      last_turn_fingerprint: "fp-live",
      on_divergence: "fail",
    });
    expect(res.body.routing).toMatchObject({ provider: "fixture-web", account_id: "acct-fw-1" });
  });

  it("409 thread_not_mapped when the thread has no active provider thread", async () => {
    deps = makeDeps(dir);
    const tok = issueToken(deps.db, "bot-1", "bot", ["tasks:submit"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${tok}`)
      .send({ initiated_by: HUMAN, capability: "chat.continue", prompt: "x", thread_id: "nope" });
    expect(res.status).toBe(409);
    expect(res.body.error).toBe("thread_not_mapped");
  });
});
