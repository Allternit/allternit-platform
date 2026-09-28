// Login mode (P3 gate finding): providers that sign in through Google refuse
// automated Chrome ("This browser or app may not be secure"), so the human
// logs in inside a plain, non-automated Firefox on a per-account profile.
// The adapter keeps running Chrome; on the next launch the Firefox session is
// imported into the Chrome profile. Cookie values are never logged.
import { spawn, type ChildProcess } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import Database from "better-sqlite3";

// The Firefox profile that belongs to a Chrome user-data dir.
export function firefoxProfileFor(userDataDir: string): string {
  return `${userDataDir}-firefox`;
}

export interface LoginBrowser {
  open(accountId: string, userDataDir: string, url: string): Promise<void>;
  isOpen(accountId: string): boolean;
  // Graceful close so Firefox flushes cookies.sqlite; no-op when not open.
  close(accountId: string): Promise<void>;
}

export interface FirefoxLoginBrowserOptions {
  executable: string;
  spawnFn?: typeof spawn;
  closeTimeoutMs?: number;
}

// First-run/onboarding screens would sit between the human and the login page.
const PROFILE_PREFS = [
  'user_pref("browser.aboutwelcome.enabled", false);',
  'user_pref("browser.preonboarding.enabled", false);',
  'user_pref("termsofuse.bypassNotification", true);',
  'user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);',
  'user_pref("browser.startup.homepage_override.mstone", "ignore");',
  'user_pref("browser.shell.checkDefaultBrowser", false);',
].join("\n");

export function createFirefoxLoginBrowser(opts: FirefoxLoginBrowserOptions): LoginBrowser {
  const spawnFn = opts.spawnFn ?? spawn;
  const closeTimeoutMs = opts.closeTimeoutMs ?? 15000;
  const open = new Map<string, ChildProcess>();

  return {
    async open(accountId, userDataDir, url) {
      if (open.has(accountId)) return;
      const profile = firefoxProfileFor(userDataDir);
      mkdirSync(profile, { recursive: true, mode: 0o700 });
      writeFileSync(join(profile, "user.js"), `${PROFILE_PREFS}\n`, { mode: 0o600 });
      rmSync(join(profile, "lock"), { force: true });
      rmSync(join(profile, ".parentlock"), { force: true });
      const child = spawnFn(opts.executable, ["--profile", profile, "--no-remote", url], {
        stdio: "ignore",
        env: process.env,
      });
      child.once("exit", () => open.delete(accountId));
      open.set(accountId, child);
    },
    isOpen(accountId) {
      return open.has(accountId);
    },
    async close(accountId) {
      const child = open.get(accountId);
      if (!child) return;
      const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
      child.kill("SIGTERM");
      const timer = new Promise<"timeout">((r) => setTimeout(() => r("timeout"), closeTimeoutMs));
      if ((await Promise.race([exited, timer])) === "timeout") {
        child.kill("SIGKILL");
        await exited;
      }
      open.delete(accountId);
    },
  };
}

export interface ImportableCookie {
  name: string;
  value: string;
  domain: string;
  path: string;
  expires: number;
  secure: boolean;
  httpOnly: boolean;
  sameSite: "Strict" | "Lax" | "None";
}

const MARKER = ".allternit-firefox-import";
const SAME_SITE = ["None", "Lax", "Strict"] as const;

// Reads the Firefox profile's cookies (a copy, with its WAL, so a live
// database is never touched) as Playwright cookies. Expired cookies are
// dropped. Firefox stores expiry in seconds (older) or ms (newer).
export function readFirefoxCookies(firefoxProfile: string, nowS = Date.now() / 1000): ImportableCookie[] {
  const src = join(firefoxProfile, "cookies.sqlite");
  if (!existsSync(src)) return [];
  const tmp = mkdtempSync(join(tmpdir(), "ffck-"));
  try {
    const dst = join(tmp, "cookies.sqlite");
    copyFileSync(src, dst);
    for (const ext of ["-wal", "-shm"]) {
      if (existsSync(src + ext)) copyFileSync(src + ext, dst + ext);
    }
    const db = new Database(dst, { readonly: true });
    try {
      const rows = db
        .prepare(
          "SELECT name, value, host, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies"
        )
        .all() as Array<{
        name: string;
        value: string;
        host: string;
        path: string;
        expiry: number;
        isSecure: number;
        isHttpOnly: number;
        sameSite: number;
      }>;
      const out: ImportableCookie[] = [];
      for (const r of rows) {
        let expires = Number(r.expiry);
        if (expires > 1e11) expires = expires / 1000;
        if (expires > 0 && expires <= nowS) continue;
        const sameSite = SAME_SITE[r.sameSite] ?? "Lax";
        out.push({
          name: r.name,
          value: r.value,
          domain: r.host,
          path: r.path || "/",
          expires: expires > 0 ? expires : -1,
          // Chrome rejects SameSite=None without Secure.
          secure: r.isSecure === 1 || sameSite === "None",
          httpOnly: r.isHttpOnly === 1,
          sameSite,
        });
      }
      return out;
    } finally {
      db.close();
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

// Imports the Firefox session into a Chrome context only when Firefox's
// cookie store changed since the last import, so a stale Firefox copy never
// overwrites a session Chrome has since refreshed. Returns cookies imported.
export async function importFirefoxSessionIfNewer(
  userDataDir: string,
  context: { addCookies(cookies: ImportableCookie[]): Promise<void> }
): Promise<number> {
  const profile = firefoxProfileFor(userDataDir);
  const db = join(profile, "cookies.sqlite");
  if (!existsSync(db)) return 0;
  const mtime = Math.max(
    statSync(db).mtimeMs,
    existsSync(`${db}-wal`) ? statSync(`${db}-wal`).mtimeMs : 0
  );
  const marker = join(userDataDir, MARKER);
  const last = existsSync(marker) ? Number(readFileSync(marker, "utf8")) : 0;
  if (mtime <= last) return 0;
  const cookies = readFirefoxCookies(profile);
  if (cookies.length > 0) await context.addCookies(cookies);
  writeFileSync(marker, String(mtime), { mode: 0o600 });
  return cookies.length;
}
