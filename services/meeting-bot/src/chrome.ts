// Launch a normal Chrome and drive it over its DevTools port (spike/join.ts).
//
// Meet refuses browsers in automation mode ("You can't join this video call";
// spike result 1), so Chrome starts without --enable-automation and Playwright
// attaches with connectOverCDP. The bot still joins under its own name, shows
// its own tile and posts the consent notice. No evasion beyond that.

import { chromium, type Browser, type BrowserContext, type Page } from "playwright-core";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface ChromeOptions {
  chromePath?: string;
  /** Legacy switch: true = windowMode "headless". */
  headless?: boolean;
  /** Overrides MEETING_BOT_WINDOW. Default: resolveWindowMode(). */
  windowMode?: WindowMode;
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

export type WindowMode = "headless" | "offscreen" | "normal";

export interface ProbeEnv {
  platform: NodeJS.Platform;
  env: Record<string, string | undefined>;
  exists: (path: string) => boolean;
}

const realProbeEnv = (): ProbeEnv => ({ platform: process.platform, env: process.env, exists: existsSync });

/** MEETING_BOT_WINDOW, else "normal" on a bootstrapped cloud computer with a display, else "headless". */
export function resolveWindowMode(p: ProbeEnv = realProbeEnv()): WindowMode {
  const v = p.env.MEETING_BOT_WINDOW;
  if (v === "headless" || v === "offscreen" || v === "normal") return v;
  return p.env.DISPLAY && p.exists("/etc/allternit/bootstrap.json") ? "normal" : "headless";
}

/** Candidate Chrome-family binaries in preference order: Chrome, Edge, Chromium. */
export function chromeCandidates(p: ProbeEnv): string[] {
  if (p.platform === "darwin") {
    return [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
  }
  if (p.platform === "win32") {
    const roots = [p.env.ProgramFiles, p.env["ProgramFiles(x86)"], p.env.LOCALAPPDATA].filter((r): r is string => Boolean(r));
    const rel = (r: string, sub: string) => `${r}\\${sub}`;
    return [
      ...roots.map((r) => rel(r, "Google\\Chrome\\Application\\chrome.exe")),
      ...roots.map((r) => rel(r, "Microsoft\\Edge\\Application\\msedge.exe")),
      ...roots.map((r) => rel(r, "Chromium\\Application\\chrome.exe")),
    ];
  }
  const dirs = (p.env.PATH ?? "/usr/local/bin:/usr/bin:/bin").split(":").filter(Boolean);
  const onPath = (names: string[]) => names.flatMap((n) => dirs.map((d) => `${d}/${n}`));
  return [
    ...onPath(["google-chrome", "google-chrome-stable"]),
    "/opt/google/chrome/chrome",
    ...onPath(["microsoft-edge", "microsoft-edge-stable"]),
    ...onPath(["chromium", "chromium-browser"]),
  ];
}

/** CHROME_PATH, else the first installed Chrome, Edge, then Chromium; null when none is found. */
export function findChrome(p: ProbeEnv = realProbeEnv()): string | null {
  if (p.env.CHROME_PATH) return p.env.CHROME_PATH;
  return chromeCandidates(p).find((c) => p.exists(c)) ?? null;
}

export function defaultChromePath(): string {
  return findChrome() ?? (process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "/usr/bin/google-chrome");
}

/** A normal desktop Chrome user agent (headless Chrome says "HeadlessChrome"). */
export function desktopUserAgent(platform: NodeJS.Platform, version = "131.0.0.0"): string {
  const os =
    platform === "darwin" ? "Macintosh; Intel Mac OS X 10_15_7" : platform === "win32" ? "Windows NT 10.0; Win64; x64" : "X11; Linux x86_64";
  return `Mozilla/5.0 (${os}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${version} Safari/537.36`.replace(/HeadlessChrome/g, "Chrome");
}

/** Window-mode flags. `ua` is the browser's own UA if known (HeadlessChrome is stripped). */
export function windowModeArgs(mode: WindowMode, platform: NodeJS.Platform, ua?: string): string[] {
  if (mode === "headless") {
    const agent = ua ? ua.replace(/HeadlessChrome/g, "Chrome") : desktopUserAgent(platform);
    return [
      "--headless=new",
      `--user-agent=${agent}`,
      "--mute-audio",
      ...(platform === "darwin" ? ["--use-mock-keychain"] : []),
    ];
  }
  if (mode === "offscreen") return ["--window-position=-32000,-32000", "--start-minimized", "--mute-audio"];
  return [];
}

function chromeMajorVersion(chromePath: string): string | undefined {
  if (process.platform === "win32") return undefined;
  try {
    const out = execFileSync(chromePath, ["--version"], { timeout: 5000, encoding: "utf8" });
    const m = /(\d+)\.\d+\.\d+\.\d+/.exec(out);
    return m ? `${m[1]}.0.0.0` : undefined;
  } catch {
    return undefined;
  }
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
  // One --disable-features flag only: Chrome keeps the last one it is given.
  // MacAppCodeSignClone: macOS dev runs leave a code_sign_clone copy in $TMPDIR without it (workspace AGENTS.md).
  // WebRtcHideLocalIpsWithMdns: plain host ICE candidates, so the live tile's
  // WebRTC loopback to the meeting tab (live-tile.ts) connects without mDNS,
  // which an unprivileged pod's network may not resolve.
  "--disable-features=MacAppCodeSignClone,WebRtcHideLocalIpsWithMdns",
  // A bot profile captured from a killed Chrome would show "Restore pages?" over
  // Meet's side panel (first live Meet call 2026-11-14: chat_post_failed).
  "--hide-crash-restore-bubble",
  // Suppresses the "unsupported command-line flag: --no-sandbox" infobar, which
  // pushes the meeting page down. Sets no automation signal (navigator.webdriver stays false).
  "--test-type",
];

/**
 * Mark a persistent profile as cleanly shut down, so Chrome never offers to
 * restore pages from a crash (the pod's bot profile is captured after the
 * runner's Chrome was stopped, often by SIGKILL). Best effort: a missing or
 * unreadable file is left alone.
 */
export function markProfileCleanExit(profileDir: string): string[] {
  const touched: string[] = [];
  const patch = (file: string, edit: (j: Record<string, any>) => void) => {
    if (!existsSync(file)) return;
    try {
      const j = JSON.parse(readFileSync(file, "utf8")) as Record<string, any>;
      edit(j);
      writeFileSync(file, JSON.stringify(j));
      touched.push(file);
    } catch {
      /* corrupt or locked: Chrome rewrites it */
    }
  };
  patch(join(profileDir, "Default", "Preferences"), (j) => {
    j.profile = { ...(j.profile ?? {}), exit_type: "Normal", exited_cleanly: true };
  });
  patch(join(profileDir, "Local State"), (j) => {
    j.exited_cleanly = true;
    if (j.profile && typeof j.profile === "object") j.profile.exit_type = "Normal";
  });
  return touched;
}

export async function launchChrome(opts: ChromeOptions = {}): Promise<ChromeSession> {
  const log = opts.onEvent ?? (() => undefined);
  const ownsProfile = !opts.profileDir;
  const profileDir = opts.profileDir ?? mkdtempSync(join(tmpdir(), "gizzi-pod-profile-"));
  // A stale port file from a crashed run would point at a dead port.
  rmSync(join(profileDir, "DevToolsActivePort"), { force: true });
  if (!ownsProfile) {
    const touched = markProfileCleanExit(profileDir);
    if (touched.length) log("chrome_profile_clean_exit", { files: touched.length });
  }
  const chromePath = opts.chromePath ?? defaultChromePath();
  const mode: WindowMode = opts.headless ? "headless" : (opts.windowMode ?? resolveWindowMode());
  const args = [
    ...BASE_CHROME_ARGS,
    `--user-data-dir=${profileDir}`,
    "--remote-debugging-port=0",
    ...windowModeArgs(mode, process.platform, mode === "headless" ? desktopUserAgent(process.platform, chromeMajorVersion(chromePath)) : undefined),
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
  log("chrome_ready", { port, mode, profile: ownsProfile ? "temp" : "persistent" });

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
