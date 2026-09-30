// AxDriver: typed client for native/ax-bridge (JSON-lines over stdio) + the AxDriver interface the replay driver also implements.
import { spawn as nodeSpawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { AX_SNAPSHOT_FORMAT, AxError, DEFAULT_OBSERVE, type AxEvent, type AxNode, type AxSnapshot } from "./types.js";

export interface AxDriver {
  trust(): Promise<boolean>;
  /** Attach to a running app by bundle id (sets AXManualAccessibility for Electron). Throws AxError not_trusted / not_running. */
  attach(bundleId: string): Promise<{ pid: number; manualAccessibility: boolean }>;
  snapshot(opts?: { maxDepth?: number }): Promise<AxSnapshot>;
  find(q: { role?: string; labelRegex?: string }): Promise<AxNode[]>;
  press(path: number[]): Promise<boolean>;
  setValue(path: number[], value: string): Promise<boolean>;
  focus(path: number[]): Promise<boolean>;
  /** Stream AX notifications; returns an unsubscribe. */
  observe(listener: (e: AxEvent) => void, notifications?: readonly string[]): Promise<() => void>;
  dispose(): Promise<void>;
}

export type SpawnFn = (cmd: string, args: string[]) => ChildProcessWithoutNullStreams;
export interface AxBridgeOptions {
  /** Absolute path of the ax-bridge binary (native/ax-bridge/.build/release/ax-bridge, or the packaged copy). */
  binPath: string;
  requestTimeoutMs?: number;
  spawnFn?: SpawnFn;
}
interface Pending { resolve(v: Record<string, unknown>): void; reject(e: Error): void; timer: ReturnType<typeof setTimeout> }

export class AxBridgeDriver implements AxDriver {
  private child?: ChildProcessWithoutNullStreams;
  private buf = "";
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(e: AxEvent) => void>();
  private bundleId = "";
  constructor(private o: AxBridgeOptions) {}

  private start() {
    if (this.child) return this.child;
    let c: ChildProcessWithoutNullStreams;
    try { c = (this.o.spawnFn ?? ((cmd, args) => nodeSpawn(cmd, args, { stdio: "pipe" })))(this.o.binPath, []); }
    catch (e) { throw new AxError("bridge_unavailable", `Could not start ax-bridge: ${(e as Error).message}`); }
    c.stdout.setEncoding("utf8");
    c.stdout.on("data", (d: string) => this.onData(d));
    c.on("error", (e) => this.die(new AxError("bridge_unavailable", `ax-bridge failed: ${e.message}`)));
    c.on("exit", () => this.die(new AxError("bridge_unavailable", "ax-bridge exited")));
    this.child = c;
    return c;
  }
  private die(err: AxError) {
    this.child = undefined;
    for (const [, p] of this.pending) { clearTimeout(p.timer); p.reject(err); }
    this.pending.clear();
  }
  private onData(d: string) {
    this.buf += d;
    let i: number;
    while ((i = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, i).trim(); this.buf = this.buf.slice(i + 1);
      if (!line) continue;
      let m: Record<string, unknown>;
      try { m = JSON.parse(line); } catch { continue; }
      if (typeof m.event === "string") { for (const l of this.listeners) l(m as unknown as AxEvent); continue; }
      const p = this.pending.get(m.id as number);
      if (p) { this.pending.delete(m.id as number); clearTimeout(p.timer); p.resolve(m); }
    }
  }
  private req(cmd: string, args: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    const child = this.start();
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new AxError("timeout", `ax-bridge ${cmd} timed out`)); }, this.o.requestTimeoutMs ?? 10_000);
      this.pending.set(id, { resolve, reject, timer });
      child.stdin.write(JSON.stringify({ id, cmd, ...args }) + "\n");
    }).then((m) => {
      const r = m as { ok?: boolean; error?: { code?: string; message?: string } };
      if (r.ok === false && r.error) {
        const f = ({ NOT_TRUSTED: "not_trusted", NOT_RUNNING: "not_running", NOT_ATTACHED: "not_attached" } as const)[r.error.code as "NOT_TRUSTED"] ?? "failed";
        throw new AxError(f, r.error.message ?? cmd);
      }
      return m;
    });
  }
  async trust() { return (await this.req("trust")).trusted === true; }
  async attach(bundleId: string) {
    const m = await this.req("attach", { bundleId }); this.bundleId = bundleId;
    return { pid: m.pid as number, manualAccessibility: m.manualAccessibility === true };
  }
  async snapshot(opts: { maxDepth?: number } = {}): Promise<AxSnapshot> {
    const m = await this.req("snapshot", { maxDepth: opts.maxDepth ?? 12 });
    return { formatVersion: AX_SNAPSHOT_FORMAT, bundleId: this.bundleId, capturedAt: (m.capturedAt as number) ?? Date.now(), root: m.root as AxNode, truncated: m.truncated === true };
  }
  async find(q: { role?: string; labelRegex?: string }) { return ((await this.req("find", q)).matches as AxNode[]) ?? []; }
  async press(path: number[]) { return (await this.req("press", { path }).catch(notFoundFalse)).ok === true; }
  async setValue(path: number[], value: string) { return (await this.req("setValue", { path, value }).catch(notFoundFalse)).ok === true; }
  async focus(path: number[]) { return (await this.req("focus", { path }).catch(notFoundFalse)).ok === true; }
  async observe(listener: (e: AxEvent) => void, notifications: readonly string[] = DEFAULT_OBSERVE) {
    this.listeners.add(listener);
    try { await this.req("observe", { notifications: [...notifications] }); } catch (e) { this.listeners.delete(listener); throw e; }
    return () => { this.listeners.delete(listener); };
  }
  async dispose() {
    const c = this.child; this.listeners.clear();
    if (c) { try { c.stdin.end(); c.kill(); } catch { /* already gone */ } }
    this.die(new AxError("bridge_unavailable", "ax-bridge disposed"));
  }
}
function notFoundFalse(e: unknown): Record<string, unknown> {
  if (e instanceof AxError && e.fault === "failed") return { ok: false };
  throw e;
}
