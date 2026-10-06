/**
 * Google ARTEMIS (github.com/google/artemis, Apache-2.0) as the plain-English
 * brain behind `phone.task`.
 *
 * ARTEMIS is a Python 3.12+ project with no PyPI package or release tags, so
 * "install" means: clone the repo at one pinned commit into the user data dir
 * and `uv sync` it. One MCP stdio server (`python -m mcp_server`) runs per
 * phone as an owned sidecar (lifeline-wrapped, killed on quit). Its tools
 * (verified against the pinned commit):
 *   mobile_run_task(task_desc, model, device_serial, …)  → { trace_id, … } (async)
 *   mobile_manage_task(action='status', trace_id)        → { status, … }
 * ARTEMIS drives its own model: it needs LLM credentials (e.g. GEMINI_API_KEY)
 * in its environment; without them phone.task reports the failure honestly.
 */

import { execFile, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { createNegotiatingClient, ProcStdioTransport, type McpProc } from './mcp-stdio-transport.js';
import { spawnSidecar } from './process-lifeline.js';
import type { ArtemisClient } from './phone-tools.js';

export const ARTEMIS_REPO = 'https://github.com/google/artemis.git';
/** google/artemis main as of 2026-10-02; bump deliberately after re-reading its MCP tool signatures. */
export const ARTEMIS_PINNED_COMMIT = '351ca8422f7b5b54e80a9c1ce03a222e02415b6b';

export type ArtemisInstallState = 'not_installed' | 'installing' | 'ready' | 'error';

export interface ArtemisStatus {
  state: ArtemisInstallState;
  pinnedCommit: string;
  error?: string;
}

// ── MCP stdio client (official SDK over the lifeline-wrapped process) ──────

export type { McpProc };

export class McpStdioClient {
  private readonly client = createNegotiatingClient('allternit-desktop-phone', '1.0.0');
  private ready: Promise<void> | null = null;
  closed = false;

  constructor(private readonly proc: McpProc, private readonly requestTimeoutMs = 30_000) {
    proc.onExit(() => {
      this.closed = true;
    });
  }

  private connect(): Promise<void> {
    if (this.closed) return Promise.reject(new Error('ARTEMIS server is not running'));
    return this.client.connect(new ProcStdioTransport(this.proc), { timeout: this.requestTimeoutMs });
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    if (this.closed) throw new Error('ARTEMIS server is not running');
    this.ready ??= this.connect();
    await this.ready;
    let result: {
      content?: Array<{ type: string; text?: string }>;
      structuredContent?: Record<string, unknown>;
      isError?: boolean;
    };
    try {
      result = (await this.client.callTool({ name, arguments: args }, { timeout: this.requestTimeoutMs })) as typeof result;
    } catch (error) {
      if (this.closed) throw new Error('ARTEMIS server exited');
      if (/timed? ?out/i.test(String((error as Error).message))) throw new Error(`ARTEMIS tools/call timed out`);
      throw error;
    }
    if (result.isError) throw new Error(result.content?.[0]?.text ?? `${name} failed`);
    if (result.structuredContent) return result.structuredContent;
    const text = result.content?.find((c) => c.type === 'text')?.text;
    if (!text) return {};
    try {
      return JSON.parse(text) as Record<string, unknown>;
    } catch {
      return { text };
    }
  }

  close(): void {
    this.closed = true;
    void this.client.close().catch(() => undefined);
    this.proc.kill();
  }
}

// ── Task driver ─────────────────────────────────────────────────────────────

const TERMINAL = new Set(['completed', 'failed', 'cancelled', 'stopped']);

export async function runArtemisTask(
  client: Pick<McpStdioClient, 'callTool'>,
  serial: string,
  task: string,
  model: 'Flash' | 'Pro',
  opts: { pollMs?: number; timeoutMs?: number; sleep?: (ms: number) => Promise<void> } = {},
): Promise<Record<string, unknown>> {
  const pollMs = opts.pollMs ?? 3_000;
  const timeoutMs = opts.timeoutMs ?? 5 * 60_000;
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));

  const started = await client.callTool('mobile_run_task', { task_desc: task, model, device_serial: serial });
  const traceId = typeof started.trace_id === 'string' ? started.trace_id : null;
  if (!traceId) throw new Error(`ARTEMIS did not start the task: ${JSON.stringify(started).slice(0, 200)}`);

  let waited = 0;
  for (;;) {
    const status = await client.callTool('mobile_manage_task', { action: 'status', trace_id: traceId });
    const s = String(status.status ?? '');
    if (TERMINAL.has(s)) return { traceId, ...status };
    if (waited >= timeoutMs) {
      await client.callTool('mobile_manage_task', { action: 'stop', trace_id: traceId }).catch(() => undefined);
      throw new Error('The phone task timed out after 5 minutes and was stopped.');
    }
    await sleep(pollMs);
    waited += pollMs;
  }
}

