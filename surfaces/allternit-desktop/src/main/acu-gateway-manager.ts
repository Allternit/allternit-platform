/**
 * ACU computer-use gateway manager.
 *
 * Spawns the Python FastAPI gateway on :8760 so /api/aci/* on allternit-api
 * can show the actual computer. Adopt a healthy process already on the port
 * (dev). Otherwise spawn launch.py from the staged tree or the repo checkout.
 *
 * Best-effort: a failure here must not block Desktop boot. ACI then 502s
 * honestly until the gateway is up.
 */

import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import { app } from 'electron';
import log from 'electron-log';
import { PORTS, URLS } from './config.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const ADOPT_PROBE_MS = 1_000;
const HEALTH_TIMEOUT_MS = 20_000;
const HEALTH_INTERVAL_MS = 250;

export type AcuGatewayMode = 'adopted' | 'spawned';

export interface AcuGatewaySpawnSpec {
  command: string;
  args: string[];
  cwd: string;
  extraEnv: Record<string, string>;
}

export interface AcuGatewaySpawnContext {
  packaged: boolean;
  resourcesPath?: string;
  repoRoot: string;
  platform?: NodeJS.Platform;
  pythonPath?: string;
}

export function acuGatewayUrl(): string {
  return URLS.ACU;
}

function pythonHasUvicorn(python: string): boolean {
  try {
    execFileSync(python, ['-c', 'import uvicorn'], { stdio: 'ignore', timeout: 4000 });
    return true;
  } catch {
    return false;
  }
}

