/**
 * Bonsai Local Companion Manager
 *
 * Supervises the packaged Bonsai Image companion (services/bonsai-local):
 *   - install / progress / cancel / repair via install.sh
 *   - start / stop of the loopback server with app lifecycle
 *   - removal of managed files under the Allternit support directory
 *
 * The server binds 127.0.0.1:8000 and is health-checked via GET /backends.
 */

import { app, BrowserWindow } from 'electron';
import { spawn, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import log from 'electron-log';

const BONSAI_PORT = 8000;
const HEALTH_TIMEOUT_MS = 120_000;

export interface BonsaiStatus {
  installed: boolean;
  running: boolean;
  installing: boolean;
  url: string;
  revisions?: {
    source?: string;
    model?: string;
    mlxWheel?: string;
  };
  installDir: string;
  error?: string;
}

export interface BonsaiProgress {
  stage: 'starting' | 'installing' | 'ready' | 'error' | 'cancelled';
  message: string;
}

function defaultRoot(): string {
  return path.join(os.homedir(), 'Library', 'Application Support', 'Allternit', 'bonsai-local');
}

export class BonsaiCompanionManager {
  private static instance: BonsaiCompanionManager;
  private serverProc: ChildProcess | null = null;
  private installProc: ChildProcess | null = null;

  static getInstance(): BonsaiCompanionManager {
    if (!BonsaiCompanionManager.instance) {
      BonsaiCompanionManager.instance = new BonsaiCompanionManager();
    }
    return BonsaiCompanionManager.instance;
  }

  get root(): string {
    return process.env.ALLTERNIT_BONSAI_HOME || defaultRoot();
  }

  get url(): string {
    return `http://127.0.0.1:${BONSAI_PORT}`;
  }

  private get sourceDir(): string {
    return path.join(this.root, 'image-studio');
  }

  private get modelDir(): string {
    return path.join(this.root, 'models', 'bonsai-image-ternary-4B-mlx-2bit');
  }

  private resolveScriptsDir(): string | null {
    const candidates = app.isPackaged
      ? [path.join(process.resourcesPath ?? '', 'bonsai-local')]
      : [
          path.join(app.getAppPath(), '..', '..', '..', 'services', 'bonsai-local'),
          path.join(process.resourcesPath ?? '', 'bonsai-local'),
        ];
    for (const candidate of candidates) {
      if (fs.existsSync(path.join(candidate, 'install.sh'))) return candidate;
    }
    return null;
  }

  private readRevision(file: string): string | undefined {
    try {
      return fs.readFileSync(path.join(this.root, file), 'utf8').trim() || undefined;
    } catch {
      return undefined;
    }
  }

  async getStatus(): Promise<BonsaiStatus> {
    const installed =
      fs.existsSync(path.join(this.sourceDir, '.venv', 'bin', 'uvicorn')) &&
      fs.existsSync(path.join(this.modelDir, 'transformer-packed-mflux'));

    let running = false;
    try {
      const res = await fetch(`${this.url}/backends`, { signal: AbortSignal.timeout(1000) });
      const body = (await res.json().catch(() => null)) as { healthy?: boolean } | null;
      running = res.ok && body?.healthy === true;
    } catch {
      running = false;
    }

    return {
      installed,
      running,
      installing: this.installProc !== null,
      url: this.url,
      revisions: {
        source: this.readRevision('source.revision'),
        model: this.readRevision('model.revision'),
        mlxWheel: this.readRevision('mlx-wheel.version'),
      },
      installDir: this.root,
    };
  }

  /** Run (or re-run = repair) the installer, streaming output as progress events. */
  async install(): Promise<void> {
    if (this.installProc) {
      throw new Error('A Bonsai install is already in progress.');
    }
    const scriptsDir = this.resolveScriptsDir();
    if (!scriptsDir) {
      throw new Error('Bonsai installer scripts are not available in this build.');
    }

    this.emitProgress({ stage: 'starting', message: 'Starting Bonsai installer…' });

    return new Promise<void>((resolve, reject) => {
      const child = spawn('bash', [path.join(scriptsDir, 'install.sh')], {
        env: process.env,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      this.installProc = child;

      let lastLine = '';
      const onData = (d: Buffer) => {
        for (const line of d.toString().split('\n')) {
          const trimmed = line.trim();
          if (!trimmed) continue;
          lastLine = trimmed;
          this.emitProgress({ stage: 'installing', message: trimmed });
        }
      };
      child.stdout?.on('data', onData);
      child.stderr?.on('data', onData);

      child.on('error', (err) => {
        this.installProc = null;
        this.emitProgress({ stage: 'error', message: err.message });
        reject(err);
      });
      child.on('exit', (code, signal) => {
        this.installProc = null;
        if (code === 0) {
          log.info('[Bonsai] install complete');
          this.emitProgress({ stage: 'ready', message: 'Bonsai companion installed.' });
          resolve();
        } else if (signal === 'SIGTERM') {
          this.emitProgress({ stage: 'cancelled', message: 'Install cancelled.' });
          reject(new Error('Install cancelled.'));
        } else {
          const message = `Bonsai installer failed (code ${code}): ${lastLine}`;
          log.error(`[Bonsai] ${message}`);
          this.emitProgress({ stage: 'error', message });
          reject(new Error(message));
        }
      });
    });
  }

  cancelInstall(): boolean {
    if (!this.installProc) return false;
    log.info('[Bonsai] cancelling install');
    this.installProc.kill('SIGTERM');
    return true;
  }

  /** Start the companion server and wait until it reports healthy. */
  async start(): Promise<void> {
    try {
      await this.waitForHealthy('existing bonsai companion', 5_000);
      return;
    } catch {
      // Not already running; continue with startup.
    }
    if (this.serverProc) return;

    const scriptsDir = this.resolveScriptsDir();
    if (!scriptsDir) {
      throw new Error('Bonsai start script is not available in this build.');
    }

    log.info('[Bonsai] starting companion server');
    this.serverProc = spawn('bash', [path.join(scriptsDir, 'start.sh')], {
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.serverProc.stdout?.on('data', (d: Buffer) => log.info('[Bonsai]', d.toString().trim()));
    this.serverProc.stderr?.on('data', (d: Buffer) => log.warn('[Bonsai]', d.toString().trim()));
    this.serverProc.on('exit', (code) => {
      log.warn(`[Bonsai] companion exited (code ${code})`);
      this.serverProc = null;
    });

    await this.waitForHealthy('bonsai companion', HEALTH_TIMEOUT_MS);
  }

  stop(): void {
    if (this.serverProc) {
      log.info('[Bonsai] stopping companion server');
      this.serverProc.kill('SIGTERM');
      this.serverProc = null;
    }
  }

  /** Stop the server and delete all managed files. Never touches user data. */
  async remove(): Promise<void> {
    this.stop();
    if (this.installProc) this.cancelInstall();

    const root = path.resolve(this.root);
    const expected = path.resolve(defaultRoot());
    if (root !== expected || !root.startsWith(os.homedir() + path.sep)) {
      throw new Error(`Refusing to delete unexpected path: ${root}`);
    }
    log.info(`[Bonsai] removing ${root}`);
    await fs.promises.rm(root, { recursive: true, force: true });
  }

  private emitProgress(progress: BonsaiProgress): void {
    for (const win of BrowserWindow.getAllWindows()) {
      win.webContents.send('bonsai:progress', progress);
    }
  }

  private async waitForHealthy(label: string, timeoutMs: number): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const res = await fetch(`${this.url}/backends`, { signal: AbortSignal.timeout(1000) });
        const body = (await res.json().catch(() => null)) as { healthy?: boolean } | null;
        if (res.ok && body?.healthy === true) return;
      } catch {
        // Not ready yet.
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    throw new Error(`${label} did not become healthy within ${Math.round(timeoutMs / 1000)}s`);
  }
}

export const bonsaiCompanion = BonsaiCompanionManager.getInstance();
