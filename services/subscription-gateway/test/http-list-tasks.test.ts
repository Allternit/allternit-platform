// GET /v1/tasks — read-only task listing for Settings → Sessions Computer
// (recent tasks + the needs-user queue).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { issueToken } from "../src/security/tokens.js";
import { insertTask, TASK_PROMPT_PREVIEW_CHARS } from "../src/store/queries.js";
import { cleanupDir, makeDeps, sampleTask, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;
let deps: TestDeps;

beforeEach(() => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
});

afterEach(() => {
  deps.cleanup();
  cleanupDir(dir);
});

function token(scopes: Parameters<typeof issueToken>[3]): string {
  return issueToken(deps.db, "settings", "test", scopes).token;
}

function seed(): void {
  insertTask(
    deps.db,
    sampleTask({
      task_id: "t-old",
      status: "completed",
      created_at: "2026-09-28T10:00:00.000Z",
      updated_at: "2026-09-28T10:01:00.000Z",
      completed_at: "2026-09-28T10:01:00.000Z",
    })
  );
  insertTask(
    deps.db,
    sampleTask({
      task_id: "t-ask",
      status: "needs_user",
      status_detail: "ChatGPT asks which region to research",
      prompt: "research   the\nmarket",
      routing: {
        mode: "auto",
        provider: "chatgpt",
        account_id: "acct-1",
        allow_fallback: true,
        allow_metered: false,
        allow_thread_migration: false,
      },
      created_at: "2026-09-28T11:00:00.000Z",
      updated_at: "2026-09-28T11:02:00.000Z",
    })
  );
  insertTask(
    deps.db,
    sampleTask({
      task_id: "t-run",
      status: "running",
      prompt: "x".repeat(400),
      created_at: "2026-09-28T12:00:00.000Z",
      updated_at: "2026-09-28T12:00:00.000Z",
    })
  );
}

describe("GET /v1/tasks", () => {
  it("lists newest first as summaries without inputs, options or results", async () => {
    seed();
    const res = await request(deps.app).get("/v1/tasks").set("authorization", `Bearer ${token(["tasks:read"])}`);
    expect(res.status).toBe(200);
    expect(res.body.tasks.map((t: { task_id: string }) => t.task_id)).toEqual(["t-run", "t-ask", "t-old"]);
    const ask = res.body.tasks[1];
    expect(ask).toEqual({
      task_id: "t-ask",
      capability: "chat.create",
      status: "needs_user",
      status_detail: "ChatGPT asks which region to research",
      provider: "chatgpt",
      account_id: "acct-1",
      thread_id: null,
      prompt_preview: "research the market",
      created_at: "2026-09-28T11:00:00.000Z",
      updated_at: "2026-09-28T11:02:00.000Z",
      completed_at: null,
    });
    const long = res.body.tasks[0].prompt_preview as string;
    expect(long.length).toBe(TASK_PROMPT_PREVIEW_CHARS);
    expect(long.endsWith("…")).toBe(true);
  });

  it("filters by a comma-separated status list and caps with limit", async () => {
    seed();
    const auth = `Bearer ${token(["tasks:read"])}`;
    const needs = await request(deps.app).get("/v1/tasks?status=needs_user").set("authorization", auth);
    expect(needs.body.tasks.map((t: { task_id: string }) => t.task_id)).toEqual(["t-ask"]);

    const two = await request(deps.app).get("/v1/tasks?status=needs_user,completed").set("authorization", auth);
    expect(two.body.tasks.map((t: { task_id: string }) => t.task_id)).toEqual(["t-ask", "t-old"]);

    const one = await request(deps.app).get("/v1/tasks?limit=1").set("authorization", auth);
    expect(one.body.tasks.map((t: { task_id: string }) => t.task_id)).toEqual(["t-run"]);
  });

  it("rejects unknown statuses, bad limits and repeated params with 400", async () => {
    const auth = `Bearer ${token(["tasks:read"])}`;
    for (const q of ["status=bogus", "limit=0", "limit=101", "limit=abc", "status=queued&status=running"]) {
      const res = await request(deps.app).get(`/v1/tasks?${q}`).set("authorization", auth);
      expect(res.status, q).toBe(400);
      expect(res.body.error, q).toBe("invalid_query");
    }
  });

  it("needs tasks:read", async () => {
    const res = await request(deps.app).get("/v1/tasks").set("authorization", `Bearer ${token(["tasks:submit"])}`);
    expect(res.status).toBe(403);
    const unauth = await request(deps.app).get("/v1/tasks");
    expect(unauth.status).toBe(401);
  });
});
