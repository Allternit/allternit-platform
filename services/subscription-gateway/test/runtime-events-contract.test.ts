// The fields allternit-api's subscription sync reads to turn login health and
// needs_user tasks into runtime ledger events (cmd/allternit-api/src/
// subscription_sync.rs: `logins_from`, `record_needs_user_tasks`). The sync
// is the only path Subscriptions events take into the event backbone, so a
// rename here would silently stop `subscription.login_needed` /
// `subscription.signed_in` / `subscription.task.needs_user`.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { issueToken } from "../src/security/tokens.js";
import { insertTask, upsertAccount } from "../src/store/queries.js";
import { Notifier } from "../src/events/notify.js";
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

const auth = (scopes: Parameters<typeof issueToken>[3]) => `Bearer ${issueToken(deps.db, "allternit-api", "test", scopes).token}`;

describe("runtime event sync contract", () => {
  it("GET /v1/accounts carries account_id, provider, session_health, enabled and user_action.label", async () => {
    upsertAccount(deps.db, {
      account_id: "acct-1",
      provider: "chatgpt",
      label: "Work",
      plan: null,
      plan_observed_at: null,
      profile_ref: "p1",
      session_health: "auth_required",
      enabled: true,
    } as never);
    const res = await request(deps.app).get("/v1/accounts").set("authorization", auth(["accounts:manage"]));
    expect(res.status).toBe(200);
    const row = res.body.find((r: { account_id: string }) => r.account_id === "acct-1");
    expect(row).toMatchObject({ account_id: "acct-1", provider: "chatgpt", session_health: "auth_required", enabled: true });
    expect(typeof row.user_action?.label).toBe("string");
  });

  it("GET /v1/tasks?status=needs_user carries task_id, status, status_detail, provider, account_id, thread_id, updated_at", async () => {
    insertTask(
      deps.db,
      sampleTask({
        task_id: "t-ask",
        status: "needs_user",
        status_detail: "Sign in to ChatGPT",
        updated_at: "2026-10-05T10:00:00.000Z",
      })
    );
    const res = await request(deps.app)
      .get("/v1/tasks?status=needs_user&limit=100")
      .set("authorization", auth(["tasks:read"]));
    expect(res.status).toBe(200);
    const [t] = res.body.tasks;
    for (const k of ["task_id", "status", "status_detail", "provider", "account_id", "thread_id", "updated_at", "capability"]) {
      expect(t, k).toHaveProperty(k);
    }
    expect(t.status).toBe("needs_user");
  });

  it("the notifier no longer carries an MCP hook (the sync is the one events path)", async () => {
    const calls: string[] = [];
    const n = new Notifier({
      notificationsDir: dir,
      fetchImpl: (async (url: string) => {
        calls.push(String(url));
        return new Response("{}", { status: 200 });
      }) as unknown as typeof fetch,
    });
    await n.notifyTerminal({ task_id: "t1", status: "needs_user", caller_id: "bot-1", requester_kind: "bot", thread_id: null, detail: null });
    expect(calls).toEqual(["http://127.0.0.1:18013/api/rails/peers/bot-1/send"]);
    expect("mcp" in (n as unknown as Record<string, unknown>)).toBe(false);
  });
});
