// Memory Drive reads: GET /v1/accounts/:id/memory, /threads, /threads/:id —
// read-only, never on a busy account, honest about health — plus the pure
// parsers that turn ChatGPT's and Claude's own page data into plain text.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { issueToken } from "../src/security/tokens.js";
import { upsertAccount } from "../src/store/queries.js";
import type { LaneRuntime, WorkerPool } from "../src/worker/pool.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";
import {
  chatgptMemory,
  chatgptThreads,
  chatgptThreadText,
  claudeMemory,
  claudeThreads,
  claudeThreadText,
  splitMemoryText,
} from "../adapters/_shared/vendor-memory.js";

let dir: string;
let deps: TestDeps;
const auth = () => `Bearer ${issueToken(deps.db, "admin-1", "test", ["accounts:manage"]).token}`;

function seed(enabled = true): void {
  upsertAccount(deps.db, {
    account_id: "acct-1",
    provider: "fixture-web",
    label: "Fixture",
    plan: null,
    plan_observed_at: null,
    profile_ref: "profiles/acct-1",
    session_health: "ready",
    enabled,
  });
}

function pool(runtime: Partial<LaneRuntime>, outcome: "ok" | "busy" | "not_ready" = "ok"): { pool: WorkerPool; reads: number } {
  const state = { reads: 0 };
  const p = {
    readForMemory: async (_lane: unknown, read: (rt: LaneRuntime) => Promise<unknown> | null) => {
      if (outcome === "busy") return { ok: false, reason: "busy" };
      if (outcome === "not_ready") return { ok: false, reason: "not_ready", health: "challenge_presented" };
      state.reads++;
      const pending = read(runtime as LaneRuntime);
      if (!pending) return { ok: false, reason: "not_supported" };
      return { ok: true, value: await pending };
    },
  } as unknown as WorkerPool;
  return { pool: p, get reads() { return state.reads; } };
}

beforeEach(() => {
  dir = tmpStateDir();
});
afterEach(() => {
  deps.cleanup();
  cleanupDir(dir);
});

