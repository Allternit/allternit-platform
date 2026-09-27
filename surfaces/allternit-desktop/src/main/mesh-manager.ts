/**
 * Mesh connectivity manager (Allternit tailnet client).
 *
 * Gives the desktop app reachability to gizzi instances registered on the
 * Allternit tailnet with `http://100.x.y.z:4096` URLs — the same problem the
 * iOS app solves with its embedded tsnet + loopback proxy. There is no
 * system VPN here: the mesh-node tsnet sidecar (infrastructure/mesh/tsnet-ios/
 * cmd/mesh-node) joins the tailnet in pure userspace and, in `--reverse`
 * mode, listens on 127.0.0.1:<ephemeral> and dials one fixed tailnet target
 * per connection. One sidecar per instance target; the renderer rewrites
 * 100.64.0.0/10 URLs to the returned loopback URL.
 *
 * Follows the DevicePairingManager pattern: the runtime device credential
 * never leaves Electron main — enrollment (`POST /api/v1/mesh/enroll`,
 * dual-auth with the device token) is brokered here and the renderer only
 * sees state and loopback URLs.
 *
 * Sidecar stdout contract (scraped like gizzi's mesh.ts):
 *   MESH_READY ip=<100.x addr>   — node is up on the tailnet
 *   PROXY_READY port=<n>         — reverse-mode loopback listener is bound
 *   MESH_ERROR reason=...        — stderr, fatal (non-zero exit follows)
 */

import { app, ipcMain } from 'electron';
import { spawn, type ChildProcess } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import log from 'electron-log';
import { authManager } from './auth-manager.js';
import { URLS } from './config.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

export type MeshState = 'stopped' | 'starting' | 'running' | 'error';

export interface MeshProxyInfo {
  target: string;
  url: string;
}

export interface MeshStatus {
  state: MeshState;
  meshIp?: string;
  error?: string;
  proxies: MeshProxyInfo[];
}

interface MeshEnrollment {
  controlUrl: string;
  authKey: string;
  expiresAt: string;
  meshUser: string;
}

interface MeshNode {
  target: string;
  proc: ChildProcess;
  port: number;
  restarting?: NodeJS.Timeout;
}

const MESH_HOSTNAME = 'allternit-desktop';
const READY_TIMEOUT_MS = 60_000;
const MAX_RESTART_DELAY_MS = 30_000;

function cloudApiBaseUrl(): string {
  return (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, '');
}

/** True for IPv4 hosts in the Tailscale CGNAT range (100.64.0.0/10). */
export function isMeshHost(host: string): boolean {
  const match = /^100\.(\d{1,3})\.\d{1,3}\.\d{1,3}$/.exec(host);
  return !!match && Number(match[1]) >= 64 && Number(match[1]) <= 127;
}

class MeshManager {
  private state: MeshState = 'stopped';
  private meshIp?: string;
  private lastError?: string;
  private nodes = new Map<string, MeshNode>();
  private starting?: Promise<void>;
  private stopping = false;
  private restartAttempts = new Map<string, number>();

  constructor() {
    ipcMain.handle('mesh:start', async () => {
      await this.start();
      return this.getStatus();
    });
    ipcMain.handle('mesh:stop', async () => {
      await this.stop();
      return this.getStatus();
    });
    ipcMain.handle('mesh:status', () => this.getStatus());
    ipcMain.handle('mesh:proxy-for', async (_event, instanceUrl: string) =>
      this.proxyFor(instanceUrl),
    );
  }

  getStatus(): MeshStatus {
    return {
      state: this.state,
      meshIp: this.meshIp,
      error: this.lastError,
      proxies: [...this.nodes.values()].map((node) => ({
        target: node.target,
        url: `http://127.0.0.1:${node.port}`,
      })),
    };
  }

  /**
   * Verify the mesh can be used: sidecar binary present, desktop paired,
   * enrollment accepted. Actual sidecars start lazily on the first
   * `proxyFor` — enrollment keys are single-use, so we mint them per node.
   */
  async start(): Promise<void> {
    if (this.state === 'running' || this.state === 'starting') return this.starting ?? Promise.resolve();
    this.stopping = false;
    this.setState('starting');
    this.starting = (async () => {
      const binary = this.resolveBinaryPath();
      if (!binary) {
        throw new Error(
          'mesh-node sidecar not found. Set ALLTERNIT_MESH_NODE_BIN, or rebuild the sidecar (infrastructure/mesh/tsnet-ios/build-sidecar.sh).',
        );
      }
      const session = await authManager.getSession();
      if (!session) {
        throw new Error('This desktop is not paired with an Allternit account.');
      }
      // Validate enrollment end-to-end now so `start` fails fast with a
      // useful error; proxyFor re-enrolls per sidecar (keys are single-use).
      await this.enroll();
      this.setState('running');
      log.info('[Mesh] Mesh available (sidecar starts on first mesh URL use)');
    })().catch((err) => {
      this.setError(err);
      throw err;
    });
    return this.starting;
  }

