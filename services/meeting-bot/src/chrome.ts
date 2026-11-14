// Launch a normal Chrome and drive it over its DevTools port (spike/join.ts).
//
// Meet refuses browsers in automation mode ("You can't join this video call";
// spike result 1), so Chrome starts without --enable-automation and Playwright
// attaches with connectOverCDP. The bot still joins under its own name, shows
// its own tile and posts the consent notice. No evasion beyond that.

import { chromium, type Browser, type BrowserContext, type Page } from "playwright-core";
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface ChromeOptions {
  chromePath?: string;
  headless?: boolean;
  /** Persistent profile (the signed-in bot account). Absent = a temp profile deleted on close. */
  profileDir?: string;
  chromeArgs?: string[];
  onEvent?: (kind: string, detail?: unknown) => void;
}

export interface ChromeSession {
  chrome: ChildProcess;
  browser: Browser;
  context: BrowserContext;
  page: Page;
  profileDir: string;
  close(): Promise<void>;
}

export function defaultChromePath(): string {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  if (process.platform === "darwin") return "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  return "/usr/bin/google-chrome";
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export const BASE_CHROME_ARGS = [
  "--autoplay-policy=no-user-gesture-required", // our AudioContexts and the call audio run without a click
  "--use-fake-ui-for-media-stream", // no permission bubble; our getUserMedia override never opens a device
  "--disable-background-timer-throttling",
  "--disable-renderer-backgrounding",
  "--disable-backgrounding-occluded-windows", // the tile window keeps animating behind the meeting window
  "--force-color-profile=srgb",
  "--window-size=1280,800",
  "--no-first-run",
  "--no-default-browser-check",
  "--lang=en-US",
  // macOS dev runs: without it every launch leaves a code_sign_clone copy in $TMPDIR (workspace AGENTS.md).
  "--disable-features=MacAppCodeSignClone",
];

export async function launchChrome(opts: ChromeOptions = {}): Promise<ChromeSession> {
  const log = opts.onEvent ?? (() => undefined);
  const ownsProfile = !opts.profileDir;
  const profileDir = opts.profileDir ?? mkdtempSync(join(tmpdir(), "gizzi-pod-profile-"));
  // A stale port file from a crashed run would point at a dead port.
  rmSync(join(profileDir, "DevToolsActivePort"), { force: true });
  const args = [
    ...BASE_CHROME_ARGS,
    `--user-data-dir=${profileDir}`,
    "--remote-debugging-port=0",
    ...(opts.headless ? ["--headless=new"] : []),
    // Pods have no OS keyring: the bot profile's cookies must stay readable on
    // any pod host, as signed in (docs/meetings/BOT-ACCOUNTS.md).
    ...(process.platform === "linux" ? ["--password-store=basic"] : []),
    // Chrome's own sandbox needs an unprivileged user namespace, which an
    // unprivileged Incus pod refuses ("credentials.cc ... Permission denied"),
    // so Chrome dies at start. The single-use, unprivileged pod is the
    // isolation boundary instead, as for the cloud computers' Chrome; the image
    // sets ALLTERNIT_CHROME_NO_SANDBOX=1 on the bot unit.
    ...(process.getuid?.() === 0 || process.env.ALLTERNIT_CHROME_NO_SANDBOX === "1" ? ["--no-sandbox"] : []),
    ...(opts.chromeArgs ?? []),
    "about:blank",
  ];
  const chromePath = opts.chromePath ?? defaultChromePath();
  const chrome = spawn(chromePath, args, { stdio: "ignore" });
  let exited = false;
  chrome.once("exit", (code, signal) => {
    exited = true;
    log("chrome_exit", { code, signal });
  });
  const portFile = join(profileDir, "DevToolsActivePort");
  for (let i = 0; i < 150 && !existsSync(portFile) && !exited; i++) await sleep(100);
  if (!existsSync(portFile)) {
    chrome.kill("SIGKILL");
    throw new Error(`chrome did not open a DevTools port (${chromePath})`);
  }
  const port = readFileSync(portFile, "utf8").split("\n")[0].trim();
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const context = browser.contexts()[0];
  const page = context.pages()[0] ?? (await context.newPage());
  log("chrome_ready", { port, headless: Boolean(opts.headless), profile: ownsProfile ? "temp" : "persistent" });

  let closed = false;
  return {
    chrome,
    browser,
    context,
    page,
    profileDir,
    async close() {
      if (closed) return;
      closed = true;
      // Graceful first (a signed-in profile must flush its cookies), bounded.
      await Promise.race([browser.close().catch(() => undefined), sleep(5000)]);
      for (let i = 0; i < 20 && !exited; i++) await sleep(100);
      if (!exited) {
        chrome.kill("SIGTERM");
        for (let i = 0; i < 30 && !exited; i++) await sleep(100);
        if (!exited) chrome.kill("SIGKILL");
      }
      if (ownsProfile) rmSync(profileDir, { recursive: true, force: true });
    },
  };
}
