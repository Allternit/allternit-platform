// Browser lanes on the person's own computer (local_desktop.ts): the installed
// Google Chrome, on the account's own profile, with no window. Never Chrome
// for Testing: OpenAI geoblocks it and it draws far more checks (Eoj,
// 2026-10-17). Never the person's own Chrome profile.
//
// Playwright's own launch adds automation switches (--enable-automation and
// friends) that vendors detect, so Chrome is started here the way the ChatGPT
// image lane on Eoj's Mac starts it (headless=new, automation features off,
// a debugging port) and Playwright attaches over CDP.
//
// Headless Chrome says "HeadlessChrome" in its user agent, and Cloudflare
// holds such a browser on "Just a moment…" (checked live on chatgpt.com,
// 2026-10-17). The same Chrome's normal user agent passes, so the launch sets
// it from the installed version. The client-hint brands already read
// "Google Chrome" in headless=new.
//
// A window never opens from here. When a vendor shows a check, the account
// reports it (challenge_presented) and the person clears it from Settings,
// which opens the login browser on the same profile — on their click.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import type { Browser, BrowserContext } from "playwright";

export interface HiddenChrome {
  context: BrowserContext;
  /** Resolves when Chrome goes away (crash, kill, or close()). */
  closed: Promise<void>;
  close(): Promise<void>;
}

export interface HiddenChromeOptions {
  /** The installed Google Chrome binary. */
  executable: string;
  connect?: (endpoint: string) => Promise<Browser>;
  spawnFn?: typeof spawn;
  /** How long to wait for DevToolsActivePort (default 30 s). */
  startTimeoutMs?: number;
  isRoot?: boolean;
  /** Override the detected user agent (tests). */
  userAgent?: string | null;
}

const majorCache = new Map<string, number | null>();

// The installed Google Chrome, never Chrome for Testing or Chromium.
export function detectGoogleChrome(
  platform: NodeJS.Platform = process.platform,
  env: NodeJS.ProcessEnv = process.env,
  exists: (p: string) => boolean = existsSync
): string | null {
  const candidates =
    platform === "darwin"
      ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
      : platform === "win32"
        ? [
            env.PROGRAMFILES && join(env.PROGRAMFILES, "Google", "Chrome", "Application", "chrome.exe"),
            env["PROGRAMFILES(X86)"] && join(env["PROGRAMFILES(X86)"], "Google", "Chrome", "Application", "chrome.exe"),
            env.LOCALAPPDATA && join(env.LOCALAPPDATA, "Google", "Chrome", "Application", "chrome.exe"),
          ]
        : ["/usr/bin/google-chrome-stable", "/usr/bin/google-chrome", "/opt/google/chrome/chrome"];
  return (candidates.filter(Boolean) as string[]).find((c) => exists(c)) ?? null;
}

/** The installed Chrome's major version, or null. Windows' chrome.exe prints nothing, so read its version folder. */
export function chromeMajorVersion(
  executable: string,
  platform: NodeJS.Platform = process.platform,
  run: (exe: string) => string = (exe) => execFileSync(exe, ["--version"], { encoding: "utf8", timeout: 10_000 }),
  listDir: (dir: string) => string[] = (dir) => readdirSync(dir)
): number | null {
  try {
    const text = platform === "win32" ? listDir(dirname(executable)).filter((d) => /^\d+\.\d+\./.test(d)).sort().pop() ?? "" : run(executable);
    const m = /(\d+)\.\d+\.\d+/.exec(text);
    return m ? Number(m[1]) : null;
  } catch {
    return null;
  }
}

/** Chrome's own reduced user agent for this platform (what a windowed Chrome sends). */
export function normalUserAgent(major: number, platform: NodeJS.Platform = process.platform): string {
  const os =
    platform === "darwin"
      ? "Macintosh; Intel Mac OS X 10_15_7"
      : platform === "win32"
        ? "Windows NT 10.0; Win64; x64"
        : "X11; Linux x86_64";
  return `Mozilla/5.0 (${os}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${major}.0.0.0 Safari/537.36`;
}

export function hiddenChromeArgs(userDataDir: string, isRoot: boolean, userAgent?: string | null): string[] {
  return [
    "--headless=new",
    `--user-data-dir=${userDataDir}`,
    // Port 0: Chrome picks one and writes it to DevToolsActivePort.
    "--remote-debugging-port=0",
    "--remote-debugging-address=127.0.0.1",
    // Same store as the login browser, so the cookies it wrote decrypt here.
    "--password-store=basic",
    ...(isRoot ? ["--no-sandbox"] : []),
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-blink-features=AutomationControlled",
    "--hide-crash-restore-bubble",
    "--window-size=1400,1000",
    ...(userAgent ? [`--user-agent=${userAgent}`] : []),
    "about:blank",
  ];
}

async function defaultConnect(endpoint: string): Promise<Browser> {
  const { chromium } = await import("playwright");
  return chromium.connectOverCDP(endpoint);
}

export async function launchHiddenChrome(userDataDir: string, opts: HiddenChromeOptions): Promise<HiddenChrome> {
  const spawnFn = opts.spawnFn ?? spawn;
  const isRoot = opts.isRoot ?? process.getuid?.() === 0;
  mkdirSync(userDataDir, { recursive: true, mode: 0o700 });
  const portFile = join(userDataDir, "DevToolsActivePort");
  rmSync(portFile, { force: true });
  // A killed Chrome leaves its singleton lock behind.
  for (const lock of ["SingletonLock", "SingletonCookie", "SingletonSocket"]) rmSync(join(userDataDir, lock), { force: true });

  let userAgent = opts.userAgent;
  if (userAgent === undefined) {
    if (!majorCache.has(opts.executable)) majorCache.set(opts.executable, chromeMajorVersion(opts.executable));
    const major = majorCache.get(opts.executable);
    userAgent = major ? normalUserAgent(major) : null;
  }
  const child: ChildProcess = spawnFn(opts.executable, hiddenChromeArgs(userDataDir, isRoot, userAgent), {
    stdio: "ignore",
    env: process.env,
    windowsHide: true,
  });
  let exited = false;
  const closed = new Promise<void>((resolve) => {
    child.once("exit", () => {
      exited = true;
      resolve();
    });
    child.once("error", () => {
      exited = true;
      resolve();
    });
  });

  const deadline = Date.now() + (opts.startTimeoutMs ?? 30_000);
  let port: number | null = null;
  while (Date.now() < deadline && !exited) {
    if (existsSync(portFile)) {
      const first = readFileSync(portFile, "utf8").split("\n")[0]?.trim();
      const n = Number(first);
      if (Number.isInteger(n) && n > 0) {
        port = n;
        break;
      }
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  if (port === null) {
    child.kill();
    throw new Error(exited ? "Google Chrome exited before it started" : "Google Chrome did not start in time");
  }

  let browser: Browser;
  try {
    browser = await (opts.connect ?? defaultConnect)(`http://127.0.0.1:${port}`);
  } catch (err) {
    child.kill();
    throw err;
  }
  const context = browser.contexts()[0];
  if (!context) {
    child.kill();
    throw new Error("Google Chrome started without a browser context");
  }
  return {
    context,
    closed,
    async close() {
      await browser.close().catch(() => {});
      if (!exited) child.kill();
      await closed;
    },
  };
}
