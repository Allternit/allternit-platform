// Live driver: attaches to Claude Desktop's claude.ai page over local CDP (--remote-debugging-port, loopback only).
// UNVERIFIED LIVE. Claude Desktop 2.16120 exits at startup when --remote-debugging-port is on the command line unless a valid
// Anthropic-signed CLAUDE_CDP_AUTH token is present (index.pre.js), so this lane is likely LANE_BLOCKED for ordinary users (README).
// Never attaches without an explicit port, never quits/relaunches a running app, never touches its user-data dir.
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { fail, ok, type AaiResult } from "@allternit/agent-gateway";
import { APP_NAME, PROCESS_NAME } from "./manifest.js";
import { DriverError, type ClickOptions, type ClaudeDesktopDriver } from "./driver.js";
import { NAMES, SELECTORS } from "./selectors.js";

const pexec = promisify(execFile);
const LOOPBACK = new Set(["127.0.0.1", "localhost", "::1"]);

export interface CdpOptions { port: number; host?: string; timeoutMs?: number }
interface Pending { resolve(v: unknown): void; reject(e: Error): void }

export class CdpClaudeDesktopDriver implements ClaudeDesktopDriver {
  private ws: WebSocket | null = null;
  private seq = 0;
  private pending = new Map<number, Pending>();
  constructor(private o: CdpOptions) {
    if (!LOOPBACK.has(o.host ?? "127.0.0.1")) throw new Error("CdpClaudeDesktopDriver only attaches to loopback");
  }
  private get base() { return `http://${this.o.host ?? "127.0.0.1"}:${this.o.port}`; }

  async isAppRunning(): Promise<boolean> {
    try { await pexec("pgrep", ["-x", PROCESS_NAME]); return true; } catch { return false; }
  }
  async connect(): Promise<void> {
    if (this.ws && this.ws.readyState === 1) return;
    if (!(await this.isAppRunning())) throw new DriverError("not_running", `${APP_NAME} is not running.`);
    let targets: Array<{ type: string; url: string; title: string; webSocketDebuggerUrl?: string }>;
    try {
      const r = await fetch(`${this.base}/json/list`, { signal: AbortSignal.timeout(this.o.timeoutMs ?? 3000) });
      targets = (await r.json()) as typeof targets;
    } catch {
      throw new DriverError("unreachable", `${APP_NAME} is running but has no debugging port ${this.o.port}. Quit it and let Allternit relaunch it with your consent.`);
    }
    // The chat UI is a remote claude.ai page inside the app; the bundled renderer/*.html pages are shell windows (about, quick, find).
    const page = targets.find((t) => t.type === "page" && /^https:\/\/claude\.ai\//.test(t.url));
    if (!page?.webSocketDebuggerUrl) throw new DriverError("unreachable", `${APP_NAME} claude.ai page not found on the debugging port.`);
    await new Promise<void>((resolve, reject) => {
      const ws = new WebSocket(page.webSocketDebuggerUrl!);
      ws.onopen = () => { this.ws = ws; resolve(); };
      ws.onerror = () => reject(new DriverError("unreachable", "CDP websocket failed"));
      ws.onclose = () => { this.ws = null; for (const p of this.pending.values()) p.reject(new DriverError("unreachable", "CDP closed")); this.pending.clear(); };
      ws.onmessage = (ev) => {
        const m = JSON.parse(String(ev.data)) as { id?: number; result?: unknown; error?: { message: string } };
        const p = m.id !== undefined ? this.pending.get(m.id) : undefined;
        if (!p) return; this.pending.delete(m.id!);
        m.error ? p.reject(new Error(m.error.message)) : p.resolve(m.result);
      };
    });
  }
  private send<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (!this.ws) return Promise.reject(new DriverError("unreachable", "not attached"));
    const id = ++this.seq;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.ws!.send(JSON.stringify({ id, method, params }));
      setTimeout(() => { if (this.pending.delete(id)) reject(new DriverError("unreachable", `CDP ${method} timed out`)); }, this.o.timeoutMs ?? 5000);
    });
  }
  private async evalJs<T>(expression: string): Promise<T> {
    const r = await this.send<{ result: { value: T }; exceptionDetails?: unknown }>("Runtime.evaluate", { expression, returnByValue: true });
    if (r.exceptionDetails) throw new Error("page evaluation failed");
    return r.result.value;
  }
  async html(): Promise<string> { await this.connect(); return this.evalJs<string>("document.documentElement.outerHTML"); }
  async newChat(): Promise<boolean> { return this.clickButton(NAMES.newChat); }
  async typeText(text: string): Promise<boolean> {
    await this.connect();
    const sels = JSON.stringify(SELECTORS.composer.css);
    const focused = await this.evalJs<boolean>(`(()=>{for(const s of ${sels}){const e=document.querySelector(s);if(e){e.focus();document.execCommand('selectAll');document.execCommand('delete');return true}}return false})()`);
    if (!focused) return false;
    await this.send("Input.insertText", { text });
    return true;
  }
  async clickButton(nameSource: string, o?: ClickOptions): Promise<boolean> {
    await this.connect();
    const re = JSON.stringify(nameSource), within = JSON.stringify(o?.withinText ?? "");
    return this.evalJs<boolean>(`(()=>{const re=new RegExp(${re},'i'),w=${within}?new RegExp(${within},'i'):null;
      for(const b of document.querySelectorAll('button,a,[role=button],[role=tab]')){const n=(b.getAttribute('aria-label')||b.textContent||'').trim();
        if(!re.test(n)||b.disabled)continue;
        if(w){let p=b.parentElement,hit=false;while(p&&p!==document.body){if(w.test(p.textContent||'')){hit=true;break}p=p.parentElement}if(!hit)continue}
        b.click();return true}return false})()`);
  }
  async dispose(): Promise<void> { try { this.ws?.close(); } catch { /* ignore */ } this.ws = null; }
}

