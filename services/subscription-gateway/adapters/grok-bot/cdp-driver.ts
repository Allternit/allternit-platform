// Live driver: attaches to Grok Bot's Electron renderer over local CDP (--remote-debugging-port, loopback only).
// UNVERIFIED LIVE: written from the 0.61.0 bundle; needs one user-consented session to confirm (README).
// Never attaches without an explicit port, never quits/relaunches a running app, never touches its user-data dir.
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { fail, ok, type AaiResult } from "@allternit/agent-gateway";
import { APP_NAME } from "./manifest.js";
import { DriverError, type ClickOptions, type GrokDriver } from "./driver.js";
import { SELECTORS } from "./selectors.js";

const pexec = promisify(execFile);
const LOOPBACK = new Set(["127.0.0.1", "localhost", "::1"]);

export interface CdpOptions { port: number; host?: string; timeoutMs?: number }
interface Pending { resolve(v: unknown): void; reject(e: Error): void }

export class CdpGrokDriver implements GrokDriver {
  private ws: WebSocket | null = null;
  private seq = 0;
  private pending = new Map<number, Pending>();
  constructor(private o: CdpOptions) {
    if (!LOOPBACK.has(o.host ?? "127.0.0.1")) throw new Error("CdpGrokDriver only attaches to loopback");
  }
  private get base() { return `http://${this.o.host ?? "127.0.0.1"}:${this.o.port}`; }

  async isAppRunning(): Promise<boolean> {
    try { await pexec("pgrep", ["-x", APP_NAME]); return true; } catch { return false; }
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
    const page = targets.find((t) => t.type === "page" && /renderer\/index\.html/.test(t.url)) ?? targets.find((t) => t.type === "page" && t.title === APP_NAME);
    if (!page?.webSocketDebuggerUrl) throw new DriverError("unreachable", `${APP_NAME} main window not found on the debugging port.`);
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
  private async evalJs<T>(expression: string, awaitPromise = false): Promise<T> {
    const r = await this.send<{ result: { value: T }; exceptionDetails?: unknown }>("Runtime.evaluate", { expression, returnByValue: true, awaitPromise });
    if (r.exceptionDetails) throw new Error("page evaluation failed");
    return r.result.value;
  }
  async html(): Promise<string> { await this.connect(); return this.evalJs<string>("document.documentElement.outerHTML"); }
  async avatars(): Promise<BotMark[]> { await this.connect(); return this.evalJs<BotMark[]>(AVATARS_JS, true); }
  async newChat(): Promise<boolean> { return this.clickButton("^new chat$"); }
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
      for(const b of document.querySelectorAll('button,[role=button]')){const n=(b.getAttribute('aria-label')||b.textContent||'').trim();
        if(!re.test(n)||b.disabled)continue;
        if(w){let p=b.parentElement,hit=false;while(p&&p!==document.body){if(w.test(p.textContent||'')){hit=true;break}p=p.parentElement}if(!hit)continue}
        b.click();return true}return false})()`);
  }
  async dispose(): Promise<void> { try { this.ws?.close(); } catch { /* ignore */ } this.ws = null; }
}

export interface LaunchDeps {
  isRunning(): Promise<boolean>;
  launch(port: number): Promise<void>;
}
const realDeps: LaunchDeps = {
  async isRunning() { try { await pexec("pgrep", ["-x", APP_NAME]); return true; } catch { return false; } },
  async launch(port) {
    // LaunchServices start (keeps the app's normal identity); loopback is Electron's default bind for this flag.
    spawn("open", ["-a", APP_NAME, "--args", `--remote-debugging-port=${port}`], { detached: true, stdio: "ignore" }).unref();
  },
};

/** Start Grok Bot with a debugging port. Consent-gated; never terminates a running instance. */
export async function launchWithDebugPort(
  input: { userConsented?: boolean; port: number },
  deps: LaunchDeps = realDeps,
): Promise<AaiResult<{ port: number }>> {
  if (input.userConsented !== true)
    return fail("POLICY_DENIED", `Starting ${APP_NAME} with a local debugging port needs your explicit consent in the connection wizard.`);
  if (await deps.isRunning())
    return fail("LANE_BLOCKED", `${APP_NAME} is already running without a debugging port. Please quit ${APP_NAME} yourself, then connect again. Allternit will not close it for you.`, { retryable: false });
  await deps.launch(input.port);
  return ok({ port: input.port });
}

/**
 * Grok Bot draws each Bot's mascot as an <svg><use href="#sand-agent-mark-source-<id>"> whose source is styled by
 * CSS variables. Clone the source, bake each element's resolved style in, draw it to a canvas and export PNG, so
 * the avatar leaves the page as a plain raster image (never svg). Read-only: nothing is clicked or typed.
 */
export type BotMark = { name?: string; text: string; png: string };
export const AVATARS_JS = `(async () => {
  const PROPS = ['fill','fill-opacity','stroke','stroke-width','stroke-opacity','opacity','display','visibility','stroke-linecap','stroke-linejoin'];
  const out = [];
  for (const item of document.querySelectorAll('button.sand-agent-item')) {
    const use = item.querySelector('svg use');
    const ref = use && (use.getAttribute('href') || use.getAttribute('xlink:href'));
    const src = ref && ref.startsWith('#sand-agent-mark-source-') && document.getElementById(ref.slice(1));
    if (!src) continue;
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
    svg.setAttribute('viewBox', item.querySelector('svg').getAttribute('viewBox') || '-15 -15 259 259');
    svg.setAttribute('width', '128'); svg.setAttribute('height', '128');
    const clone = src.cloneNode(true); clone.removeAttribute('id');
    const orig = [src, ...src.querySelectorAll('*')], copy = [clone, ...clone.querySelectorAll('*')];
    orig.forEach((o, i) => { const cs = getComputedStyle(o); copy[i].setAttribute('style', PROPS.map((p) => p + ':' + cs.getPropertyValue(p)).join(';')); });
    svg.appendChild(clone);
    const img = new Image();
    img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(new XMLSerializer().serializeToString(svg));
    try { await img.decode(); } catch { continue; }
    const c = document.createElement('canvas'); c.width = c.height = 128;
    c.getContext('2d').drawImage(img, 0, 0, 128, 128);
    const w = document.createTreeWalker(item, NodeFilter.SHOW_TEXT);
    let t; while ((t = w.nextNode()) && !t.textContent.trim());
    out.push({ name: t ? t.textContent.trim() : '', text: (item.textContent || '').trim(), png: c.toDataURL('image/png') });
  }
  return out;
})()`;
