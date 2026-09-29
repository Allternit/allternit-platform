// Settings › Subscriptions hub: providers, per-state user actions, one-step
// add + login, the login watcher that finishes a sign-in by itself, cancel,
// and remove. Fakes only — no browser, no provider.
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import request from "supertest";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import { issueToken } from "../src/security/tokens.js";
import { getAccount, upsertAccount } from "../src/store/queries.js";
import type { ImportableCookie, LoginBrowser } from "../src/worker/login_browser.js";
import type { WorkerPool } from "../src/worker/pool.js";
import { cleanupDir, fixtureWebConfig, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;
beforeEach(() => {
  dir = tmpStateDir();
});
afterEach(() => cleanupDir(dir));

const cookie = (name: string, value: string, domain: string): ImportableCookie => ({
  name,
  value,
  domain,
  path: "/",
  expires: -1,
  secure: true,
  httpOnly: true,
  sameSite: "Lax",
});

const until = async (check: () => Promise<boolean>, ms = 3000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await check()) return;
    await new Promise((r) => setTimeout(r, 20));
  }
  throw new Error("timed out");
};

describe("subscriptions hub", () => {
  let deps: TestDeps;
  afterEach(() => deps.cleanup());

  function setup(opts: { probe?: "ready" | "auth_required" } = {}) {
    const manifest = {
      ...fixtureWebConfig().manifest,
      auth: { ...fixtureWebConfig().manifest.auth, session_cookies: ["session-token"] },
    };
    const host = new URL(manifest.origins[0]).hostname;
    const jar: { cookies: ImportableCookie[] } = { cookies: [] };
    const calls: string[] = [];
    const pool = {
      deactivate: async () => void calls.push("deactivate"),
      // The probe after sign-in: records what it found, like the real pool.
      activate: async (lane: { account_id: string }) => {
        calls.push("activate");
        const a = getAccount(deps.db, lane.account_id)!;
        upsertAccount(deps.db, {
          ...a,
          session_health: opts.probe ?? "ready",
          identity: "eoj@example.com",
          usage: { remaining_pct: 8, resets_at: null, observed_at: "2026-09-29T14:54:00.000Z" },
        });
        return {};
      },
      userDataDirFor: (ref: string) => join(dir, ref),
    } as unknown as WorkerPool;
    let open = false;
    const loginBrowser: LoginBrowser = {
      open: async () => {
        calls.push("open");
        open = true;
      },
      isOpen: () => open,
      close: async () => {
        calls.push("close");
        open = false;
      },
    };
    const adapterRegistry = {
      adapters: [{ dir: "/x", manifest }],
      byId: () => undefined,
      capabilities: () => [],
    } as unknown as AdapterRegistry;
    deps = makeDeps(dir, {
      pool,
      adapterRegistry,
      loginBrowser,
      accountsOptions: { loginPollMs: 20, readLoginCookies: () => jar.cookies },
    });
    const tok = issueToken(deps.db, "admin-1", "test", ["accounts:manage"]).token;
    const api = (method: "get" | "post" | "delete", path: string) =>
      request(deps.app)[method](path).set("authorization", `Bearer ${tok}`);
    return { api, jar, calls, host, provider: manifest.provider, closeWindow: () => (open = false) };
  }

  it("lists providers: registered ones are supported, the rest are coming soon", async () => {
    const { api, provider } = setup();
    const res = await api("get", "/v1/providers");
    expect(res.status).toBe(200);
    const byId = Object.fromEntries(res.body.map((p: { id: string }) => [p.id, p]));
    expect(byId[provider]).toMatchObject({ supported: true, login_supported: true });
    expect(byId.claude).toMatchObject({ name: "Claude", supported: false, login_supported: false });
    expect(byId.kimi).toMatchObject({ name: "Kimi", supported: false });
  });

  it("one-step add opens the login; the watcher finishes the sign-in by itself", async () => {
    const { api, jar, calls, host, provider } = setup();
    const created = await api("post", "/v1/accounts").send({ provider, login: true });
    expect(created.status).toBe(201);
    const id = created.body.account_id as string;
    expect(created.body).toMatchObject({
      session_health: "auth_required",
      identity: null,
      usage: null,
      user_action: { kind: "login", label: "Log in" },
      login: { state: "waiting" },
    });
    expect(calls).toEqual(["deactivate", "open"]);

    // An unrelated cookie is not a sign-in.
    jar.cookies = [cookie("analytics", "x", host)];
    await new Promise((r) => setTimeout(r, 80));
    expect((await api("get", `/v1/accounts/${id}/login`)).body.state).toBe("waiting");

    // The provider's session cookie appears: Firefox closes, the lane reconnects.
    jar.cookies = [cookie("session-token.0", "abc", `.${host}`)];
    await until(async () => (await api("get", `/v1/accounts/${id}/login`)).body.state === "signed_in");
    const done = (await api("get", `/v1/accounts/${id}/login`)).body;
    expect(done.account).toMatchObject({
      session_health: "ready",
      identity: "eoj@example.com",
      usage: { remaining_pct: 8 },
      user_action: { kind: "none" },
    });
    expect(calls.slice(2)).toEqual(["close", "deactivate", "activate"]);
  });

  it("a session cookie already there when the window opened is not a new sign-in", async () => {
    const { api, jar, host, provider } = setup();
    jar.cookies = [cookie("session-token", "old", host)];
    const created = await api("post", "/v1/accounts").send({ provider, login: true });
    const id = created.body.account_id as string;
    await new Promise((r) => setTimeout(r, 80));
    expect((await api("get", `/v1/accounts/${id}/login`)).body.state).toBe("waiting");
    jar.cookies = [cookie("session-token", "new", host)];
    await until(async () => (await api("get", `/v1/accounts/${id}/login`)).body.state === "signed_in");
  });

  it("a sign-in the probe doesn't accept is failed, with what to do", async () => {
    const { api, jar, host, provider } = setup({ probe: "auth_required" });
    const id = (await api("post", "/v1/accounts").send({ provider, login: true })).body.account_id as string;
    jar.cookies = [cookie("session-token", "abc", host)];
    await until(async () => (await api("get", `/v1/accounts/${id}/login`)).body.state === "failed");
    expect((await api("get", `/v1/accounts/${id}/login`)).body.detail).toBe("Log in");
  });

  it("closing the window, or cancelling, ends the login as closed; the account stays as it was", async () => {
    const { api, provider, closeWindow, calls } = setup();
    const a = (await api("post", "/v1/accounts").send({ provider, login: true })).body.account_id as string;
    closeWindow();
    await until(async () => (await api("get", `/v1/accounts/${a}/login`)).body.state === "closed");

    const b = (await api("post", "/v1/accounts").send({ provider, login: true })).body.account_id as string;
    const cancelled = await api("delete", `/v1/accounts/${b}/login`);
    expect(cancelled.body.state).toBe("closed");
    expect(calls).toContain("close");
    expect(getAccount(deps.db, b)?.session_health).toBe("auth_required");
  });

  it("every session state maps to a user action", async () => {
    const { api, provider } = setup();
    const id = (await api("post", "/v1/accounts").send({ provider })).body.account_id as string;
    const expected: Record<string, string> = {
      ready: "none",
      auth_required: "login",
      challenge_presented: "login",
      profile_locked: "wait",
      degraded: "wait",
      provider_down: "wait",
      ui_drift: "wait",
      account_restricted: "contact",
    };
    for (const [health, kind] of Object.entries(expected)) {
      upsertAccount(deps.db, { ...getAccount(deps.db, id)!, session_health: health as never });
      const row = (await api("get", "/v1/accounts")).body.find((x: { account_id: string }) => x.account_id === id);
      expect(row.user_action.kind, health).toBe(kind);
    }
  });

  it("remove: 409 while a task runs on it; otherwise signs out (profiles deleted) and deletes the row", async () => {
    const { api, provider } = setup();
    const id = (await api("post", "/v1/accounts").send({ provider, label: "Mine" })).body.account_id as string;
    const chrome = join(dir, `profiles/${id}`);
    mkdirSync(chrome, { recursive: true });
    mkdirSync(`${chrome}-firefox`, { recursive: true });

    const now = new Date().toISOString();
    deps.db
      .prepare(
        `INSERT INTO tasks (task_id, requester_id, capability, capability_version, requester, prompt, inputs, options, routing, constraints, priority, status, created_at, updated_at)
         VALUES ('t1', 'u', 'chat.create', 1, '{}', 'p', '[]', '{}', ?, '{}', 'interactive', 'running', ?, ?)`
      )
      .run(JSON.stringify({ account_id: id }), now, now);
    const busy = await api("delete", `/v1/accounts/${id}`);
    expect(busy.status).toBe(409);
    expect(busy.body.error).toBe("account_busy");

    deps.db.prepare("UPDATE tasks SET status = 'completed' WHERE task_id = 't1'").run();
    const removed = await api("delete", `/v1/accounts/${id}`);
    expect(removed.status).toBe(204);
    expect(getAccount(deps.db, id)).toBeNull();
    expect(existsSync(chrome)).toBe(false);
    expect(existsSync(`${chrome}-firefox`)).toBe(false);
  });
});
