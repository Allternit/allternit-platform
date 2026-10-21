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
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import Database from "better-sqlite3";

// The Firefox profile that belongs to a Chrome user-data dir.
export function firefoxProfileFor(userDataDir: string): string {
  return `${userDataDir}-firefox`;
}

export interface SessionStorageKey {
  origin: string;
  key: string;
}

// Chrome keeps localStorage in a LevelDB under Default/Local Storage/leveldb;
// each write is appended to the .log as `_<origin>\0\x01<key>` + value. The
// newest record's bytes are hashed, so a new sign-in (a new token) shows up
// as a changed fingerprint. The value is never kept, logged or decoded.
export function readChromeStorageMarkers(userDataDir: string, entries: SessionStorageKey[]): Map<string, string> {
  const out = new Map<string, string>();
  const dir = join(userDataDir, "Default", "Local Storage", "leveldb");
  let files: string[];
  try {
    files = readdirSync(dir).filter((f) => f.endsWith(".log") || f.endsWith(".ldb"));
  } catch {
    return out;
  }
  // Oldest first, so the newest record wins (.log holds the latest writes).
  const ordered = files
    .map((f) => ({ f, t: statSync(join(dir, f)).mtimeMs }))
    .sort((a, b) => a.t - b.t || (a.f.endsWith(".log") ? 1 : -1))
    .map((x) => readFileSync(join(dir, x.f)));
  for (const { origin, key } of entries) {
    const needle = Buffer.concat([Buffer.from(`_${origin.replace(/\/$/, "")}`), Buffer.from([0, 1]), Buffer.from(key)]);
    let latest: Buffer | null = null;
    for (const buf of ordered) {
      const at = buf.lastIndexOf(needle);
      if (at >= 0) latest = buf.subarray(at + needle.length, at + needle.length + 256);
    }
    if (latest) out.set(`${origin} ${key}`, createHash("sha256").update(latest).digest("hex"));
  }
  return out;
}

export interface LoginBrowser {
  /** The profile the login browser signs in on, for an account's Chrome user-data dir. */
  profileFor(userDataDir: string): string;
  /** That profile's cookies (for sign-in detection). */
  readCookies(profile: string): ImportableCookie[];
  /** Fingerprints of localStorage session keys (sign-in detection); absent when unsupported. */
  readStorage?(profile: string, entries: SessionStorageKey[]): Map<string, string>;
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
    profileFor: firefoxProfileFor,
    readCookies: (profile) => readFirefoxCookies(profile),
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

// Login in a plain Google Chrome window — no automation flags, no DevTools
// port — on the account's own Chrome user-data dir, the one the adapter's
// automated Chrome uses. Google sign-in and Cloudflare see an ordinary
// Chrome, and nothing is copied afterwards: the adapter relaunches on the
// same profile. --password-store=basic matches Playwright's launch so both
// read the same cookie encryption.
export interface ChromeLoginBrowserOptions {
  executable: string;
  spawnFn?: typeof spawn;
  closeTimeoutMs?: number;
  // Chrome refuses to start as root without --no-sandbox (Sessions machines
  // run the gateway as root). Defaults to the process's own uid.
  isRoot?: boolean;
}

export function createChromeLoginBrowser(opts: ChromeLoginBrowserOptions): LoginBrowser {
  const spawnFn = opts.spawnFn ?? spawn;
  const closeTimeoutMs = opts.closeTimeoutMs ?? 15000;
  const isRoot = opts.isRoot ?? process.getuid?.() === 0;
  const open = new Map<string, ChildProcess>();

  return {
    profileFor: (userDataDir) => userDataDir,
    readCookies: (profile) => readChromeCookies(profile),
    readStorage: (profile, entries) => readChromeStorageMarkers(profile, entries),
    async open(accountId, userDataDir, url) {
      if (open.has(accountId)) return;
      mkdirSync(userDataDir, { recursive: true, mode: 0o700 });
      // A killed Chrome leaves its singleton lock behind; the adapter's
      // Chrome is already closed for this account.
      for (const lock of ["SingletonLock", "SingletonCookie", "SingletonSocket"]) {
        rmSync(join(userDataDir, lock), { force: true });
      }
      const child = spawnFn(
        opts.executable,
        [
          `--user-data-dir=${userDataDir}`,
          "--password-store=basic",
          ...(isRoot ? ["--no-sandbox"] : []),
          "--no-first-run",
          "--no-default-browser-check",
          "--hide-crash-restore-bubble",
          "--start-maximized",
          "--new-window",
          url,
        ],
        { stdio: "ignore", env: process.env }
      );
      child.once("exit", () => open.delete(accountId));
      open.set(accountId, child);
    },
    isOpen(accountId) {
      return open.has(accountId);
    },
    async close(accountId) {
      const child = open.get(accountId);
      if (!child) return;
      // SIGTERM: Chrome shuts down cleanly and flushes its cookie store.
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

// Chrome's cookies for sign-in detection only (a copy, so the live database
// is never touched). Values are encrypted on disk; `value` is the encrypted
// blob in hex — it changes when the cookie changes, which is all the login
// watcher compares. Never logged, never imported anywhere.
export function readChromeCookies(userDataDir: string, nowS = Date.now() / 1000): ImportableCookie[] {
  const src = [join(userDataDir, "Default", "Network", "Cookies"), join(userDataDir, "Default", "Cookies")].find(
    (p) => existsSync(p)
  );
  if (!src) return [];
  const tmp = mkdtempSync(join(tmpdir(), "chck-"));
  try {
    const dst = join(tmp, "Cookies");
    copyFileSync(src, dst);
    for (const ext of ["-wal", "-journal"]) {
      if (existsSync(src + ext)) copyFileSync(src + ext, dst + ext);
    }
    const db = new Database(dst, { readonly: true });
    try {
      const rows = db
        .prepare(
          "SELECT name, value, encrypted_value, host_key, path, expires_utc, is_secure, is_httponly FROM cookies"
        )
        .all() as Array<{
        name: string;
        value: string;
        encrypted_value: Buffer | null;
        host_key: string;
        path: string;
        expires_utc: number;
        is_secure: number;
        is_httponly: number;
      }>;
      const out: ImportableCookie[] = [];
      for (const r of rows) {
        // Chrome time: microseconds since 1601-01-01; 0 = session cookie.
        const expires = Number(r.expires_utc) > 0 ? Number(r.expires_utc) / 1e6 - 11644473600 : -1;
        if (expires > 0 && expires <= nowS) continue;
        out.push({
          name: r.name,
          value: r.value || (r.encrypted_value ? Buffer.from(r.encrypted_value).toString("hex") : ""),
          domain: r.host_key,
          path: r.path || "/",
          expires,
          secure: r.is_secure === 1,
          httpOnly: r.is_httponly === 1,
          sameSite: "Lax",
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
