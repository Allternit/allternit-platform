// Browser lanes on the person's own computer (Eoj, 2026-10-17): off until
// turned on, then run in the installed Google Chrome, hidden, on the account's
// own profile. src/local_desktop.ts, src/http/routes_settings.ts,
// src/worker/hidden_chrome.ts.
import { EventEmitter } from "node:events";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import request from "supertest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  browserLanesEnabled,
  browserLanesRoutable,
  readBrowserLanes,
  setBrowserLanesCheck,
  writeBrowserLanes,
} from "../src/local_desktop.js";
import { buildSnapshot } from "../src/router/snapshot.js";
import { issueToken } from "../src/security/tokens.js";
import { chromeMajorVersion, detectGoogleChrome, hiddenChromeArgs, launchHiddenChrome, normalUserAgent } from "../src/worker/hidden_chrome.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

const LOCAL = { SUBS_GATEWAY_LOCAL_DESKTOP: "1" };

describe("browser lanes switch", () => {
  let dir: string;
  beforeEach(() => (dir = tmpStateDir()));
  afterEach(() => cleanupDir(dir));

  it("is always on off the person's computer (a Sessions computer)", () => {
    expect(readBrowserLanes(dir, {})).toEqual({ local_desktop: false, enabled: true, changed_at: null });
  });

  it("is off on the person's computer until they turn it on, and remembers the change", () => {
    expect(browserLanesEnabled(dir, LOCAL)).toBe(false);
    writeBrowserLanes(dir, true, new Date("2026-10-17T12:00:00Z"));
    expect(readBrowserLanes(dir, LOCAL)).toEqual({ local_desktop: true, enabled: true, changed_at: "2026-10-17T12:00:00.000Z" });
    writeBrowserLanes(dir, false);
    expect(browserLanesEnabled(dir, LOCAL)).toBe(false);
  });

  it("never turns browsers on from an unreadable file", () => {
    writeFileSync(join(dir, "browser-lanes.json"), "{not json");
    expect(browserLanesEnabled(dir, LOCAL)).toBe(false);
  });

  it("takes browser accounts out of routing while off", () => {
    const deps = makeDeps(dir);
    try {
      deps.db
        .prepare("INSERT INTO accounts (account_id, provider, label, plan, profile_ref, session_health, enabled) VALUES (?, ?, ?, ?, ?, ?, 1)")
        .run("acc-1", "chatgpt", "Me", "plus", "profiles/acc-1", "ready");
      const registry = { adapters: [], byId: () => undefined, capabilities: () => [] } as never;
      setBrowserLanesCheck(() => false);
      expect(buildSnapshot(deps.db, registry).accounts).toEqual([]);
      setBrowserLanesCheck(() => true);
      expect(buildSnapshot(deps.db, registry).accounts.map((a) => a.account_id)).toEqual(["acc-1"]);
      setBrowserLanesCheck(() => {
        throw new Error("unreadable");
      });
      expect(browserLanesRoutable()).toBe(false);
    } finally {
      setBrowserLanesCheck(() => true);
      deps.cleanup();
    }
  });
});

describe("GET/PUT /v1/settings/browser-lanes", () => {
  let dir: string;
  let deps: TestDeps;
  const auth = () => `Bearer ${issueToken(deps.db, "cli", "test", ["accounts:manage"]).token}`;

  beforeEach(() => {
    dir = tmpStateDir();
    deps = makeDeps(dir);
  });
  afterEach(() => {
    vi.unstubAllEnvs();
    deps.cleanup();
    cleanupDir(dir);
  });

  it("refuses to change anything on a Sessions computer", async () => {
    const get = await request(deps.app).get("/v1/settings/browser-lanes").set("authorization", auth());
    expect(get.body).toMatchObject({ local_desktop: false, enabled: true, chrome_installed: null });
    const put = await request(deps.app).put("/v1/settings/browser-lanes").set("authorization", auth()).send({ enabled: false });
    expect(put.status).toBe(409);
  });

  it("turns browser lanes on and off on the person's computer", async () => {
    vi.stubEnv("SUBS_GATEWAY_LOCAL_DESKTOP", "1");
    const first = await request(deps.app).get("/v1/settings/browser-lanes").set("authorization", auth());
    expect(first.body).toMatchObject({ local_desktop: true, enabled: false });
    expect(typeof first.body.chrome_installed).toBe("boolean");
    const on = await request(deps.app).put("/v1/settings/browser-lanes").set("authorization", auth()).send({ enabled: true });
    expect(on.status).toBe(200);
    expect(on.body.enabled).toBe(true);
    const bad = await request(deps.app).put("/v1/settings/browser-lanes").set("authorization", auth()).send({ enabled: "yes" });
    expect(bad.status).toBe(400);
  });

  it("says browser sign-in is off instead of opening a window", async () => {
    vi.stubEnv("SUBS_GATEWAY_LOCAL_DESKTOP", "1");
    const providers = await request(deps.app).get("/v1/providers").set("authorization", auth());
    expect(providers.body.every((p: { login_supported: boolean }) => p.login_supported === false)).toBe(true);
  });
});

