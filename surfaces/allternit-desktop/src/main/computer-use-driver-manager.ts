import { app } from 'electron';
import { spawn, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import log from 'electron-log';
import { spawnSidecar } from './process-lifeline.js';

export interface ComputerUseDriverStatus {
  available: boolean;
  running: boolean;
  embedded: boolean;
  executable?: string;
  socket?: string;
  error?: string;
}

const INSTALLED_CUA_DRIVER = '/Applications/CuaDriver.app/Contents/MacOS/cua-driver';
const INSTALLED_CUA_SOCKET = path.join(os.homedir(), 'Library/Caches/cua-driver/cua-driver.sock');

/** True when something is listening on the Unix socket. */
export function socketAnswers(socketPath: string, timeoutMs = 1000): Promise<boolean> {
  if (!fs.existsSync(socketPath)) return Promise.resolve(false);
  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    const done = (alive: boolean) => {
      socket.removeAllListeners();
      socket.destroy();
      resolve(alive);
    };
    socket.setTimeout(timeoutMs, () => done(false));
    socket.once('connect', () => done(true));
    socket.once('error', () => done(false));
  });
}

/** Remove a dead daemon's socket and pid file so a fresh one can bind. */
export function removeStaleDaemonFiles(socketPath: string): void {
  for (const file of [socketPath, path.join(path.dirname(socketPath), 'cua-driver.pid')]) {
    try {
      fs.rmSync(file, { force: true });
    } catch (error) {
      log.warn('[ComputerUseDriver] could not remove stale file', file, error);
    }
  }
}

export function cuaDriverBinaryName(platform: NodeJS.Platform = process.platform): string {
  return platform === 'win32' ? 'cua-driver.exe' : 'cua-driver';
}

export function defaultCuaSocketPath(
  runtimeDir: string,
  platform: NodeJS.Platform = process.platform,
): string {
  if (platform === 'win32') return String.raw`\\.\pipe\allternit-cua-driver`;
  return path.join(runtimeDir, 'cua-driver.sock');
}

/** The six agent cursor motion styles Cua Driver 0.34+ supports. */
export const CUA_CURSOR_MOTION_STYLES = [
  'signature_arc',
  'spring_settle',
  'magnetic',
  'comet_swoop',
  'adaptive',
  'classic',
] as const;
export type CuaCursorMotionStyle = (typeof CUA_CURSOR_MOTION_STYLES)[number];

/**
 * Cursor motion for new computer-use sessions. Setting key
 * `ALLTERNIT_CUA_CURSOR_MOTION`; unknown or empty values give the default,
 * `signature_arc`. The engine passes it to the driver's `start_session`, and
 * the driver's reduced-motion "auto" theme follows the OS setting.
 */
export function cuaCursorMotionStyle(value: string | undefined = process.env.ALLTERNIT_CUA_CURSOR_MOTION): CuaCursorMotionStyle {
  const candidate = (value ?? '').trim().toLowerCase();
  return (CUA_CURSOR_MOTION_STYLES as readonly string[]).includes(candidate)
    ? (candidate as CuaCursorMotionStyle)
    : 'signature_arc';
}

function isInstalledCuaDriver(executable: string): boolean {
  return process.platform === 'darwin' && path.resolve(executable) === path.resolve(INSTALLED_CUA_DRIVER);
}

/**
 * Owns Allternit's Cua Driver backend.
 *
 * On macOS, Computer History admission requires the exact executable inside the
 * verified, installed `/Applications/CuaDriver.app` bundle. A standalone
 * embedded binary cannot satisfy that check. Therefore this manager prefers the
 * installed app when present and connects to its daemon socket. If the app is
 * not installed, it falls back to spawning the embedded binary directly so that
 * regular computer-use actions still work (Accessibility/Screen Recording are
 * then attributed to Allternit Desktop when it is signed).
 *
 * On Windows and Linux the bundled cua-driver-rs binary is spawned directly.
 */
class ComputerUseDriverManager {
  private child: ChildProcess | null = null;
  private socketPath: string | null = null;
  private lastError: string | undefined;

  resolveExecutable(): string | null {
    const binaryName = cuaDriverBinaryName();
    const candidates = [
      process.env.ALLTERNIT_CUA_DRIVER_PATH,
      process.platform === 'darwin' ? INSTALLED_CUA_DRIVER : undefined,
      path.join(process.resourcesPath ?? '', 'computer-use', binaryName),
      !app.isPackaged ? path.join(os.homedir(), '.local', 'bin', binaryName) : undefined,
    ];
    for (const candidate of candidates) {
      if (candidate && fs.existsSync(candidate)) return path.resolve(candidate);
    }
    return null;
  }