  /**
   * Resolve a mesh instance URL (http://100.x.y.z:4096/...) to a loopback URL
   * (http://127.0.0.1:<port>/...) bridged into the tailnet, preserving the
   * path and query. Lazily starts the mesh on first use. Non-mesh URLs are
   * returned unchanged.
   */
  async proxyFor(instanceUrl: string): Promise<string> {
    let parsed: URL;
    try {
      parsed = new URL(instanceUrl);
    } catch {
      throw new Error(`Invalid instance URL: ${instanceUrl}`);
    }
    if (!isMeshHost(parsed.hostname)) {
      return instanceUrl;
    }
    await this.start();
    const target = `${parsed.hostname}:${parsed.port || '4096'}`;
    const node = this.nodes.get(target) ?? await this.spawnNode(target);
    const rewritten = new URL(instanceUrl);
    rewritten.protocol = 'http:';
    rewritten.hostname = '127.0.0.1';
    rewritten.port = String(node.port);
    return rewritten.toString();
  }

  async stop(): Promise<void> {
    this.stopping = true;
    for (const node of this.nodes.values()) {
      if (node.restarting) clearTimeout(node.restarting);
      try {
        node.proc.kill('SIGTERM');
      } catch {
        // already gone
      }
    }
    this.nodes.clear();
    this.restartAttempts.clear();
    this.meshIp = undefined;
    this.starting = undefined;
    this.setState('stopped');
    log.info('[Mesh] Stopped all mesh-node sidecars');
  }

  resolveSidecarPath(): string | null {
    return this.resolveBinaryPath();
  }

  async enrollForGizzi(): Promise<{ authKey: string; controlUrl: string } | null> {
    const session = await authManager.getSession();
    if (!session) return null;
    const enrollment = await this.enroll();
    return { authKey: enrollment.authKey, controlUrl: enrollment.controlUrl };
  }

