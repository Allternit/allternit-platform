/**
 * Office Engine Manager
 *
 * Manages the office-engine Node sidecar (services/office-engine — a Hono
 * service on port 8099) that backs the Rust gateway's /api/office/* routes.
 *
 *   - If a healthy office-engine already answers on the port (e.g. a dev
 *     machine running `pnpm dev`), it is adopted as-is.
 *   - Otherwise the desktop spawns it: dev → `tsx src/index.ts` from the
 *     repo checkout; packaged → the bundled copy under
 *     process.resourcesPath/office-engine (staged by
 *     scripts/prepare-office-engine.cjs) run with ELECTRON_RUN_AS_NODE.
 *
 * The sidecar is strictly best-effort: a failure here must never block or
 * crash the app — the gateway already answers 502 when the engine is down.
 */

import { spawn, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import { app } from 'electron';
import log from 'electron-log';
import { PORTS, officeEngineUrl } from './config.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const DEFAULT_HEALTH_TIMEOUT_MS = 15_000;
const DEFAULT_HEALTH_INTERVAL_MS = 200;
const ADOPT_PROBE_TIMEOUT_MS = 1_000;

export type OfficeEngineMode = 'adopted' | 'spawned';

export interface OfficeEngineSpawnSpec {
  command: string;
  args: string[];
  cwd?: string;
  extraEnv?: Record<string, string>;
}

export interface OfficeEngineSpawnContext {
  packaged: boolean;
  /** process.resourcesPath in packaged builds. */
  resourcesPath?: string;
  /** Repo root (…/allternit) used in development. */
  repoRoot: string;
  platform?: NodeJS.Platform;
  /** process.execPath, used to re-run Electron as plain Node when packaged. */
  execPath?: string;
}

/**
 * Resolve how the office-engine sidecar would be launched, or null when no
 * usable entry point exists (e.g. missing checkout in dev or the bundled
 * copy was not staged into resources). Pure — kept separate from the manager
 * so it can be unit-tested without spawning anything.
 */
export function resolveXlsxSidecarBinary(context: OfficeEngineSpawnContext): string | null {
  const platform = context.platform ?? process.platform;
  const name = platform === 'win32' ? 'allternit-xlsx-sidecar.exe' : 'allternit-xlsx-sidecar';
  const candidates = context.packaged
    ? [
        path.join(context.resourcesPath ?? '', 'office-engine', 'bin', name),
        path.join(context.resourcesPath ?? '', 'bin', name),
      ]
    : [
        path.join(
          context.repoRoot,
          'packages',
          '@allternit',
          'office-xlsx-engine',
          'crate',
          'target',
          'release',
          name,
        ),
        path.join(context.repoRoot, 'target', 'release', name),
      ];
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? null;
}

function spawnExtraEnv(
  context: OfficeEngineSpawnContext,
  base: Record<string, string> = {},
): Record<string, string> {
  const extraEnv = { ...base };
  const xlsx = resolveXlsxSidecarBinary(context);
  if (xlsx) extraEnv.ALLTERNIT_XLSX_SIDECAR_BINARY = xlsx;
  return extraEnv;
}

export function resolveOfficeEngineSpawn(context: OfficeEngineSpawnContext): OfficeEngineSpawnSpec | null {
  const platform = context.platform ?? process.platform;

  if (context.packaged) {
    const root = path.join(context.resourcesPath ?? '', 'office-engine');
    const entry = path.join(root, 'dist', 'index.js');
    if (!fs.existsSync(entry)) {
      return null;
    }
    return {
      // No Node runtime is guaranteed on end-user machines; re-exec the
      // Electron binary as plain Node against the bundled service build.
      command: context.execPath ?? process.execPath,
      args: [entry],
      extraEnv: spawnExtraEnv(context, { ELECTRON_RUN_AS_NODE: '1' }),
    };
  }

  const serviceDir = path.join(context.repoRoot, 'services', 'office-engine');
  const entry = path.join(serviceDir, 'src', 'index.ts');
  const tsxBin = path.join(
    serviceDir,
    'node_modules',
    '.bin',
    platform === 'win32' ? 'tsx.cmd' : 'tsx'
  );
  if (!fs.existsSync(entry) || !fs.existsSync(tsxBin)) {
    return null;
  }
  return {
    command: tsxBin,
    args: ['src/index.ts'],
    cwd: serviceDir,
    extraEnv: spawnExtraEnv(context),
  };
}

export interface OfficeEngineManagerOptions {
  port?: number;
  fetchImpl?: typeof fetch;
  healthTimeoutMs?: number;
  healthIntervalMs?: number;
  /** Overrides for tests; defaults come from the Electron app runtime. */
  spawnContext?: Partial<OfficeEngineSpawnContext>;
}

export class OfficeEngineManager {
  private child: ChildProcess | null = null;
  private mode: OfficeEngineMode | null = null;
  private readonly port: number;
  private readonly fetchImpl: typeof fetch;
  private readonly healthTimeoutMs: number;
  private readonly healthIntervalMs: number;
  private readonly spawnContextOverride?: Partial<OfficeEngineSpawnContext>;

  constructor(options: OfficeEngineManagerOptions = {}) {
    this.port = options.port ?? PORTS.OFFICE_ENGINE;
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.healthTimeoutMs = options.healthTimeoutMs ?? DEFAULT_HEALTH_TIMEOUT_MS;
    this.healthIntervalMs = options.healthIntervalMs ?? DEFAULT_HEALTH_INTERVAL_MS;
    this.spawnContextOverride = options.spawnContext;
  }

  getUrl(): string {
    return officeEngineUrl(undefined, this.port);
  }

  getMode(): OfficeEngineMode | null {
    return this.mode;
  }

  /**
   * Ensure the office-engine sidecar is reachable. Adopts an already-healthy
   * instance on the port, otherwise spawns one. Returns the base URL on
   * success and null on failure — this method never throws.
   */
  async start(): Promise<string | null> {
    const url = this.getUrl();
    if (this.child) {
      return url;
    }

    if (await this.isHealthy()) {
      this.mode = 'adopted';
      log.info(`[OfficeEngineManager] Adopted existing office-engine at ${url}`);
      return url;
    }

    const spec = resolveOfficeEngineSpawn(this.spawnContext());
    if (!spec) {
      log.warn('[OfficeEngineManager] office-engine entry not found; /api/office/* will stay unavailable');
      return null;
    }

    log.info(`[OfficeEngineManager] Starting office-engine on port ${this.port}: ${spec.command} ${spec.args.join(' ')}`);
    this.child = spawn(spec.command, spec.args, {
      cwd: spec.cwd,
      env: {
        ...Object.fromEntries(
          Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][]
        ),
        OFFICE_ENGINE_PORT: String(this.port),
        OFFICE_ENGINE_HOST: '127.0.0.1',
        ...(spec.extraEnv ?? {}),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });

    this.child.stdout?.on('data', (d: Buffer) =>
      log.info('[OfficeEngine]', d.toString().trim())
    );
    this.child.stderr?.on('data', (d: Buffer) =>
      log.warn('[OfficeEngine]', d.toString().trim())
    );
    this.child.on('exit', (code) => {
      log.warn(`[OfficeEngineManager] office-engine exited (code ${code})`);
      this.child = null;
      if (this.mode === 'spawned') {
        this.mode = null;
      }
    });
    this.child.on('error', (err) => {
      log.warn('[OfficeEngineManager] Failed to spawn office-engine:', err);
      this.child = null;
    });

    if (await this.waitForHealth()) {
      this.mode = 'spawned';
      log.info(`[OfficeEngineManager] office-engine ready at ${url}`);
      return url;
    }

    log.warn('[OfficeEngineManager] office-engine did not become healthy; continuing without it');
    this.stop();
    return null;
  }

  /** Stop only the child we spawned — an adopted process is left running. */
  stop(): void {
    if (this.child && !this.child.killed) {
      log.info('[OfficeEngineManager] Stopping office-engine…');
      this.child.kill('SIGTERM');
    }
    this.child = null;
    if (this.mode === 'spawned') {
      this.mode = null;
    }
  }

  async getStatus(): Promise<{ running: boolean; mode: OfficeEngineMode | null; url: string }> {
    return { running: await this.isHealthy(), mode: this.mode, url: this.getUrl() };
  }

  /** True when a healthy office-engine answers /health on the port. */
  private async isHealthy(): Promise<boolean> {
    try {
      const res = await this.fetchImpl(`${this.getUrl()}/health`, {
        signal: AbortSignal.timeout(ADOPT_PROBE_TIMEOUT_MS),
      });
      if (!res.ok) {
        return false;
      }
      const body = await res.json().catch(() => null) as { status?: string; service?: string } | null;
      return body?.status === 'ok' && body?.service === 'office-engine';
    } catch {
      return false;
    }
  }

  private async waitForHealth(): Promise<boolean> {
    const deadline = Date.now() + this.healthTimeoutMs;
    while (Date.now() < deadline) {
      if (this.child?.exitCode !== null && this.child?.exitCode !== undefined) {
        return false;
      }
      if (await this.isHealthy()) {
        return true;
      }
      await new Promise((r) => setTimeout(r, this.healthIntervalMs));
    }
    return false;
  }

  private spawnContext(): OfficeEngineSpawnContext {
    // __dirname is dist/main; go up four levels to reach the repo root.
    const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
    return {
      packaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      repoRoot,
      execPath: process.execPath,
      ...(this.spawnContextOverride ?? {}),
    };
  }
}

export const officeEngineManager = new OfficeEngineManager();