describe("memory read routes", () => {
  it("returns what the provider remembers", async () => {
    const fp = pool({ readMemory: async () => ({ available: true, memories: [{ text: "Likes tea.", updated_at: null }], instructions: [] }) });
    deps = makeDeps(dir, { pool: fp.pool });
    seed();
    const res = await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", auth());
    expect(res.status).toBe(200);
    expect(res.body.memories).toEqual([{ text: "Likes tea.", updated_at: null }]);
  });

  it("lists and reads past chats, bounding the limit", async () => {
    let asked = 0;
    const fp = pool({
      listThreads: async ({ limit }) => {
        asked = limit;
        return [{ id: "c1", title: "Trip", updated_at: null }];
      },
      readThreadText: async (id) => (id === "c1" ? { id, title: "Trip", updated_at: null, messages: [{ role: "user", text: "Plan Lisbon" }] } : null),
    });
    deps = makeDeps(dir, { pool: fp.pool });
    seed();
    const list = await request(deps.app).get("/v1/accounts/acct-1/threads?limit=500").set("authorization", auth());
    expect(list.status).toBe(200);
    expect(asked).toBe(100);
    const one = await request(deps.app).get("/v1/accounts/acct-1/threads/c1").set("authorization", auth());
    expect(one.body.messages[0].text).toBe("Plan Lisbon");
    expect((await request(deps.app).get("/v1/accounts/acct-1/threads/zz").set("authorization", auth())).status).toBe(404);
    expect((await request(deps.app).get("/v1/accounts/acct-1/threads/a%2Fb").set("authorization", auth())).status).toBe(400);
  });

  it("never reads a busy account and says why it can't read", async () => {
    deps = makeDeps(dir, { pool: pool({}, "busy").pool });
    seed();
    const busy = await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", auth());
    expect([busy.status, busy.body.error]).toEqual([409, "busy"]);
    deps.cleanup();
    deps = makeDeps(dir, { pool: pool({}, "not_ready").pool });
    seed();
    const wall = await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", auth());
    expect([wall.status, wall.body.error, wall.body.session_health]).toEqual([409, "not_ready", "challenge_presented"]);
  });

  it("501 when the adapter has no reader; 409 when the login is off; needs accounts:manage", async () => {
    deps = makeDeps(dir, { pool: pool({}).pool });
    seed();
    expect((await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", auth())).status).toBe(501);
    const reader = issueToken(deps.db, "admin-1", "test", ["tasks:read"]).token;
    expect((await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", `Bearer ${reader}`)).status).toBe(403);
    seed(false);
    expect((await request(deps.app).get("/v1/accounts/acct-1/memory").set("authorization", auth())).body.error).toBe("account_disabled");
  });
});

describe("vendor memory parsers", () => {
  it("ChatGPT: saved memories, custom instructions and the history list", () => {
    const m = chatgptMemory({
      memories: { ok: true, body: { memories: [{ content: "  Lives in Saint Paul. ", updated_at: "2026-10-01T00:00:00Z" }, { content: "" }] } },
      instructions: { ok: true, body: { enabled: true, about_user_message: "I run Allternit.", about_model_message: "Be brief." } },
    });
    expect(m.available).toBe(true);
    expect(m.memories).toEqual([{ text: "Lives in Saint Paul.", updated_at: "2026-10-01T00:00:00.000Z" }]);
    expect(m.instructions.map((i) => i.label)).toEqual(["About you", "How ChatGPT should respond"]);
    expect(chatgptMemory({ memories: { ok: false, body: null }, instructions: { ok: true, body: { enabled: false, about_user_message: "x" } } }).instructions).toEqual([]);
    expect(chatgptThreads({ items: [{ id: "c1", title: "Trip", update_time: 1760000000 }, { title: "no id" }] })).toEqual([
      { id: "c1", title: "Trip", updated_at: new Date(1760000000 * 1000).toISOString() },
    ]);
  });

  it("ChatGPT: a chat is the visible branch from the current node, hidden nodes dropped", () => {
    const t = chatgptThreadText("c1", "Trip", null, "a2", [
      { id: "root", parent: null, role: "system", text: "sys", hidden: true },
      { id: "u1", parent: "root", role: "user", text: "Plan Lisbon", hidden: false },
      { id: "a1", parent: "u1", role: "assistant", text: "Old branch", hidden: false },
      { id: "a2", parent: "u1", role: "assistant", text: "Day 1: Alfama", hidden: false },
      { id: "x", parent: "a2", role: "tool", text: "tool output", hidden: false },
    ]);
    expect(t.messages).toEqual([
      { role: "user", text: "Plan Lisbon" },
      { role: "assistant", text: "Day 1: Alfama" },
    ]);
  });

  it("Claude: memory text or list, preferences, history and one chat", () => {
    expect(claudeMemory({ memory: { ok: true, body: { memory: "# Work\n- Runs Allternit\n- Uses Rust" } }, profile: { ok: true, body: { conversation_preferences: "Short answers." } } })).toEqual({
      available: true,
      memories: [{ text: "Runs Allternit", updated_at: null }, { text: "Uses Rust", updated_at: null }],
      instructions: [{ label: "Personal preferences", text: "Short answers." }],
    });
    expect(claudeMemory({ memory: { ok: false, body: null }, profile: { ok: false, body: null } }).available).toBe(false);
    expect(claudeThreads([{ uuid: "u-1", name: "", updated_at: "2026-10-02T00:00:00Z" }])[0].title).toBe("Untitled");
    const chat = claudeThreadText("u-1", {
      name: "Rust",
      chat_messages: [
        { sender: "human", content: [{ type: "text", text: "How do I borrow?" }] },
        { sender: "assistant", content: [{ type: "thinking", thinking: "…" }, { type: "text", text: "Use &." }] },
      ],
    });
    expect(chat?.messages).toEqual([
      { role: "user", text: "How do I borrow?" },
      { role: "assistant", text: "Use &." },
    ]);
    expect(claudeThreadText("u-1", { error: "x" })).toBeNull();
  });

  it("splits free-text memory into lines", () => {
    expect(splitMemoryText("## Heading\n1. First\n* Second\n\nok")).toEqual(["First", "Second"]);
  });
});