  private async waitForSocket(deadlineMs: number, label: string): Promise<boolean> {
    const deadline = Date.now() + deadlineMs;
    while (Date.now() < deadline) {
      if (this.child && this.child.exitCode !== null) break;
      if (this.socketPath && fs.existsSync(this.socketPath)) {
        this.lastError = undefined;
        log.info(`[ComputerUseDriver] ${label} ready; socket=${this.socketPath}`);
        return true;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return false;
  }

  async start(): Promise<ComputerUseDriverStatus> {
    const executable = this.resolveExecutable();
    if (!executable) {
      this.lastError = process.platform === 'darwin'
        ? 'Bundled computer-use driver is missing.'
        : `Bundled computer-use driver is missing for ${process.platform}.`;
      return this.getStatus();
    }

    // If the installed CuaDriver.app is present, use its daemon. History and
    // all TCC attribution stay with the verified Cua Driver app bundle.
    if (isInstalledCuaDriver(executable)) {
      // A socket file alone doesn't mean the daemon is up: a crashed or
      // killed daemon leaves it behind, and every computer-use call then
      // fails with "Cua Driver daemon is not running". Only reuse a socket
      // that answers.
      if (await socketAnswers(INSTALLED_CUA_SOCKET)) {
        this.socketPath = INSTALLED_CUA_SOCKET;
        this.lastError = undefined;
        log.info('[ComputerUseDriver] using installed CuaDriver.app daemon');
        return this.getStatus();
      }
      if (fs.existsSync(INSTALLED_CUA_SOCKET)) {
        log.warn('[ComputerUseDriver] installed CuaDriver.app socket is stale; relaunching the daemon');
        removeStaleDaemonFiles(INSTALLED_CUA_SOCKET);
      }

      log.info('[ComputerUseDriver] launching installed CuaDriver.app daemon');
      const open = spawn('open', ['-n', '-g', '-a', 'CuaDriver', '--args', 'serve'], {
        stdio: 'ignore',
        windowsHide: true,
      });
      await new Promise<void>((resolve) => open.on('exit', () => resolve()));

      this.socketPath = INSTALLED_CUA_SOCKET;
      const ready = await this.waitForSocket(10_000, 'installed CuaDriver.app daemon');
      if (!ready) {
        this.lastError ??= 'Installed CuaDriver.app daemon did not become ready within 10 seconds.';
      }
      return this.getStatus();
    }

    const runtimeDir = path.join(app.getPath('userData'), 'computer-use');
    fs.mkdirSync(runtimeDir, { recursive: true, mode: 0o700 });
    this.socketPath = defaultCuaSocketPath(runtimeDir);
    if (process.platform !== 'win32') {
      fs.rmSync(this.socketPath, { force: true });
    }

    const env = {
      ...process.env,
      CUA_DRIVER_EMBEDDED: '1',
      CUA_DRIVER_RS_TELEMETRY_ENABLED: 'false',
      CUA_TELEMETRY_ENABLED: 'false',
      NO_COLOR: '1',
      ...(process.platform === 'darwin' ? { CUA_DRIVER_HOST_BUNDLE_ID: 'com.allternit.desktop' } : {}),
    };
    const args = ['serve', '--embedded', '--socket', this.socketPath];
    if (process.platform === 'darwin') {
      args.splice(2, 0, '--host-bundle-id', 'com.allternit.desktop');
    }
    const child = spawnSidecar(executable, args, { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    this.child = child;

    child.stdout?.on('data', (data: Buffer) => log.info('[ComputerUseDriver]', data.toString().trim()));
    child.stderr?.on('data', (data: Buffer) => log.warn('[ComputerUseDriver]', data.toString().trim()));
    child.on('error', (error) => {
      this.lastError = error.message;
      log.error('[ComputerUseDriver] failed:', error);
    });
    child.on('exit', (code) => {
      if (code && code !== 0) this.lastError = `Embedded driver exited with code ${code}.`;
      this.child = null;
      log.info(`[ComputerUseDriver] stopped (code ${code ?? 'unknown'})`);
    });

    const ready = await this.waitForSocket(10_000, 'embedded driver');
    if (!ready) {
      this.lastError ??= 'Embedded driver did not become ready within 10 seconds.';
    }
    return this.getStatus();
  }

  stop(): void {
    this.child?.kill('SIGTERM');
    this.child = null;
    if (this.socketPath && process.platform !== 'win32' && !this.socketPath.startsWith(INSTALLED_CUA_SOCKET)) {
      fs.rmSync(this.socketPath, { force: true });
    }
    this.socketPath = null;
  }

  getLaunchEnvironment(): Record<string, string> {
    const executable = this.resolveExecutable();
    if (!executable || !this.socketPath) return {};
    const env: Record<string, string> = {
      ALLTERNIT_CUA_DRIVER_PATH: executable,
      ALLTERNIT_CUA_DRIVER_SOCKET: this.socketPath,
      CUA_DRIVER_RS_TELEMETRY_ENABLED: 'false',
      CUA_TELEMETRY_ENABLED: 'false',
      ALLTERNIT_CUA_CURSOR_MOTION: cuaCursorMotionStyle(),
    };
    if (!isInstalledCuaDriver(executable)) {
      env.ALLTERNIT_CUA_DRIVER_EMBEDDED = 'true';
    }
    return env;
  }

  getStatus(): ComputerUseDriverStatus {
    const executable = this.resolveExecutable();
    const socketAlive = Boolean(this.socketPath && fs.existsSync(this.socketPath));
    const childAlive = Boolean(this.child && this.child.exitCode === null);
    return {
      available: executable !== null,
      running: socketAlive && (childAlive || isInstalledCuaDriver(executable ?? '')),
      embedded: Boolean(this.child),
      executable: executable ?? undefined,
      socket: this.socketPath ?? undefined,
      error: this.lastError,
    };
  }
}

export const computerUseDriverManager = new ComputerUseDriverManager();
