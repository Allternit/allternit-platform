/**
 * Factory Engine Manager
 *
 * Allternit Desktop ships the Allternit Factory engine (`allternit-factory`,
 * staged into resources/bin by scripts/prepare-factory.cjs) next to Gizzi and
 * runs it, so Code mode, the terminal wall, Mission Control and the bot
 * surfaces all read the same engine through allternit-api's /api/factory
 * proxy.
 *
 *   - start(): adopt an engine that already answers on the port (a `gizzi`
 *     command may have started one), else spawn
 *     `allternit-factory --root <workspace> serve --port <port>`.
 *     Desktop runs it on PORTS.FACTORY (3018), not the engine's default 3011,
 *     because Desktop's extension bridge owns 3011. allternit-api is told the
 *     address through ALLTERNIT_FACTORY_URL (backend-manager).
 *   - installCli(): first run, link `gizzi` (and only `gizzi`) into
 *     ~/.local/bin. The engine is found next to the real gizzi binary, so it
 *     is never put on PATH itself.
 *   - removeStaleTools(): remove the retired pre-Factory work-engine and
 *     orchestrator entries in ~/.local/bin (names in removeStaleTools), only
 *     after checking each one is the old tool.
 *
 * Best-effort like every sidecar: a failure here never blocks the app. The
 * proxy answers 502 `transport` while the engine is down, and the Factory
 * screens say so.
 */

import { ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import { app } from 'electron';
import log from 'electron-log';
import { PORTS, factoryEngineUrl } from './config.js';
import { runtimeResource } from './runtime-home.js';
import { spawnSidecar } from './process-lifeline.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const DEFAULT_HEALTH_TIMEOUT_MS = 15_000;
const DEFAULT_HEALTH_INTERVAL_MS = 200;
const PROBE_TIMEOUT_MS = 1_000;

export type FactoryEngineMode = 'adopted' | 'spawned';

export interface FactoryBinaryContext {
  packaged: boolean;
  /** process.resourcesPath in packaged builds. */
  resourcesPath?: string;
  /**
   * The active runtime package's copy (runtimeResource('bin', name)), which
   * a runtime update moves together with gizzi-code. Checked before
   * resources/bin when packaged.
   */
  runtimeBinary?: string;
  /** Repo root (…/allternit) used in development. */
  repoRoot: string;
  platform?: NodeJS.Platform;
  env?: NodeJS.ProcessEnv;
  exists?: (p: string) => boolean;
}

function binaryName(platform: NodeJS.Platform): string {
  return platform === 'win32' ? 'allternit-factory.exe' : 'allternit-factory';
}

/**
 * Where the engine binary is, or null. Packaged: the active runtime
 * package's copy, then resources/bin (the copy that shipped with this
 * Desktop). Dev: $ALLTERNIT_FACTORY_BIN, then the
 * repo's cargo targets. Never PATH: an unrelated engine version must not be
 * picked up silently.
 */
export function resolveFactoryBinary(context: FactoryBinaryContext): string | null {
  const platform = context.platform ?? process.platform;
  const env = context.env ?? process.env;
  const exists = context.exists ?? ((p: string) => fs.existsSync(p));
  const name = binaryName(platform);
  const candidates = context.packaged
    ? [context.runtimeBinary ?? '', path.join(context.resourcesPath ?? '', 'bin', name)]
    : [
        env.ALLTERNIT_FACTORY_BIN ?? '',
        env.CARGO_TARGET_DIR ? path.join(env.CARGO_TARGET_DIR, 'release', name) : '',
        env.CARGO_TARGET_DIR ? path.join(env.CARGO_TARGET_DIR, 'debug', name) : '',
        path.join(context.repoRoot, 'target', 'release', name),
        path.join(context.repoRoot, 'target', 'debug', name),
        path.join(context.repoRoot, 'surfaces', 'allternit-desktop', 'resources', 'bin', name),
      ];
  return candidates.find((c) => c !== '' && exists(c)) ?? null;
}

/** The gizzi binary a packaged Desktop ships in resources/bin, or null. */
export function bundledGizziBinary(
  resourcesPath: string | undefined,
  platform: NodeJS.Platform = process.platform,
  exists: (p: string) => boolean = fs.existsSync,
): string | null {
  if (!resourcesPath || platform === 'win32') return null;
  const p = path.join(resourcesPath, 'bin', 'gizzi-code');
  return exists(p) ? p : null;
}

/** The workspace whose `.allternit/` ledger Desktop's engine serves. */
export function factoryWorkspaceRoot(env: NodeJS.ProcessEnv = process.env, home = os.homedir()): string {
  const fromEnv = env.ALLTERNIT_FACTORY_ROOT?.trim();
  return fromEnv ? fromEnv : path.join(home, '.allternit', 'factory', 'workspace');
}

export function factoryServeArgs(root: string, port: number, peerPort: number | null = null): string[] {
  return ['--root', root, 'serve', '--port', String(port), ...(peerPort ? ['--peer-port', String(peerPort)] : [])];
}

/** The Factory engine's peer port (matches `allternit computers serve`). */
export const FACTORY_PEER_PORT = 3019;

/**
 * The peer port when this computer is paired as an Allternit remote computer
 * (`allternit computer pair` wrote ~/.allternit/computer/paired.json), else
 * null. With it, other computers' engines of the same account or its
 * organization can run bots here, over the mesh, with a peer ticket.
 */
export function factoryPeerPort(home = os.homedir(), exists: (p: string) => boolean = fs.existsSync): number | null {
  return exists(path.join(home, '.allternit', 'computer', 'paired.json')) ? FACTORY_PEER_PORT : null;
}

// ── PATH install and stale tool removal ─────────────────────────────────────

export interface LinkFs {
  lstat(p: string): fs.Stats | null;
  readlink(p: string): string | null;
  /** First `bytes` bytes of a regular file, as latin1 (binaries included). */
  head(p: string, bytes: number): string | null;
  readdir(p: string): string[];
  symlink(target: string, p: string): void;
  unlink(p: string): void;
  mkdirp(p: string): void;
}

export const nodeLinkFs: LinkFs = {
  lstat: (p) => {
    try {
      return fs.lstatSync(p);
    } catch {
      return null;
    }
  },
  readlink: (p) => {
    try {
      return fs.readlinkSync(p);
    } catch {
      return null;
    }
  },
  head: (p, bytes) => {
    let fd: number | null = null;
    try {
      fd = fs.openSync(p, 'r');
      const buf = Buffer.alloc(bytes);
      const n = fs.readSync(fd, buf, 0, bytes, 0);
      return buf.subarray(0, n).toString('latin1');
    } catch {
      return null;
    } finally {
      if (fd !== null) fs.closeSync(fd);
    }
  },
  readdir: (p) => {
    try {
      return fs.readdirSync(p);
    } catch {
      return [];
    }
  },
  symlink: (target, p) => fs.symlinkSync(target, p),
  unlink: (p) => fs.unlinkSync(p),
  mkdirp: (p) => fs.mkdirSync(p, { recursive: true }),
};

/** A link target that lives inside an Allternit Desktop bundle or staging dir. */
function isDesktopBundled(target: string): boolean {
  return /Allternit Desktop\.app\/|allternit-desktop\/resources\/bin\//.test(target);
}

export type GizziLinkResult =
  | { action: 'linked' | 'relinked'; link: string; target: string }
  | { action: 'kept'; link: string; reason: string }
  | { action: 'skipped'; reason: string };

/**
 * Put `gizzi` on PATH as ~/.local/bin/gizzi → the bundled gizzi binary.
 * Creates it when absent and refreshes it when it points into a (moved or
 * older) Desktop bundle. Anything else there — a brew link, a file someone
 * put there — is the user's and is kept.
 */
export function installGizziLink(gizziBinary: string | null, home: string, lfs: LinkFs = nodeLinkFs): GizziLinkResult {
  if (!gizziBinary) return { action: 'skipped', reason: 'no bundled gizzi binary' };
  const dir = path.join(home, '.local', 'bin');
  const link = path.join(dir, 'gizzi');
  const st = lfs.lstat(link);
  if (!st) {
    lfs.mkdirp(dir);
    lfs.symlink(gizziBinary, link);
    return { action: 'linked', link, target: gizziBinary };
  }
  if (!st.isSymbolicLink()) return { action: 'kept', link, reason: 'a file the user put there' };
  const current = lfs.readlink(link) ?? '';
  if (current === gizziBinary) return { action: 'kept', link, reason: 'already current' };
  if (!isDesktopBundled(current)) return { action: 'kept', link, reason: `points at ${current}, not a Desktop bundle` };
  lfs.unlink(link);
  lfs.symlink(gizziBinary, link);
  return { action: 'relinked', link, target: gizziBinary };
}

/** Markers of the pre-Factory work engine and orchestrator tools. */
const OLD_TARGET = /allternit-rails|commrails|agent-orchestrator|ao-engine/; // old-names: keep (installer removes the old binaries)
const OLD_BINARY_MARKER = /allternit-rails|commrails|allternit_commrails/; // old-names: keep (installer removes the old binaries)
const OLD_CONSULT_GUARD = /AO_CONSULT_ACTIVE|ao-consult\.repo-link|allternit-rails steer consult/; // old-names: keep (installer removes the old binaries)

export interface StaleRemoval {
  removed: string[];
  kept: { path: string; reason: string }[];
}

/**
 * Remove the retired tools from ~/.local/bin, each only after checking it is
 * the old tool (the names are in the code below, marked as kept):
 *   - the old work-engine binary: a symlink into the old engine, or a binary
 *     that carries the old engine's crate name;
 *   - the old orchestrator scripts (prefix match, including the consult
 *     repo link): a symlink into the old orchestrator or its engine;
 *   - the old consult script: that symlink, or the recursion-guard script
 *     that wrapped the old engine's steer consult.
 * Anything else with those names is kept and listed.
 */
export function removeStaleTools(home: string, lfs: LinkFs = nodeLinkFs): StaleRemoval {
  const dir = path.join(home, '.local', 'bin');
  const out: StaleRemoval = { removed: [], kept: [] };
  const names = lfs.readdir(dir).filter((n) => n === 'allternit-rails' || n.startsWith('ao-')); // old-names: keep (installer removes the old binaries)
  for (const name of names) {
    const p = path.join(dir, name);
    const st = lfs.lstat(p);
    if (!st) continue;
    let old = false;
    let reason = '';
    if (st.isSymbolicLink()) {
      const target = lfs.readlink(p) ?? '';
      old = OLD_TARGET.test(target);
      reason = `points at ${target}`;
    } else if (st.isFile()) {
      if (name === 'allternit-rails') { // old-names: keep (installer removes the old binaries)
        old = OLD_BINARY_MARKER.test(lfs.head(p, 8 * 1024 * 1024) ?? '');
        reason = 'not the old engine binary';
      } else if (name === 'ao-consult') { // old-names: keep (installer removes the old binaries)
        old = OLD_CONSULT_GUARD.test(lfs.head(p, 64 * 1024) ?? '');
        reason = 'not the old consult guard';
      } else {
        reason = 'a regular file, not an old-tool link';
      }
    } else {
      reason = 'not a file or link';
    }
    if (old) {
      try {
        lfs.unlink(p);
        out.removed.push(p);
      } catch (err) {
        out.kept.push({ path: p, reason: `could not remove: ${(err as Error).message}` });
      }
    } else {
      out.kept.push({ path: p, reason });
    }
  }
  return out;
}

// ── The engine process ─────────────────────────────────────────────────────

export interface FactoryEngineManagerOptions {
  port?: number;
  fetchImpl?: typeof fetch;
  healthTimeoutMs?: number;
  healthIntervalMs?: number;
  binaryContext?: Partial<FactoryBinaryContext>;
}

export class FactoryEngineManager {
  private child: ChildProcess | null = null;
  private mode: FactoryEngineMode | null = null;
  private lastError: string | null = null;
  private readonly port: number;
  private readonly fetchImpl: typeof fetch;
  private readonly healthTimeoutMs: number;
  private readonly healthIntervalMs: number;
  private readonly binaryContextOverride?: Partial<FactoryBinaryContext>;

  constructor(options: FactoryEngineManagerOptions = {}) {
    this.port = options.port ?? PORTS.FACTORY;
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.healthTimeoutMs = options.healthTimeoutMs ?? DEFAULT_HEALTH_TIMEOUT_MS;
    this.healthIntervalMs = options.healthIntervalMs ?? DEFAULT_HEALTH_INTERVAL_MS;
    this.binaryContextOverride = options.binaryContext;
  }

  getUrl(): string {
    return factoryEngineUrl(this.port);
  }

  getMode(): FactoryEngineMode | null {
    return this.mode;
  }

  /** The engine binary this Desktop ships (or dev build), for gizzi's env. */
  getBinaryPath(): string | null {
    return resolveFactoryBinary(this.binaryContext());
  }

  /**
   * Ensure the engine answers on the port. Returns its URL, or null with the
   * reason in getStatus().error. Never throws.
   */
  async start(): Promise<string | null> {
    const url = this.getUrl();
    if (this.child) return url;
    if (await this.isHealthy()) {
      this.mode = 'adopted';
      this.lastError = null;
      log.info(`[FactoryEngine] Adopted the Factory engine already running at ${url}`);
      return url;
    }
    const bin = this.getBinaryPath();
    if (!bin) {
      this.lastError = 'allternit-factory is not bundled with this build';
      log.warn(`[FactoryEngine] ${this.lastError}; /api/factory will answer 502 transport`);
      return null;
    }
    const root = factoryWorkspaceRoot();
    try {
      fs.mkdirSync(root, { recursive: true });
    } catch (err) {
      log.warn('[FactoryEngine] Could not create the Factory workspace:', err);
    }
    const args = factoryServeArgs(root, this.port, factoryPeerPort());
    log.info(`[FactoryEngine] Starting ${bin} ${args.join(' ')}`);
    this.child = spawnSidecar(bin, args, {
      env: {
        ...(Object.fromEntries(
          Object.entries(process.env).filter(([, v]) => v !== undefined),
        ) as Record<string, string>),
        ALLTERNIT_FACTORY_BIN: bin,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.child.stdout?.on('data', (d: Buffer) => log.info('[FactoryEngine]', d.toString().trim()));
    this.child.stderr?.on('data', (d: Buffer) => log.warn('[FactoryEngine]', d.toString().trim()));
    this.child.on('exit', (code) => {
      log.warn(`[FactoryEngine] allternit-factory exited (code ${code})`);
      this.lastError = `allternit-factory exited (code ${code})`;
      this.child = null;
      if (this.mode === 'spawned') this.mode = null;
    });
    this.child.on('error', (err) => {
      log.warn('[FactoryEngine] Failed to spawn allternit-factory:', err);
      this.lastError = `could not start allternit-factory: ${err.message}`;
      this.child = null;
    });
    if (await this.waitForHealth()) {
      this.mode = 'spawned';
      this.lastError = null;
      log.info(`[FactoryEngine] Ready at ${url}`);
      return url;
    }
    this.lastError ??= 'allternit-factory did not become healthy';
    log.warn(`[FactoryEngine] ${this.lastError}; continuing without it`);
    this.stop();
    return null;
  }

  /** Stop only an engine we spawned. An adopted one belongs to whoever started it. */
  stop(): void {
    if (this.child && !this.child.killed) {
      log.info('[FactoryEngine] Stopping allternit-factory…');
      this.child.kill('SIGTERM');
    }
    this.child = null;
    if (this.mode === 'spawned') this.mode = null;
  }

  async getStatus(): Promise<{ running: boolean; mode: FactoryEngineMode | null; url: string; error: string | null }> {
    return { running: await this.isHealthy(), mode: this.mode, url: this.getUrl(), error: this.lastError };
  }

  /** First-run install: `gizzi` on PATH, and the retired tools removed. */
  installCli(gizziBinary: string | null, home = os.homedir()): { gizzi: GizziLinkResult; stale: StaleRemoval } {
    let gizzi: GizziLinkResult;
    try {
      gizzi = installGizziLink(gizziBinary, home);
    } catch (err) {
      gizzi = { action: 'skipped', reason: (err as Error).message };
    }
    const stale = removeStaleTools(home);
    log.info('[FactoryEngine] gizzi on PATH:', JSON.stringify(gizzi));
    if (stale.removed.length) log.info('[FactoryEngine] Removed retired tools:', stale.removed.join(', '));
    if (stale.kept.length) log.info('[FactoryEngine] Kept (not the retired tools):', JSON.stringify(stale.kept));
    return { gizzi, stale };
  }

  private async isHealthy(): Promise<boolean> {
    try {
      const res = await this.fetchImpl(`${this.getUrl()}/api/factory/health`, {
        signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
      });
      if (!res.ok) return false;
      const body = (await res.json().catch(() => null)) as { ok?: boolean; service?: string } | null;
      return body?.ok === true && body?.service === 'allternit-factory';
    } catch {
      return false;
    }
  }

  private async waitForHealth(): Promise<boolean> {
    const deadline = Date.now() + this.healthTimeoutMs;
    while (Date.now() < deadline) {
      if (this.child?.exitCode !== null && this.child?.exitCode !== undefined) return false;
      if (!this.child) return false;
      if (await this.isHealthy()) return true;
      await new Promise((r) => setTimeout(r, this.healthIntervalMs));
    }
    return false;
  }

  private binaryContext(): FactoryBinaryContext {
    // __dirname is dist/main; four levels up is the repo root.
    return {
      packaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      runtimeBinary: app.isPackaged ? runtimeResource('bin', binaryName(process.platform)) : undefined,
      repoRoot: path.resolve(__dirname, '..', '..', '..', '..'),
      ...(this.binaryContextOverride ?? {}),
    };
  }
}

export const factoryEngineManager = new FactoryEngineManager();