  /** Binary discovery mirrors gizzi-manager: packaged resources/bin first, then the repo vendor tree, then an env override. */
  private resolveBinaryPath(): string | null {
    if (process.env.ALLTERNIT_MESH_NODE_BIN && fs.existsSync(process.env.ALLTERNIT_MESH_NODE_BIN)) {
      return process.env.ALLTERNIT_MESH_NODE_BIN;
    }
    const binaryName = process.platform === 'win32' ? 'mesh-node.exe' : 'mesh-node';
    const platformArch = `${process.platform}-${process.arch}`;
    const candidates = app.isPackaged
      ? [path.join(process.resourcesPath ?? '', 'bin', binaryName)]
      : [
          path.join(process.resourcesPath ?? '', 'bin', binaryName),
          path.join(
            app.getAppPath(), '..', '..', 'cmd', 'gizzi-code', 'vendor', 'mesh-node', platformArch, binaryName,
          ),
          path.join(__dirname, '..', '..', 'resources', 'bin', binaryName),
        ];
    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) return candidate;
    }
    log.error('[Mesh] mesh-node binary not found. Searched:', candidates);
    return null;
  }

  /**
   * Mint a fresh Headscale pre-auth key for this device's owner. Keys are
   * single-use and expire after 24h, so every new sidecar enrolls for
   * itself; a sidecar with existing tsnet state never consumes its key.
   */
  private async enroll(): Promise<MeshEnrollment> {
    const session = await authManager.getSession();
    if (!session) {
      throw new Error('This desktop is not paired with an Allternit account.');
    }
    const response = await fetch(`${cloudApiBaseUrl()}/api/v1/mesh/enroll`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${session.accessToken}` },
    });
    const payload = await response.json().catch(() => ({})) as {
      message?: string;
      error?: string;
    } & Partial<MeshEnrollment>;
    if (!response.ok) {
      throw new Error(payload.message || payload.error || `Mesh enrollment failed (${response.status})`);
    }
    if (!payload.controlUrl || !payload.authKey) {
      throw new Error('Mesh enrollment returned an incomplete response');
    }
    return payload as MeshEnrollment;
  }

  private async spawnNode(target: string, retriedWithFreshKey = false): Promise<MeshNode> {
    const binary = this.resolveBinaryPath();
    if (!binary) {
      throw new Error('mesh-node sidecar not found (set ALLTERNIT_MESH_NODE_BIN)');
    }
    const enrollment = await this.enroll();
    const dataDir = path.join(app.getPath('userData'), 'mesh', target.replace(/[^a-zA-Z0-9.-]+/g, '_'));
    const args = [
      '--hostname', MESH_HOSTNAME,
      '--control-url', enrollment.controlUrl,
      '--auth-key', enrollment.authKey,
      '--data-dir', dataDir,
      '--reverse', target,
    ];
    log.info(`[Mesh] Spawning mesh-node sidecar for ${target}`, { binary, dataDir });
    const proc = spawn(binary, args, { stdio: ['ignore', 'pipe', 'pipe'] });

    try {
      const { ip, port } = await this.waitReady(proc);
      const node: MeshNode = { target, proc, port };
      this.nodes.set(target, node);
      this.meshIp = ip;
      this.restartAttempts.delete(target);
      this.setState('running');
      log.info(`[Mesh] Proxy ready for ${target} at 127.0.0.1:${port} (mesh ip ${ip})`);
      proc.once('exit', (code) => this.onNodeExit(node, code));
      return node;
    } catch (err) {
      try {
        proc.kill('SIGTERM');
      } catch {
        // already gone
      }
      // Auth keys are single-use and short-lived: if the sidecar was
      // rejected, re-enroll once with a fresh key before giving up.
      if (!retriedWithFreshKey && err instanceof Error && /auth.?key|unauthorized|401/i.test(err.message)) {
        log.warn('[Mesh] Auth key rejected; re-enrolling with a fresh key');
        return this.spawnNode(target, true);
      }
      throw err;
    }
  }

  /** Scrape the MESH_READY / PROXY_READY contract lines (or fail on MESH_ERROR / early exit). */
  private waitReady(proc: ChildProcess): Promise<{ ip: string; port: number }> {
    return new Promise((resolve, reject) => {
      let settled = false;
      let ip: string | undefined;
      let port: number | undefined;
      const done = () => {
        if (settled || !ip || port === undefined) return;
        settled = true;
        clearTimeout(timeout);
        resolve({ ip, port });
      };
      const fail = (err: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        reject(err);
      };
      const timeout = setTimeout(
        () => fail(new Error('mesh-node did not report PROXY_READY within 60 seconds')),
        READY_TIMEOUT_MS,
      );
      proc.stdout?.on('data', (chunk: Buffer) => {
        const text = chunk.toString();
        const ready = /^MESH_READY ip=(\S+)$/m.exec(text);
        if (ready) ip = ready[1];
        const proxy = /^PROXY_READY port=(\d+)$/m.exec(text);
        if (proxy) port = Number(proxy[1]);
        done();
      });
      proc.stderr?.on('data', (chunk: Buffer) => {
        for (const line of chunk.toString().split('\n')) {
          if (!line.trim()) continue;
          if (line.startsWith('MESH_ERROR')) {
            fail(new Error(line.replace(/^MESH_ERROR\s+reason=/, '').trim() || 'mesh-node failed'));
          } else {
            log.debug('[Mesh] mesh-node:', line.trim());
          }
        }
      });
      proc.once('error', (err) => fail(new Error(`failed to spawn mesh-node: ${err.message}`)));
      proc.once('exit', (code) => fail(new Error(`mesh-node exited (code ${code}) before reporting PROXY_READY`)));
    });
  }

  /** Unexpected sidecar exit: drop the proxy and respawn with backoff unless we're stopping. */
  private onNodeExit(node: MeshNode, code: number | null): void {
    if (this.nodes.get(node.target) !== node) return;
    this.nodes.delete(node.target);
    if (this.stopping) return;
    const attempts = (this.restartAttempts.get(node.target) ?? 0) + 1;
    this.restartAttempts.set(node.target, attempts);
    const delay = Math.min(1000 * 2 ** (attempts - 1), MAX_RESTART_DELAY_MS);
    log.warn(`[Mesh] mesh-node for ${node.target} exited (code ${code}); restarting in ${delay}ms`);
    node.restarting = setTimeout(() => {
      if (this.stopping) return;
      this.spawnNode(node.target).catch((err) => {
        log.error(`[Mesh] Restart of mesh-node for ${node.target} failed:`, err);
        this.setError(err);
      });
    }, delay);
  }

  private setState(state: MeshState): void {
    this.state = state;
    if (state !== 'error') this.lastError = undefined;
  }

  private setError(err: unknown): void {
    this.state = 'error';
    this.lastError = err instanceof Error ? err.message : String(err);
    log.error('[Mesh]', this.lastError);
  }
}

export const meshManager = new MeshManager();
