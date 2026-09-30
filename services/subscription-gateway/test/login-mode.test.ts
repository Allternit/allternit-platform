// Login mode (P3 gate finding): Google refuses sign-in in automated Chrome, so
// the human logs in inside a plain Firefox on the account's Firefox profile and
// the adapter's Chrome imports that session on its next launch.
import { EventEmitter } from "node:events";
import { mkdirSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import Database from "better-sqlite3";
import request from "supertest";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import { issueToken } from "../src/security/tokens.js";
import { upsertAccount } from "../src/store/queries.js";
import {
  createChromeLoginBrowser,
  createFirefoxLoginBrowser,
  firefoxProfileFor,
  importFirefoxSessionIfNewer,
  readFirefoxCookies,
  type ImportableCookie,
  type LoginBrowser,
} from "../src/worker/login_browser.js";
import type { WorkerPool } from "../src/worker/pool.js";
import { cleanupDir, fixtureWebConfig, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;

beforeEach(() => {
  dir = tmpStateDir();
});
afterEach(() => cleanupDir(dir));

function writeFirefoxCookies(profile: string, rows: Array<Record<string, unknown>>): void {
  mkdirSync(profile, { recursive: true });
  const db = new Database(join(profile, "cookies.sqlite"));
  db.exec(
    "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER)"
  );
  const ins = db.prepare(
    "INSERT INTO moz_cookies (name, value, host, path, expiry, isSecure, isHttpOnly, sameSite) VALUES (@name, @value, @host, @path, @expiry, @isSecure, @isHttpOnly, @sameSite)"
  );
  for (const r of rows) ins.run(r);
  db.close();
}

const NOW_S = 1_800_000_000;
const row = (over: Record<string, unknown> = {}) => ({
  name: "__Secure-session",
  value: "v",
  host: ".chatgpt.com",
  path: "/",
  expiry: NOW_S + 3600,
  isSecure: 1,
  isHttpOnly: 1,
  sameSite: 1,
  ...over,
});

describe("readFirefoxCookies", () => {
  it("maps moz_cookies to Playwright cookies; drops expired; normalizes ms expiry and SameSite=None→Secure", () => {
    const profile = join(dir, "ff");
    writeFirefoxCookies(profile, [
      row(),
      row({ name: "expired", expiry: NOW_S - 1 }),
      row({ name: "ms", expiry: (NOW_S + 60) * 1000 }),
      row({ name: "none", isSecure: 0, sameSite: 0 }),
    ]);
    const cookies = readFirefoxCookies(profile, NOW_S);
    expect(cookies.map((c) => c.name).sort()).toEqual(["__Secure-session", "ms", "none"]);
    expect(cookies.find((c) => c.name === "ms")?.expires).toBe(NOW_S + 60);
    const none = cookies.find((c) => c.name === "none")!;
    expect(none.sameSite).toBe("None");
    expect(none.secure).toBe(true);
    expect(cookies.find((c) => c.name === "__Secure-session")).toMatchObject({
      domain: ".chatgpt.com",
      httpOnly: true,
      sameSite: "Lax",
    });
  });

  it("returns [] when the profile has no cookie store", () => {
    expect(readFirefoxCookies(join(dir, "nope"))).toEqual([]);
  });
});

describe("importFirefoxSessionIfNewer", () => {
  it("imports once, skips an unchanged store, re-imports after the store changes", async () => {
    const userDataDir = join(dir, "profiles", "acct-1");
    mkdirSync(userDataDir, { recursive: true });
    writeFirefoxCookies(firefoxProfileFor(userDataDir), [row({ expiry: 4_000_000_000 })]);
    const added: ImportableCookie[][] = [];
    const ctx = { addCookies: async (c: ImportableCookie[]) => void added.push(c) };

    expect(await importFirefoxSessionIfNewer(userDataDir, ctx)).toBe(1);
    expect(await importFirefoxSessionIfNewer(userDataDir, ctx)).toBe(0); // unchanged → never overwrite Chrome
    const db = join(firefoxProfileFor(userDataDir), "cookies.sqlite");
    const later = new Date(statSync(db).mtimeMs + 5000);
    utimesSync(db, later, later); // a new login in Firefox
    expect(await importFirefoxSessionIfNewer(userDataDir, ctx)).toBe(1);
    expect(added).toHaveLength(2);
  });

  it("no Firefox profile → nothing imported", async () => {
    const ctx = { addCookies: async () => {} };
    expect(await importFirefoxSessionIfNewer(join(dir, "none"), ctx)).toBe(0);
  });
});

describe("createFirefoxLoginBrowser", () => {
  it("opens plain Firefox (no automation flags) on the account's Firefox profile and closes it gracefully", async () => {
    const spawned: Array<{ exe: string; args: string[] }> = [];
    const signals: string[] = [];
    const spawnFn = ((exe: string, args: string[]) => {
      spawned.push({ exe, args });
      const child = new EventEmitter() as EventEmitter & { kill: (s: string) => boolean };
      child.kill = (s: string) => {
        signals.push(s);
        setImmediate(() => child.emit("exit", 0));
        return true;
      };
      return child;
    }) as unknown as typeof import("node:child_process").spawn;
    const lb = createFirefoxLoginBrowser({ executable: "/opt/firefox/firefox", spawnFn });
    const userDataDir = join(dir, "profiles", "acct-1");
    await lb.open("acct-1", userDataDir, "https://chatgpt.com/");
    await lb.open("acct-1", userDataDir, "https://chatgpt.com/"); // idempotent
    expect(spawned).toHaveLength(1);
    expect(spawned[0].args).toEqual([
      "--profile",
      firefoxProfileFor(userDataDir),
      "--no-remote",
      "https://chatgpt.com/",
    ]);
    expect(spawned[0].args.join(" ")).not.toMatch(/marionette|remote-debugging|headless/);
    expect(statSync(join(firefoxProfileFor(userDataDir), "user.js")).isFile()).toBe(true);
    expect(lb.isOpen("acct-1")).toBe(true);
    await lb.close("acct-1");
    expect(signals).toEqual(["SIGTERM"]);
    expect(lb.isOpen("acct-1")).toBe(false);
  });

  it("Chrome login browser: plain Chrome on the account's own profile, no automation flags", async () => {
    const spawned: Array<{ exe: string; args: string[] }> = [];
    const spawnFn = ((exe: string, args: string[]) => {
      spawned.push({ exe, args });
      const child = new EventEmitter() as EventEmitter & { kill: (s: string) => boolean };
      child.kill = () => {
        setImmediate(() => child.emit("exit", 0));
        return true;
      };
      return child;
    }) as unknown as typeof import("node:child_process").spawn;
    const lb = createChromeLoginBrowser({ executable: "/usr/bin/google-chrome-stable", spawnFn, isRoot: false });
    const userDataDir = join(dir, "profiles", "acct-1");
    expect(lb.profileFor(userDataDir)).toBe(userDataDir);
    await lb.open("acct-1", userDataDir, "https://claude.ai/");
    expect(spawned[0].args).toContain(`--user-data-dir=${userDataDir}`);
    expect(spawned[0].args).toContain("--password-store=basic");
    expect(spawned[0].args.at(-1)).toBe("https://claude.ai/");
    expect(spawned[0].args.join(" ")).not.toMatch(/remote-debugging|enable-automation|headless|no-sandbox/);
    await lb.close("acct-1");
    expect(lb.isOpen("acct-1")).toBe(false);

    // As root (Sessions machines), Chrome only starts with --no-sandbox.
    const asRoot = createChromeLoginBrowser({ executable: "/usr/bin/google-chrome-stable", spawnFn, isRoot: true });
    await asRoot.open("acct-2", join(dir, "profiles", "acct-2"), "https://kimi.com/");
    expect(spawned[1].args).toContain("--no-sandbox");
    await asRoot.close("acct-2");
  });
});

describe("POST /v1/accounts/:id/login + connect", () => {
  let deps: TestDeps;
  afterEach(() => deps.cleanup());

  function setup(withLoginBrowser = true) {
    const calls: string[] = [];
    const pool = {
      deactivate: async () => void calls.push("deactivate"),
      activate: async () => {
        calls.push("activate");
        return {};
      },
      userDataDirFor: (ref: string) => join(dir, ref),
    } as unknown as WorkerPool;
    let open = false;
    const loginBrowser: LoginBrowser = {
      profileFor: firefoxProfileFor,
      readCookies: () => [],
      open: async (id, udd, url) => {
        calls.push(`open:${id}:${udd.endsWith("profiles/acct-1")}:${url}`);
        open = true;
      },
      isOpen: () => open,
      close: async () => {
        calls.push("close");
        open = false;
      },
    };
    const manifest = fixtureWebConfig().manifest;
    const adapterRegistry = {
      adapters: [{ dir: "/x", manifest }],
      byId: () => undefined,
      capabilities: () => [],
    } as unknown as AdapterRegistry;
    deps = makeDeps(dir, { pool, adapterRegistry, loginBrowser: withLoginBrowser ? loginBrowser : undefined });
    upsertAccount(deps.db, {
      account_id: "acct-1",
      provider: manifest.provider,
      label: "Fixture",
      plan: null,
      plan_observed_at: null,
      profile_ref: "profiles/acct-1",
      session_health: "auth_required",
      enabled: true,
    });
    const tok = issueToken(deps.db, "admin-1", "test", ["accounts:manage"]).token;
    return { calls, tok, origin: manifest.origins[0] };
  }

  it("closes the lane's Chrome, opens Firefox at the provider origin; connect then closes Firefox before relaunching", async () => {
    const { calls, tok, origin } = setup();
    const res = await request(deps.app).post("/v1/accounts/acct-1/login").set("authorization", `Bearer ${tok}`);
    expect(res.status).toBe(200);
    expect(res.body.status).toBe("login_window_open");
    expect(calls).toEqual(["deactivate", `open:acct-1:true:${origin}`]);

    const c = await request(deps.app).post("/v1/accounts/acct-1/connect").set("authorization", `Bearer ${tok}`);
    expect(c.status).toBe(200);
    expect(calls.slice(2)).toEqual(["close", "deactivate", "activate"]);
  });

  it("501 when no login browser is configured", async () => {
    const { tok } = setup(false);
    const res = await request(deps.app).post("/v1/accounts/acct-1/login").set("authorization", `Bearer ${tok}`);
    expect(res.status).toBe(501);
    expect(res.body.error).toBe("login_browser_unavailable");
  });

  it("bots cannot open a login window (accounts:manage only)", async () => {
    setup();
    const bot = issueToken(deps.db, "bot-1", "bot", ["tasks:submit"]).token;
    const res = await request(deps.app).post("/v1/accounts/acct-1/login").set("authorization", `Bearer ${bot}`);
    expect(res.status).toBe(403);
  });
});