export interface LaunchDeps {
  isRunning(): Promise<boolean>;
  launch(port: number, auth?: { token: string; userDataDir: string }): Promise<void>;
}
const realDeps: LaunchDeps = {
  async isRunning() { try { await pexec("pgrep", ["-x", PROCESS_NAME]); return true; } catch { return false; } },
  async launch(port, auth) {
    // LaunchServices start (keeps the app's normal identity); loopback is Electron's default bind for this flag.
    const env = auth ? ["--env", `CLAUDE_CDP_AUTH=${auth.token}`, "--env", `CLAUDE_USER_DATA_DIR=${auth.userDataDir}`] : [];
    spawn("open", ["-a", APP_NAME, ...env, "--args", `--remote-debugging-port=${port}`], { detached: true, stdio: "ignore" }).unref();
  },
};

/**
 * Start Claude with a debugging port. Consent-gated; never terminates a running instance.
 * Claude Desktop only honours the flag with a vendor-signed developer token (`CLAUDE_CDP_AUTH`, bound to `CLAUDE_USER_DATA_DIR`).
 * Allternit cannot mint one and does not try to bypass the check: without a token supplied by the user the lane is LANE_BLOCKED.
 * The token is passed through to the child's environment only; it is never logged or stored, and user-data is never read.
 */
export async function launchWithDebugPort(
  input: { userConsented?: boolean; port: number; authToken?: string; userDataDir?: string },
  deps: LaunchDeps = realDeps,
): Promise<AaiResult<{ port: number }>> {
  if (input.userConsented !== true)
    return fail("POLICY_DENIED", `Starting ${APP_NAME} with a local debugging port needs your explicit consent in the connection wizard.`);
  if (!input.authToken || !input.userDataDir)
    return fail("LANE_BLOCKED", `${APP_NAME} refuses to start with a debugging port unless it is given a signed developer token from Anthropic. Without one, this lane is unavailable; use the Claude API (Managed Agents) lane instead.`, { retryable: false });
  if (await deps.isRunning())
    return fail("LANE_BLOCKED", `${APP_NAME} is already running without a debugging port. Please quit ${APP_NAME} yourself, then connect again. Allternit will not close it for you.`, { retryable: false });
  await deps.launch(input.port, { token: input.authToken, userDataDir: input.userDataDir });
  return ok({ port: input.port });
}
