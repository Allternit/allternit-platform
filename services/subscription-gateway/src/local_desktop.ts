// The gateway inside Allternit Desktop on the person's own computer
// (SUBS_GATEWAY_LOCAL_DESKTOP=1, set by Desktop's subs-gateway-manager.ts).
//
// Eoj, 2026-10-17: browser sign-ins used to be Sessions-computer only (D15)
// because a browser that opens on the computer you work on takes over the
// screen. People may still want browser lanes there. So on this computer:
// - browser lanes are off until the person turns them on (this file),
// - when on, they run in the installed Google Chrome on its own profile,
//   hidden (headless, no window, no Dock icon; worker/hidden_chrome.ts),
// - a window opens only when the person clicks: to sign in or clear a check.
// Phone and desktop-app lanes are not affected.
//
// The switch is a small file in the state dir, read on each check, so a
// change from Settings applies to the next turn without a restart.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export interface BrowserLanesSetting {
  /** This gateway runs on the person's own computer. */
  local_desktop: boolean;
  /** Browser lanes may run here. Always true off the person's computer (a Sessions computer). */
  enabled: boolean;
  changed_at: string | null;
}

export const BROWSER_LANES_OFF =
  "Browser sign-ins are off on this computer. Turn them on in Settings › Subscriptions, or use a Sessions computer.";
export const BROWSER_LANES_NEED_CHROME =
  "Browser sign-ins on this computer need Google Chrome. Install it, or use a Sessions computer.";

export function isLocalDesktop(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.SUBS_GATEWAY_LOCAL_DESKTOP === "1";
}

function settingFile(stateDir: string): string {
  return join(stateDir, "browser-lanes.json");
}

export function readBrowserLanes(stateDir: string, env: NodeJS.ProcessEnv = process.env): BrowserLanesSetting {
  if (!isLocalDesktop(env)) return { local_desktop: false, enabled: true, changed_at: null };
  const file = settingFile(stateDir);
  if (!existsSync(file)) return { local_desktop: true, enabled: false, changed_at: null };
  try {
    const raw = JSON.parse(readFileSync(file, "utf8")) as { enabled?: unknown; changed_at?: unknown };
    return {
      local_desktop: true,
      enabled: raw.enabled === true,
      changed_at: typeof raw.changed_at === "string" ? raw.changed_at : null,
    };
  } catch {
    // An unreadable file never turns browsers on.
    return { local_desktop: true, enabled: false, changed_at: null };
  }
}

export function writeBrowserLanes(stateDir: string, enabled: boolean, now = new Date()): BrowserLanesSetting {
  mkdirSync(stateDir, { recursive: true, mode: 0o700 });
  const file = settingFile(stateDir);
  const tmp = `${file}.tmp`;
  const body = { enabled, changed_at: now.toISOString() };
  writeFileSync(tmp, JSON.stringify(body) + "\n", { mode: 0o600 });
  renameSync(tmp, file);
  return { local_desktop: true, ...body };
}

export function browserLanesEnabled(stateDir: string, env: NodeJS.ProcessEnv = process.env): boolean {
  return readBrowserLanes(stateDir, env).enabled;
}

// The router's view (router/snapshot.ts) has no config; main.ts installs the
// check for this gateway's state dir. Default: routable (Sessions computers, tests).
let routableCheck: () => boolean = () => true;

export function setBrowserLanesCheck(check: () => boolean): void {
  routableCheck = check;
}

export function browserLanesRoutable(): boolean {
  try {
    return routableCheck();
  } catch {
    return false;
  }
}
