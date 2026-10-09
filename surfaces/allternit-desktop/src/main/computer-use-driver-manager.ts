import { app } from 'electron';
import { spawn, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as crypto from 'node:crypto';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import log from 'electron-log';
import { spawnSidecar } from './process-lifeline.js';

export interface ComputerUseDriverStatus {
  available: boolean;
  running: boolean;
  embedded: boolean;
  executable?: string;
  socket?: string;
  error?: string;
  /** The Allternit Driver sidecar (arc + Cua engines) in front of Cua Driver. */
  allternitDriver?: { running: boolean; endpoint?: string; error?: string };
}

const __dirname = path.dirname(fileURLToPath(import.meta.url));

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

/** The bundled Python inside a staged driver directory. */
export function allternitDriverPython(driverDir: string, platform: NodeJS.Platform = process.platform): string {
  return platform === 'win32'
    ? path.join(driverDir, 'python', 'python.exe')
    : path.join(driverDir, 'python', 'bin', 'python3');
}

/**
 * Where the Allternit Driver listens: a private unix socket next to Cua's,
 * or loopback TCP on Windows (port chosen by the sidecar, token-gated).
 */
export function allternitDriverListen(runtimeDir: string, platform: NodeJS.Platform = process.platform): string {
  return platform === 'win32' ? 'tcp:127.0.0.1:0' : `unix:${path.join(runtimeDir, 'allternit-driver.sock')}`;
}

/**
 * Arguments for the sidecar. `-B` keeps Python from writing bytecode into the
 * signed app bundle, `-s -E` from reading the user's site-packages or PYTHON*
 * variables; `-m` still imports from the working directory (the driver dir).
 */
export function allternitDriverArgs(opts: {
  listen: string;
  stateDir: string;
  endpointFile?: string;
  cua?: string;
  cuaSocket?: string;
  cuaEmbedded?: boolean;
}): string[] {
  const args = ['-B', '-s', '-E', '-m', 'allternit_driver', '--listen', opts.listen, '--state-dir', opts.stateDir];
  if (opts.endpointFile) args.push('--endpoint-file', opts.endpointFile);
  if (opts.cua) args.push('--cua', opts.cua);
  if (opts.cuaSocket) args.push('--cua-socket', opts.cuaSocket);
  if (opts.cuaEmbedded) args.push('--cua-embedded');
  return args;
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
  private driverChild: ChildProcess | null = null;
  private driverEndpoint: string | null = null;
  private driverToken: string | null = null;
  private driverError: string | undefined;

  /** The staged sidecar: packaged resources, else the repo checkout in dev. */
  resolveAllternitDriver(): { dir: string; python: string } | null {
    const packaged = path.join(process.resourcesPath ?? '', 'computer-use', 'driver');
    const candidates: Array<{ dir: string; python: string }> = [{ dir: packaged, python: allternitDriverPython(packaged) }];
    if (!app.isPackaged) {
      const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
      const desktopDir = path.join(repoRoot, 'surfaces', 'allternit-desktop');
      const key = `${process.platform}-${process.arch}`;
      const staged = path.join(desktopDir, 'resources', 'computer-use', 'driver-python', key);
      candidates.push({
        dir: path.join(repoRoot, 'domains', 'computer-use', 'driver'),
        python: process.env.ALLTERNIT_DRIVER_PYTHON
          || (process.platform === 'win32' ? path.join(staged, 'python.exe') : path.join(staged, 'bin', 'python3')),
      });
    }
    return candidates.find((c) => fs.existsSync(path.join(c.dir, 'allternit_driver', '__main__.py')) && fs.existsSync(c.python)) ?? null;
  }

  private driverAlive(): boolean {
    return Boolean(this.driverChild && this.driverChild.exitCode === null && this.driverEndpoint);
  }

  /** Start the Allternit Driver in front of the Cua Driver this manager runs. */
  private async startAllternitDriver(cua: string | null): Promise<void> {
    if (this.driverAlive()) return;
    const found = this.resolveAllternitDriver();
    if (!found) {
      this.driverError = 'The Allternit Driver is not bundled with this build.';
      log.warn('[AllternitDriver]', this.driverError);
      return;
    }
    const runtimeDir = path.join(app.getPath('userData'), 'computer-use');
    fs.mkdirSync(runtimeDir, { recursive: true, mode: 0o700 });
    const listen = allternitDriverListen(runtimeDir);
    const endpointFile = path.join(runtimeDir, 'allternit-driver.endpoint');
    fs.rmSync(endpointFile, { force: true });
    if (listen.startsWith('unix:')) fs.rmSync(listen.slice(5), { force: true });
    this.driverToken = process.platform === 'win32' ? crypto.randomBytes(24).toString('hex') : null;
    const args = allternitDriverArgs({
      listen,
      stateDir: path.join(runtimeDir, 'driver'),
      endpointFile,
      cua: cua ?? undefined,
      cuaSocket: this.socketPath ?? undefined,
      cuaEmbedded: Boolean(cua && !isInstalledCuaDriver(cua)),
    });
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      CUA_DRIVER_RS_TELEMETRY_ENABLED: 'false',
      CUA_TELEMETRY_ENABLED: 'false',
      ...(this.driverToken ? { ALLTERNIT_DRIVER_TOKEN: this.driverToken } : {}),
    };
    const child = spawnSidecar(found.python, args, { cwd: found.dir, env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    this.driverChild = child;
    child.stdout?.on('data', (data: Buffer) => log.info('[AllternitDriver]', data.toString().trim()));
    child.stderr?.on('data', (data: Buffer) => log.info('[AllternitDriver]', data.toString().trim()));
    child.on('error', (error) => {
      this.driverError = error.message;
      log.error('[AllternitDriver] failed:', error);
    });
    child.on('exit', (code) => {
      if (code && code !== 0) this.driverError = `Allternit Driver exited with code ${code}.`;
      if (this.driverChild === child) {
        this.driverChild = null;
        this.driverEndpoint = null;
      }
      log.info(`[AllternitDriver] stopped (code ${code ?? 'unknown'})`);
    });

    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline && child.exitCode === null) {
      if (fs.existsSync(endpointFile)) {
        this.driverEndpoint = fs.readFileSync(endpointFile, 'utf8').trim();
        this.driverError = undefined;
        log.info(`[AllternitDriver] ready on ${this.driverEndpoint}`);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    this.driverError ??= 'The Allternit Driver did not become ready within 15 seconds.';
    log.warn('[AllternitDriver]', this.driverError);
  }

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
    const status = await this.startCua();
    // The sidecar runs even without Cua (macOS reads go to arc); it reports
    // which engines it has.
    await this.startAllternitDriver(status.running ? status.executable ?? null : null);
    return this.getStatus();
  }

  private async startCua(): Promise<ComputerUseDriverStatus> {
    // Called again on backend restart: keep a healthy embedded daemon.
    if (this.child && this.child.exitCode === null && this.socketPath && fs.existsSync(this.socketPath)) {
      return this.getStatus();
    }
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
    this.driverChild?.kill('SIGTERM');
    this.driverChild = null;
    this.driverEndpoint = null;
    this.child?.kill('SIGTERM');
    this.child = null;
    if (this.socketPath && process.platform !== 'win32' && !this.socketPath.startsWith(INSTALLED_CUA_SOCKET)) {
      fs.rmSync(this.socketPath, { force: true });
    }
    this.socketPath = null;
  }

  getLaunchEnvironment(): Record<string, string> {
    const env: Record<string, string> = {};
    const executable = this.resolveExecutable();
    if (executable && this.socketPath) {
      Object.assign(env, {
        ALLTERNIT_CUA_DRIVER_PATH: executable,
        ALLTERNIT_CUA_DRIVER_SOCKET: this.socketPath,
        CUA_DRIVER_RS_TELEMETRY_ENABLED: 'false',
        CUA_TELEMETRY_ENABLED: 'false',
        ALLTERNIT_CUA_CURSOR_MOTION: cuaCursorMotionStyle(),
      });
      if (!isInstalledCuaDriver(executable)) env.ALLTERNIT_CUA_DRIVER_EMBEDDED = 'true';
    }
    // allternit-api sends this-device input through the sidecar when it's up.
    if (this.driverAlive() && this.driverEndpoint) {
      if (this.driverEndpoint.startsWith('unix:')) {
        env.ALLTERNIT_DRIVER_SOCKET = this.driverEndpoint.slice(5);
      } else {
        env.ALLTERNIT_DRIVER_ENDPOINT = this.driverEndpoint;
        if (this.driverToken) env.ALLTERNIT_DRIVER_TOKEN = this.driverToken;
      }
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
      allternitDriver: {
        running: this.driverAlive(),
        endpoint: this.driverEndpoint ?? undefined,
        error: this.driverError,
      },
    };
  }
}

export const computerUseDriverManager = new ComputerUseDriverManager();