describe("hidden Chrome", () => {
  it("only ever picks the installed Google Chrome", () => {
    const mac = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    expect(detectGoogleChrome("darwin", {}, (p) => p === mac)).toBe(mac);
    expect(detectGoogleChrome("darwin", {}, () => false)).toBeNull();
    const win = join("C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe");
    expect(detectGoogleChrome("win32", { PROGRAMFILES: "C:\\Program Files" }, (p) => p === win)).toBe(win);
  });

  it("starts headless on its own profile with the automation flags off", () => {
    const args = hiddenChromeArgs("/state/profiles/acc-1", false);
    expect(args).toContain("--headless=new");
    expect(args).toContain("--user-data-dir=/state/profiles/acc-1");
    expect(args).toContain("--disable-blink-features=AutomationControlled");
    expect(args).toContain("--remote-debugging-port=0");
    expect(args.some((a) => a.startsWith("--enable-automation"))).toBe(false);
    expect(hiddenChromeArgs("/p", true)).toContain("--no-sandbox");
  });

  it("sends Chrome's normal user agent, never HeadlessChrome (Cloudflare holds that one)", () => {
    expect(chromeMajorVersion("/chrome", "darwin", () => "Google Chrome 155.0.8059.84 \n")).toBe(155);
    expect(chromeMajorVersion("C:\\Chrome\\chrome.exe", "win32", () => "", () => ["154.0.1.2", "155.0.8059.84", "Locales"])).toBe(155);
    expect(chromeMajorVersion("/missing", "linux", () => { throw new Error("ENOENT"); })).toBeNull();
    const ua = normalUserAgent(155, "darwin");
    expect(ua).toBe("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36");
    expect(ua).not.toContain("Headless");
    expect(normalUserAgent(155, "win32")).toContain("Windows NT 10.0; Win64; x64");
    expect(hiddenChromeArgs("/p", false, ua)).toContain(`--user-agent=${ua}`);
  });

  it("attaches over the port Chrome writes, and kills Chrome on close", async () => {
    const dir = tmpStateDir();
    try {
      const child = Object.assign(new EventEmitter(), { kill: vi.fn(() => child.emit("exit", 0)) });
      const spawnFn = vi.fn((_exe: string, args: string[]) => {
        const profile = args.find((a) => a.startsWith("--user-data-dir="))!.slice("--user-data-dir=".length);
        mkdirSync(profile, { recursive: true });
        setTimeout(() => writeFileSync(join(profile, "DevToolsActivePort"), "41234\n/devtools/browser/x\n"), 20);
        return child;
      });
      const context = { id: "ctx" };
      const browser = { contexts: () => [context], close: vi.fn(async () => {}) };
      const connect = vi.fn(async () => browser);
      const hidden = await launchHiddenChrome(join(dir, "profile"), {
        executable: "/chrome",
        spawnFn: spawnFn as never,
        connect: connect as never,
        isRoot: false,
        userAgent: null,
      });
      expect(connect).toHaveBeenCalledWith("http://127.0.0.1:41234");
      expect(hidden.context).toBe(context);
      await hidden.close();
      expect(browser.close).toHaveBeenCalled();
      expect(child.kill).toHaveBeenCalled();
    } finally {
      cleanupDir(dir);
    }
  });

  it("fails clearly when Chrome exits before it starts", async () => {
    const dir = tmpStateDir();
    try {
      const child = Object.assign(new EventEmitter(), { kill: vi.fn() });
      const spawnFn = vi.fn(() => {
        setTimeout(() => child.emit("exit", 1), 10);
        return child;
      });
      await expect(
        launchHiddenChrome(join(dir, "p"), { executable: "/chrome", spawnFn: spawnFn as never, startTimeoutMs: 2000, userAgent: null })
      ).rejects.toThrow("Google Chrome exited before it started");
    } finally {
      cleanupDir(dir);
    }
  });
});