// ── Install + per-device sidecars ───────────────────────────────────────────

function exec(command: string, args: string[], cwd?: string): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(command, args, { cwd, timeout: 15 * 60_000, windowsHide: true }, (error, _o, stderr) =>
      error ? reject(new Error(`${command} ${args[0]} failed: ${(stderr || error.message).slice(0, 300)}`)) : resolve(),
    );
  });
}

export class ArtemisManager implements ArtemisClient {
  private state: ArtemisInstallState;
  private lastError: string | undefined;
  private clients = new Map<string, { client: McpStdioClient; child: ChildProcess }>();

  constructor(
    private readonly dataDir: string,
    private readonly env: () => NodeJS.ProcessEnv = () => process.env,
  ) {
    this.state = this.isInstalled() ? 'ready' : 'not_installed';
  }

  get dir(): string {
    return path.join(this.dataDir, 'artemis');
  }

  private pythonPath(): string {
    return process.platform === 'win32'
      ? path.join(this.dir, '.venv', 'Scripts', 'python.exe')
      : path.join(this.dir, '.venv', 'bin', 'python');
  }

  private isInstalled(): boolean {
    return fs.existsSync(this.pythonPath()) && fs.existsSync(path.join(this.dir, 'mcp_server'));
  }

  status(): ArtemisStatus {
    return { state: this.state, pinnedCommit: ARTEMIS_PINNED_COMMIT, error: this.lastError };
  }

  /** git clone at the pinned commit, then `uv sync`. Needs git and uv on PATH. */
  async install(): Promise<ArtemisStatus> {
    if (this.state === 'installing') return this.status();
    this.state = 'installing';
    this.lastError = undefined;
    try {
      if (!fs.existsSync(path.join(this.dir, '.git'))) {
        fs.mkdirSync(this.dir, { recursive: true });
        await exec('git', ['init', '-q'], this.dir);
        await exec('git', ['remote', 'add', 'origin', ARTEMIS_REPO], this.dir);
      }
      await exec('git', ['fetch', '--depth', '1', 'origin', ARTEMIS_PINNED_COMMIT], this.dir);
      await exec('git', ['checkout', '-q', '--detach', ARTEMIS_PINNED_COMMIT], this.dir);
      await exec('uv', ['sync', '--python', '3.12'], this.dir);
      this.state = this.isInstalled() ? 'ready' : 'error';
      if (this.state === 'error') this.lastError = 'ARTEMIS installed but its virtualenv is missing.';
    } catch (error) {
      this.state = 'error';
      this.lastError = error instanceof Error ? error.message : String(error);
    }
    return this.status();
  }

  private clientFor(serial: string): McpStdioClient {
    const existing = this.clients.get(serial);
    if (existing && !existing.client.closed) return existing.client;
    const child = spawnSidecar(this.pythonPath(), ['-m', 'mcp_server'], {
      cwd: this.dir,
      env: { ...this.env(), PYTHONUNBUFFERED: '1', PYTHONPATH: this.dir, ARTEMIS_DESKTOP_NOTIFY: 'false' },
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    });
    child.stderr?.resume();
    const client = new McpStdioClient({
      stdin: child.stdin!,
      stdout: child.stdout!,
      pid: child.pid,
      kill: () => child.kill('SIGTERM'),
      onExit: (cb) => child.once('exit', cb),
    });
    child.on('error', () => {
      client.closed = true;
    });
    this.clients.set(serial, { client, child });
    return client;
  }

  async runTask(serial: string, task: string, model: 'Flash' | 'Pro'): Promise<unknown> {
    if (this.state !== 'ready') throw new Error('ARTEMIS is not installed.');
    return runArtemisTask(this.clientFor(serial), serial, task, model);
  }

  stop(serial: string): void {
    this.clients.get(serial)?.client.close();
    this.clients.delete(serial);
  }

  stopAll(): void {
    for (const serial of [...this.clients.keys()]) this.stop(serial);
  }
}