function walkForAcuVenv(start: string | undefined, platform: NodeJS.Platform): string | null {
  if (!start) return null;
  const venvBin = platform === 'win32' ? path.join('Scripts', 'python.exe') : path.join('bin', 'python');
  let dir = start;
  for (let i = 0; i < 12; i += 1) {
    const candidate = path.join(dir, 'domains', 'computer-use', 'core', '.venv', venvBin);
    if (fs.existsSync(candidate) && pythonHasUvicorn(candidate)) return candidate;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

export function resolveAcuPython(context: AcuGatewaySpawnContext): string {
  if (context.pythonPath) return context.pythonPath;
  if (process.env.ALLTERNIT_ACU_PYTHON && pythonHasUvicorn(process.env.ALLTERNIT_ACU_PYTHON)) {
    return process.env.ALLTERNIT_ACU_PYTHON;
  }
  const platform = context.platform ?? process.platform;
  const venvBin = platform === 'win32' ? path.join('Scripts', 'python.exe') : path.join('bin', 'python');
  const venvCandidates = [
    path.join(context.resourcesPath ?? '', 'computer-use', 'acu', '.venv', venvBin),
    path.join(context.repoRoot, 'domains', 'computer-use', 'core', '.venv', venvBin),
    walkForAcuVenv(context.resourcesPath, platform),
    walkForAcuVenv(process.execPath, platform),
  ];
  for (const candidate of venvCandidates) {
    if (candidate && fs.existsSync(candidate) && pythonHasUvicorn(candidate)) return candidate;
  }
  const names = platform === 'win32' ? ['python.exe'] : ['python3.12', 'python3.11', 'python3'];
  const dirs = ['/opt/homebrew/bin', '/usr/local/bin', path.join(process.env.HOME ?? '', '.local', 'bin'), '/usr/bin'];
  for (const name of names) {
    for (const dir of dirs) {
      const full = path.join(dir, name);
      if (fs.existsSync(full) && pythonHasUvicorn(full)) return full;
    }
  }
  return platform === 'win32' ? 'python' : 'python3';
}

export function resolveAcuGatewaySpawn(context: AcuGatewaySpawnContext): AcuGatewaySpawnSpec | null {
  const roots = context.packaged
    ? [path.join(context.resourcesPath ?? '', 'computer-use', 'acu')]
    : [
        path.join(context.repoRoot, 'domains', 'computer-use', 'core'),
        path.join(context.resourcesPath ?? '', 'computer-use', 'acu'),
      ];
  for (const root of roots) {
    const launch = path.join(root, 'launch.py');
    if (!fs.existsSync(launch)) continue;
    return {
      command: resolveAcuPython({ ...context, pythonPath: context.pythonPath }),
      args: [launch],
      cwd: root,
      extraEnv: {
        PYTHONPATH: `${root}${path.delimiter}${path.join(root, 'gateway')}`,
        ALLTERNIT_ACU_HOST: '127.0.0.1',
        ALLTERNIT_ACU_PORT: String(PORTS.ACU),
        ALLTERNIT_LOCAL_BRAIN_URL: process.env.ALLTERNIT_LOCAL_BRAIN_URL || URLS.GIZZI,
        ALLTERNIT_VISION_PROVIDER: process.env.ALLTERNIT_VISION_PROVIDER || 'allternit',
      },
    };
  }
  return null;
}

export class AcuGatewayManager {
  private child: ChildProcess | null = null;
  private mode: AcuGatewayMode | null = null;
  private stopping = false;
  /** Set by the child exit handler BEFORE `child` is nulled, so waitForHealth
   * can see the crash even after the reference is gone. */
  private childDied = false;
  private fetchImpl: typeof fetch = fetch;
  spawnContextOverride?: Partial<AcuGatewaySpawnContext>;

  getUrl(): string {
    return acuGatewayUrl();
  }

  getLaunchEnvironment(): Record<string, string> {
    return { ALLTERNIT_ACU_URL: this.getUrl() };
  }

  async start(): Promise<string | null> {
    if (await this.isHealthy()) {
      this.mode = this.child ? 'spawned' : 'adopted';
      log.info(`[AcuGateway] using existing gateway at ${this.getUrl()}`);
      return this.getUrl();
    }
    const spec = resolveAcuGatewaySpawn(this.spawnContext());
    if (!spec) {
      log.warn('[AcuGateway] launch.py not staged; ACI Open computer will 502 until the gateway is started');
      return null;
    }
    this.stopping = false;
    this.childDied = false;
    log.info(`[AcuGateway] Starting ${spec.command} ${spec.args.join(' ')} (cwd ${spec.cwd})`);
    this.child = spawn(spec.command, spec.args, {
      cwd: spec.cwd,
      env: { ...process.env, ...spec.extraEnv },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.child.stdout?.on('data', (data: Buffer) => log.info('[AcuGateway]', data.toString().trim()));
    this.child.stderr?.on('data', (data: Buffer) => log.warn('[AcuGateway]', data.toString().trim()));
    this.child.on('exit', (code) => {
      log.warn(`[AcuGateway] exited (code ${code})`);
      // Record the death BEFORE clearing the reference — waitForHealth polls
      // this flag to give up immediately instead of waiting out the full
      // HEALTH_TIMEOUT_MS for a process that already crashed (e.g. the
      // packaged python missing uvicorn).
      this.childDied = true;
      this.child = null;
      if (this.mode === 'spawned') this.mode = null;
    });
    if (await this.waitForHealth()) {
      this.mode = 'spawned';
      log.info(`[AcuGateway] ready at ${this.getUrl()}`);
      return this.getUrl();
    }
    log.warn('[AcuGateway] did not become healthy; continuing without it');
    this.stop();
    return null;
  }

  stop(): void {
    this.stopping = true;
    if (this.child && !this.child.killed) {
      log.info('[AcuGateway] Stopping…');
      this.child.kill('SIGTERM');
    }
    this.child = null;
    if (this.mode === 'spawned') this.mode = null;
  }

  private async isHealthy(): Promise<boolean> {
    try {
      const res = await this.fetchImpl(`${this.getUrl()}/health`, {
        signal: AbortSignal.timeout(ADOPT_PROBE_MS),
      });
      return res.ok;
    } catch {
      return false;
    }
  }

  private async waitForHealth(): Promise<boolean> {
    const deadline = Date.now() + HEALTH_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (this.childDied) return false;
      if (this.child?.exitCode !== null && this.child?.exitCode !== undefined) return false;
      if (await this.isHealthy()) return true;
      await new Promise((resolve) => setTimeout(resolve, HEALTH_INTERVAL_MS));
    }
    return false;
  }

  private spawnContext(): AcuGatewaySpawnContext {
    const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
    return {
      packaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      repoRoot,
      ...(this.spawnContextOverride ?? {}),
    };
  }
}

export const acuGatewayManager = new AcuGatewayManager();
