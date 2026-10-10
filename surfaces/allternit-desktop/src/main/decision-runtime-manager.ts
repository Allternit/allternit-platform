import { app } from 'electron';
import type { ChildProcess } from 'node:child_process';
import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import log from 'electron-log';
import { allternitDriverPython } from './computer-use-driver-manager.js';
import { spawnSidecar } from './process-lifeline.js';

/**
 * The Decision Runtime's local scorer (domains/decision-runtime): one-pass
 * option scoring on MLX (Apple silicon) or llama.cpp (Windows, Linux, Intel).
 * allternit-api's `POST /v1/decisions` calls it over loopback with a launch
 * token. It runs on the Python bundled for the Allternit Driver and installs
 * its packages and downloads its model on first use (progress in `/health`),
 * so no weights ship in the app.
 */
export interface DecisionRuntimeStatus {
  running: boolean;
  endpoint?: string;
  /** idle | installing | downloading | loading | ready | error (from the sidecar). */
  status?: string;
  progress?: number;
  model?: string;
  engine?: string;
  error?: string;
}

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export function decisionRuntimeArgs(stateDir: string, endpointFile: string): string[] {
  // -B: no bytecode in the signed bundle; -s -E: ignore user site-packages and PYTHON* vars.
  return ['-B', '-s', '-E', '-m', 'allternit_decisions', '--state-dir', stateDir, '--endpoint-file', endpointFile];
}

class DecisionRuntimeManager {
  private child: ChildProcess | null = null;
  private endpoint: string | null = null;
  private token: string | null = null;
  private lastError: string | undefined;

  /** Sidecar source and the Python that runs it: packaged, else the repo checkout in dev. */
  resolve(): { dir: string; python: string } | null {
    const resources = process.resourcesPath ?? '';
    const candidates = [
      { dir: path.join(resources, 'decision-runtime'), python: allternitDriverPython(path.join(resources, 'computer-use', 'driver')) },
    ];
    if (!app.isPackaged) {
      const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
      const key = `${process.platform}-${process.arch}`;
      const staged = path.join(repoRoot, 'surfaces', 'allternit-desktop', 'resources', 'computer-use', 'driver-python', key);
      candidates.push({
        dir: path.join(repoRoot, 'domains', 'decision-runtime'),
        python: process.env.ALLTERNIT_DECISIONS_PYTHON
          || (process.platform === 'win32' ? path.join(staged, 'python.exe') : path.join(staged, 'bin', 'python3')),
      });
    }
    return candidates.find((c) => fs.existsSync(path.join(c.dir, 'allternit_decisions', '__main__.py')) && fs.existsSync(c.python)) ?? null;
  }

  private alive(): boolean {
    return Boolean(this.child && this.child.exitCode === null && this.endpoint);
  }

  async start(): Promise<DecisionRuntimeStatus> {
    if (this.alive()) return this.getStatus();
    if (process.env.ALLTERNIT_DECISIONS_LOCAL === '0') {
      this.lastError = 'The local decision scorer is turned off (ALLTERNIT_DECISIONS_LOCAL=0).';
      return this.getStatus();
    }
    const found = this.resolve();
    if (!found) {
      this.lastError = 'The decision runtime sidecar is not bundled with this build.';
      log.warn('[DecisionRuntime]', this.lastError);
      return this.getStatus();
    }
    const stateDir = path.join(app.getPath('userData'), 'decision-runtime');
    fs.mkdirSync(stateDir, { recursive: true, mode: 0o700 });
    const endpointFile = path.join(stateDir, 'endpoint');
    fs.rmSync(endpointFile, { force: true });
    this.token = crypto.randomBytes(24).toString('hex');
    const child = spawnSidecar(found.python, decisionRuntimeArgs(stateDir, endpointFile), {
      cwd: found.dir,
      env: { ...process.env, ALLTERNIT_DECISIONS_TOKEN: this.token, HF_HUB_DISABLE_TELEMETRY: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.child = child;
    child.stdout?.on('data', (d: Buffer) => log.info('[DecisionRuntime]', d.toString().trim()));
    child.stderr?.on('data', (d: Buffer) => log.info('[DecisionRuntime]', d.toString().trim()));
    child.on('error', (error) => {
      this.lastError = error.message;
      log.error('[DecisionRuntime] failed:', error);
    });
    child.on('exit', (code) => {
      if (code && code !== 0) this.lastError = `Decision runtime exited with code ${code}.`;
      if (this.child === child) {
        this.child = null;
        this.endpoint = null;
      }
      log.info(`[DecisionRuntime] stopped (code ${code ?? 'unknown'})`);
    });

    // The server binds before any preparation, so this is quick.
    const deadline = Date.now() + 10_000;
    while (Date.now() < deadline && child.exitCode === null) {
      if (fs.existsSync(endpointFile)) {
        this.endpoint = fs.readFileSync(endpointFile, 'utf8').trim();
        this.lastError = undefined;
        log.info(`[DecisionRuntime] ready on ${this.endpoint}`);
        return this.getStatus();
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    this.lastError ??= 'The decision runtime did not start within 10 seconds.';
    log.warn('[DecisionRuntime]', this.lastError);
    return this.getStatus();
  }

  stop(): void {
    this.child?.kill('SIGTERM');
    this.child = null;
    this.endpoint = null;
  }

  /** allternit-api reads these to reach the scorer. */
  getLaunchEnvironment(): Record<string, string> {
    if (!this.alive() || !this.endpoint || !this.token) return {};
    return { ALLTERNIT_DECISIONS_LOCAL_URL: this.endpoint, ALLTERNIT_DECISIONS_TOKEN: this.token };
  }

  /** Includes the sidecar's own preparation status (install / download progress). */
  async getStatus(): Promise<DecisionRuntimeStatus> {
    const base: DecisionRuntimeStatus = { running: this.alive(), endpoint: this.endpoint ?? undefined, error: this.lastError };
    if (!this.alive() || !this.endpoint) return base;
    try {
      const res = await fetch(`${this.endpoint}/health`, {
        headers: { authorization: `Bearer ${this.token}` },
        signal: AbortSignal.timeout(1500),
      });
      const h = (await res.json()) as { status?: string; progress?: number; model?: string; engine?: string; error?: string | null };
      return { ...base, status: h.status, progress: h.progress, model: h.model, engine: h.engine, error: h.error ?? base.error };
    } catch (error) {
      return { ...base, error: error instanceof Error ? error.message : String(error) };
    }
  }
}

export const decisionRuntimeManager = new DecisionRuntimeManager();
