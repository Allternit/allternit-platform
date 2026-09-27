/**
 * Mini-apps lifecycle manager.
 *
 * Handles installing (npm install -g) and starting/stopping local mini-app
 * services (OpenClaw, Hermes) from the Electron main process.
 */

import { spawn, ChildProcess } from 'node:child_process';
import * as net from 'node:net';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import { createHash, randomBytes } from 'node:crypto';
import { app, dialog, session } from 'electron';
import log from 'electron-log';
import { deleteAllMiniAppSecrets, getMiniAppSecretEnvironment, listMiniAppSecrets, setMiniAppSecret } from './mini-app-secrets.js';
import { sandboxCommand, type MiniAppSandboxPermissions } from './mini-app-sandbox.js';
import { startMiniAppPolicyProxy, type MiniAppPolicyProxy } from './mini-app-policy-proxy.js';
import { buildOAuthEnv, collectOAuthProviders } from './mini-app-oauth-inject.js';

// ─── Config ──────────────────────────────────────────────────────────────────

export interface MiniAppConfig {
  packageName: string;
  installCommand: string;
  installArgs: string[];
  /** Binary name after global install */
  binary: string;
  /** Args to pass when starting the service */
  startArgs: string[];
  port: number;
  statusArgs?: string[];
  harnessManaged?: boolean;
  dynamic?: boolean;
  appId?: string;
  requestedSecrets?: string[];
  workingDirectory?: string;
  permissions?: MiniAppSandboxPermissions;
  oauthProviders?: Array<{ providerId: string; envName: string }>;
}

export interface MiniAppRuntimeRegistration {
  id: string;
  name: string;
  version?: string;
  installCommand?: string;
  startCommand?: string;
  stopCommand?: string;
  healthUrl?: string;
  permissions?: { network?: string[]; filesystem?: string[]; secrets?: string[]; processes?: boolean };
  /** Manifest OAuth declaration (providerId -> provider config). Covered by
   * the approval fingerprint, so changing providers/scopes re-triggers approval. */
  oauth?: Record<string, unknown>;
}

type ApprovedRuntime = { registration: MiniAppRuntimeRegistration; fingerprint: string; approvedAt: string; previous?: Omit<ApprovedRuntime, 'previous'> };
const approvedRuntimes = new Map<string, ApprovedRuntime>();

function approvalsPath(): string {
  return path.join(app.getPath('userData'), 'mini-app-runtime-approvals.json');
}

function fingerprint(registration: MiniAppRuntimeRegistration): string {
  return createHash('sha256').update(JSON.stringify(registration)).digest('hex');
}

function loadApprovals(): void {
  if (approvedRuntimes.size) return;
  try {
    const stored = JSON.parse(fs.readFileSync(approvalsPath(), 'utf8')) as ApprovedRuntime[];
    for (const item of stored) {
      if (item.fingerprint === fingerprint(item.registration)) approvedRuntimes.set(item.registration.id, item);
    }
  } catch { /* First run or an invalid approval file starts empty. */ }
}

function persistApprovals(): void {
  fs.mkdirSync(path.dirname(approvalsPath()), { recursive: true });
  fs.writeFileSync(approvalsPath(), JSON.stringify([...approvedRuntimes.values()], null, 2), { mode: 0o600 });
}

function validateRegistration(registration: MiniAppRuntimeRegistration): string | null {
  if (!/^[a-z0-9][a-z0-9:._/-]{1,199}$/i.test(registration.id)) return 'Invalid miniapp identifier';
  for (const command of [registration.installCommand, registration.startCommand, registration.stopCommand].filter(Boolean) as string[]) {
    if (command.length > 4096 || command.includes('\0')) return 'Runtime command is invalid';
  }
  if (!registration.startCommand && !registration.installCommand) return 'No runtime commands were declared';
  if (!registration.permissions?.processes) return 'Process permission is required for command-based runtimes';
  for (const name of registration.permissions.secrets || []) {
    if (!/^[A-Z][A-Z0-9_]{1,127}$/.test(name)) return `Invalid secret name: ${name}`;
  }
  for (const location of registration.permissions.filesystem || []) {
    if (!(path.isAbsolute(location) || location.startsWith('~/'))) return `Filesystem permissions must use absolute or home-relative paths: ${location}`;
  }
  const oauth = collectOAuthProviders(registration.oauth);
  if (oauth.error) return oauth.error;
  return null;
}

function dynamicConfig(id: string): MiniAppConfig | null {
  loadApprovals();
  const approved = approvedRuntimes.get(id);
  if (!approved || approved.fingerprint !== fingerprint(approved.registration)) return null;
  const item = approved.registration;
  const requestedSecrets = item.permissions?.secrets || [];
  const missingSecrets = requestedSecrets.filter((name) => !listMiniAppSecrets(id).includes(name));
  if (missingSecrets.length) return null;
  const workingDirectory = path.join(app.getPath('userData'), 'mini-apps', createHash('sha256').update(id).digest('hex').slice(0, 24));
  fs.mkdirSync(workingDirectory, { recursive: true, mode: 0o700 });
  const port = (() => { try { return Number(new URL(item.healthUrl || '').port) || 0; } catch { return 0; } })();
  return {
    packageName: `${item.name}${item.version ? ` ${item.version}` : ''}`,
    installCommand: '/bin/sh', installArgs: ['-lc', item.installCommand || 'true'],
    binary: '/bin/sh', startArgs: ['-lc', item.startCommand || 'true'], port,
    harnessManaged: !item.startCommand,
    dynamic: true, appId: id, requestedSecrets, workingDirectory, permissions: item.permissions,
    oauthProviders: collectOAuthProviders(item.oauth).providers,
  };
}

function processEnvironment(config: MiniAppConfig): NodeJS.ProcessEnv {
  if (!config.dynamic || !config.appId) return { ...process.env };
  const environment: NodeJS.ProcessEnv = {};
  for (const name of ['PATH', 'HOME', 'USER', 'SHELL', 'TMPDIR', 'LANG']) {
    if (process.env[name]) environment[name] = process.env[name];
  }
  // Firmlinked TMPDIR (/var/folders/…) canonicalized to the backing-store
  // form as belt-and-braces (profile ancestor literals already make the
  // firmlink form traversable, but older profiles may not have them).
  if (environment.TMPDIR) {
    try {
      environment.TMPDIR = fs.realpathSync(environment.TMPDIR);
    } catch { /* keep the original value */ }
  }
  return { ...environment, ...getMiniAppSecretEnvironment(config.appId, config.requestedSecrets || []), ALLTERNIT_MINIAPP_ID: config.appId };
}

// ─── Vault Viewer (vendored WebObsidian) ────────────────────────────────────
// See services/vault-viewer/PROVENANCE.md for the full integration writeup
// (why vendored, auth model, config surface).

let resolvedVaultViewerRoot: string | null | undefined;

function resolveVaultViewerRoot(): string | null {
  if (resolvedVaultViewerRoot !== undefined) return resolvedVaultViewerRoot;
  const candidates = app.isPackaged
    ? [path.join(process.resourcesPath ?? '', 'vault-viewer')]
    : [path.join(app.getAppPath(), '..', '..', 'services', 'vault-viewer')];
  for (const candidate of candidates) {
    try {
      if (fs.existsSync(candidate)) {
        resolvedVaultViewerRoot = candidate;
        return candidate;
      }
    } catch {
      // keep checking
    }
  }
  resolvedVaultViewerRoot = null;
  return null;
}

/**
 * Matches gizzi-code's own DEFAULT_BRAIN_PATH (cmd/gizzi-code/src/cli/commands/brain/lib.ts).
 * Known v1 limitation: does not read a customized `userSettings.brain.path`
 * override (that lives in gizzi-code's own settings store, not reachable
 * from this process without a cross-service call) — the default covers the
 * overwhelmingly common case.
 */
function vaultViewerVaultPath(): string {
  return path.join(os.homedir(), 'brain');
}

function vaultViewerEnvironment(): NodeJS.ProcessEnv {
  const vaultPath = vaultViewerVaultPath();
  if (!listMiniAppSecrets('vault-viewer').includes('WEBOBSIDIAN_PASSWORD')) {
    setMiniAppSecret('vault-viewer', 'WEBOBSIDIAN_PASSWORD', randomBytes(24).toString('hex'));
  }
  const { WEBOBSIDIAN_PASSWORD } = getMiniAppSecretEnvironment('vault-viewer', ['WEBOBSIDIAN_PASSWORD']);
  return {
    VAULT_PATH: vaultPath,
    ALLOWED_ROOTS: vaultPath,
    DATA_DIR: path.join(app.getPath('userData'), 'vault-viewer-data'),
    HOST: '127.0.0.1',
    PORT: '8787',
    WEBOBSIDIAN_PASSWORD,
  };
}

/**
 * Logs in with the generated WEBOBSIDIAN_PASSWORD, one-time-promotes it to
 * the stored user password if this is the first login (so `mustChangePassword`
 * never becomes true and the forced password-change screen never shows —
 * see PROVENANCE.md's "Auth model" section), then injects the resulting
 * session cookie directly into the mini app's Electron partition so the
 * embedded webview loads already authenticated.
 */
async function authenticateVaultViewer(port: number): Promise<{ success: boolean; error?: string }> {
  const { WEBOBSIDIAN_PASSWORD: password } = getMiniAppSecretEnvironment('vault-viewer', ['WEBOBSIDIAN_PASSWORD']);
  if (!password) return { success: false, error: 'vault-viewer password unavailable' };
  const base = `http://127.0.0.1:${port}`;
  try {
    const loginRes = await fetch(`${base}/auth/login`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ password }),
    });
    if (!loginRes.ok) return { success: false, error: `vault-viewer login failed (${loginRes.status})` };
    const loginBody = (await loginRes.json()) as { mustChangePassword?: boolean };

    const setCookieHeaders =
      typeof (loginRes.headers as { getSetCookie?: () => string[] }).getSetCookie === 'function'
        ? (loginRes.headers as unknown as { getSetCookie: () => string[] }).getSetCookie()
        : [loginRes.headers.get('set-cookie') || ''];
    const tokenCookie = setCookieHeaders.find((c) => c.startsWith('webobsidian_token='));
    if (!tokenCookie) return { success: false, error: 'vault-viewer login did not return a session cookie' };
    const token = tokenCookie.split(';')[0]!.split('=')[1]!;

    if (loginBody.mustChangePassword) {
      await fetch(`${base}/auth/change-password`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', cookie: `webobsidian_token=${token}` },
        body: JSON.stringify({ currentPassword: password, newPassword: password }),
      });
    }

    const expirationDate = Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60;
    await session.fromPartition('persist:allternit-vault-viewer').cookies.set({
      url: base,
      name: 'webobsidian_token',
      value: token,
      httpOnly: true,
      expirationDate,
    });
    return { success: true };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

function resolveConfig(id: string): MiniAppConfig | undefined {
  const base = MINI_APP_CONFIGS[id];
  if (base && id === 'vault-viewer') {
    return { ...base, workingDirectory: resolveVaultViewerRoot() ?? undefined };
  }
  return base || dynamicConfig(id) || undefined;
}

// ─── OAuth token injection ───────────────────────────────────────────────────
// The broker lives in unified-main; it registers a resolver here at startup to
// avoid a circular import. Tokens are resolved in the main process and go
// straight into the child environment — they never cross IPC.
type OAuthTokenResolver = (appId: string, providerId: string) => Promise<string | null>;
let oauthTokenResolver: OAuthTokenResolver | null = null;
export function setMiniAppOAuthTokenResolver(resolver: OAuthTokenResolver): void {
  oauthTokenResolver = resolver;
}

async function oauthEnvironment(config: MiniAppConfig): Promise<Record<string, string>> {
  if (!config.dynamic || !config.appId || !config.oauthProviders?.length || !oauthTokenResolver) return {};
  const tokens: Record<string, string | null> = {};
  for (const provider of config.oauthProviders) {
    try {
      tokens[provider.envName] = await oauthTokenResolver(config.appId, provider.providerId);
    } catch (error) {
      log.warn(`[mini-apps] OAuth token for ${config.appId}/${provider.providerId} unavailable:`, error);
      tokens[provider.envName] = null;
    }
  }
  return buildOAuthEnv(tokens);
}

export function getMiniAppApproval(id: string, registration?: MiniAppRuntimeRegistration): { approved: boolean; fingerprint?: string; approvedAt?: string } {
  loadApprovals();
  const approved = approvedRuntimes.get(id);
  const matches = Boolean(approved && (!registration || approved.fingerprint === fingerprint(registration)));
  return { approved: matches, fingerprint: registration ? fingerprint(registration) : approved?.fingerprint, approvedAt: matches ? approved?.approvedAt : undefined };
}

export async function reviewAndApproveMiniApp(registration: MiniAppRuntimeRegistration): Promise<{ success: boolean; approved: boolean; fingerprint?: string; error?: string }> {
  const error = validateRegistration(registration);
  if (error) return { success: false, approved: false, error };
  const permissions = registration.permissions;
  const detail = [
    registration.installCommand && `Install: ${registration.installCommand}`,
    registration.startCommand && `Start: ${registration.startCommand}`,
    registration.stopCommand && `Stop: ${registration.stopCommand}`,
    permissions?.network?.length && `Network: ${permissions.network.join(', ')}`,
    permissions?.filesystem?.length && `Files: ${permissions.filesystem.join(', ')}`,
    permissions?.secrets?.length && `Secrets: ${permissions.secrets.join(', ')}`,
    permissions?.processes && 'May launch local processes',
  ].filter(Boolean).join('\n\n');
  const response = await dialog.showMessageBox({ type: 'warning', title: `Approve ${registration.name}`, message: 'Review this miniapp runtime', detail, buttons: ['Cancel', 'Approve runtime'], defaultId: 0, cancelId: 0, noLink: true });
  if (response.response !== 1) return { success: true, approved: false };
  const current = approvedRuntimes.get(registration.id);
  const approved: ApprovedRuntime = {
    registration,
    fingerprint: fingerprint(registration),
    approvedAt: new Date().toISOString(),
    previous: current ? { registration: current.registration, fingerprint: current.fingerprint, approvedAt: current.approvedAt } : undefined,
  };
  approvedRuntimes.set(registration.id, approved); persistApprovals();
  return { success: true, approved: true, fingerprint: approved.fingerprint };
}

export function revokeMiniAppApproval(id: string): void {
  loadApprovals(); approvedRuntimes.delete(id); persistApprovals();
}

export function rollbackMiniAppRuntime(id: string): { success: boolean; error?: string } {
  loadApprovals();
  const current = approvedRuntimes.get(id);
  if (!current?.previous) return { success: false, error: 'No previously approved runtime is available' };
  stopMiniApp(id);
  approvedRuntimes.set(id, { ...current.previous });
  persistApprovals();
  return { success: true };
}

export function removeMiniAppRuntime(id: string): { success: boolean; error?: string } {
  stopMiniApp(id);
  revokeMiniAppApproval(id);
  deleteAllMiniAppSecrets(id);
  const workingDirectory = path.join(app.getPath('userData'), 'mini-apps', createHash('sha256').update(id).digest('hex').slice(0, 24));
  try {
    fs.rmSync(workingDirectory, { recursive: true, force: true });
    return { success: true };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export const MINI_APP_CONFIGS: Record<string, MiniAppConfig> = {
  openclaw: {
    packageName: 'openclaw@latest',
    installCommand: 'npm',
    installArgs: ['install', '-g', 'openclaw@latest'],
    binary: 'openclaw',
    startArgs: ['gateway', '--port', '18789'],
    port: 18789,
  },
  hermes: {
    packageName: 'NousResearch/hermes-agent',
    installCommand: '/bin/bash',
    installArgs: ['-lc', 'curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash'],
    binary: 'hermes',
    startArgs: ['dashboard', '--no-open', '--port', '9119'],
    port: 9119,
  },
  'oh-my-pi': {
    packageName: 'can1357/oh-my-pi',
    installCommand: '/bin/bash',
    installArgs: ['-lc', 'curl -fsSL https://omp.sh/install | sh'],
    binary: 'omp',
    startArgs: ['acp'],
    port: 0,
    harnessManaged: true,
  },
  // Vendored WebObsidian — see services/vault-viewer/PROVENANCE.md. Unlike
  // the above (globally npm/curl-installed binaries), this is a vendored
  // monorepo directory needing a one-time local build; `workingDirectory`
  // is injected lazily by resolveConfig() since it depends on app.isPackaged.
  'vault-viewer': {
    packageName: 'vault-viewer',
    installCommand: '/bin/bash',
    installArgs: ['-lc', 'npm install && npm run build'],
    binary: 'node',
    startArgs: ['server/dist/index.js'],
    port: 8787,
  },
};

// ─── State ────────────────────────────────────────────────────────────────────

interface RunningApp {
  id: string;
  process: ChildProcess;
  port: number;
  proxy?: MiniAppPolicyProxy;
}

const runningApps = new Map<string, RunningApp>();

// ─── Helpers ──────────────────────────────────────────────────────────────────

function isPortOpen(port: number, timeoutMs = 3000): Promise<boolean> {
  return new Promise((resolve) => {
    const sock = new net.Socket();
    let done = false;
    const finish = (ok: boolean) => {
      if (done) return;
      done = true;
      sock.destroy();
      resolve(ok);
    };
    sock.setTimeout(timeoutMs);
    sock.on('connect', () => finish(true));
    sock.on('error', () => finish(false));
    sock.on('timeout', () => finish(false));
    sock.connect(port, '127.0.0.1');
  });
}

function commandSucceeds(binary: string, args: string[]): Promise<boolean> {
  return new Promise((resolve) => {
    const proc = spawn(binary, args, { env: { ...process.env }, shell: true, stdio: 'ignore' });
    proc.once('error', () => resolve(false));
    proc.once('close', (code) => resolve(code === 0));
  });
}

// ─── Install ──────────────────────────────────────────────────────────────────

export type InstallProgress = {
  id: string;
  line: string;
  type: 'stdout' | 'stderr' | 'info';
};

export type InstallResult = {
  success: boolean;
  error?: string;
};

/**
 * Installs a mini-app via npm install -g <package>.
 * Calls onProgress with each line of output.
 */
export async function installMiniApp(
  id: string,
  onProgress: (p: InstallProgress) => void,
): Promise<InstallResult> {
  const config = resolveConfig(id);
  if (!config) {
    return { success: false, error: `Unknown mini-app: ${id}` };
  }

  onProgress({ id, line: `Installing ${config.packageName}…`, type: 'info' });
  const command = config.dynamic
    // Install-time dependency resolution legitimately needs package
    // registries beyond the runtime host list; steady-state execution never
    // gets full network (see startMiniApp / verifyHealthy).
    ? sandboxCommand(config.installCommand, config.installArgs, config.workingDirectory!, config.permissions || {}, 'full')
    : { binary: config.installCommand, args: config.installArgs };
  if ('error' in command) return { success: false, error: command.error };

  return new Promise((resolve) => {
    const proc = spawn(command.binary, command.args, {
      env: { ...processEnvironment(config), ...(id === 'vault-viewer' ? vaultViewerEnvironment() : {}) },
      cwd: config.workingDirectory,
      shell: !config.dynamic,
    });

    proc.stdout?.on('data', (chunk: Buffer) => {
      for (const line of chunk.toString().split('\n')) {
        if (line.trim()) onProgress({ id, line: line.trim(), type: 'stdout' });
      }
    });

    proc.stderr?.on('data', (chunk: Buffer) => {
      for (const line of chunk.toString().split('\n')) {
        if (line.trim()) onProgress({ id, line: line.trim(), type: 'stderr' });
      }
    });

    proc.on('close', (code) => {
      if (code === 0) {
        log.info(`[mini-apps] ${id} installed successfully`);
        onProgress({ id, line: `✓ ${config.packageName} installed`, type: 'info' });
        resolve({ success: true });
      } else {
        const msg = `npm exited with code ${code}`;
        log.warn(`[mini-apps] ${id} install failed: ${msg}`);
        resolve({ success: false, error: msg });
      }
    });

    proc.on('error', (err) => {
      log.error(`[mini-apps] ${id} install error:`, err);
      resolve({ success: false, error: err.message });
    });
  });
}

// ─── Start ────────────────────────────────────────────────────────────────────

export async function startMiniApp(
  id: string,
  onProgress?: (p: InstallProgress) => void,
): Promise<{ success: boolean; error?: string }> {
  const config = resolveConfig(id);
  if (!config) return { success: false, error: `Unknown mini-app: ${id}` };
  if (config.harnessManaged) {
    return { success: false, error: `${config.binary} is started by the Allternit/Gizzi harness when a session begins` };
  }

  if (runningApps.has(id)) {
    const alreadyUp = config.port === 0 || await isPortOpen(config.port, 1000);
    if (alreadyUp) return { success: true };
    runningApps.delete(id);
  }

  onProgress?.({ id, line: `Starting ${config.binary}…`, type: 'info' });

  // Community runtimes with a declared network permission get loopback-only
  // sandbox network access; all external egress goes through the per-session
  // policy proxy, which enforces the approved hostname list.
  let proxy: MiniAppPolicyProxy | undefined;
  if (config.dynamic && config.permissions?.network?.length) {
    try {
      proxy = await startMiniAppPolicyProxy({ appId: id, allowedHosts: config.permissions.network });
    } catch (error) {
      return { success: false, error: `Network policy proxy failed to start: ${error instanceof Error ? error.message : String(error)}` };
    }
  }

  const command = config.dynamic
    ? sandboxCommand(config.binary, config.startArgs, config.workingDirectory!, config.permissions || {})
    : { binary: config.binary, args: config.startArgs };
  if ('error' in command) {
    await proxy?.close();
    return { success: false, error: command.error };
  }

  const proc = spawn(command.binary, command.args, {
    env: {
      ...processEnvironment(config),
      ...(proxy?.environment || {}),
      ...(await oauthEnvironment(config)),
      ...(id === 'vault-viewer' ? vaultViewerEnvironment() : {}),
    },
    cwd: config.workingDirectory,
    shell: !config.dynamic,
    detached: false,
  });

  proc.stdout?.on('data', (chunk: Buffer) => {
    log.info(`[mini-apps:${id}]`, chunk.toString().trim());
  });
  proc.stderr?.on('data', (chunk: Buffer) => {
    log.warn(`[mini-apps:${id}]`, chunk.toString().trim());
  });

  proc.on('close', (code) => {
    log.info(`[mini-apps] ${id} exited with code ${code}`);
    void proxy?.close();
    runningApps.delete(id);
  });

  proc.on('error', (err) => {
    log.error(`[mini-apps] ${id} start error:`, err);
    void proxy?.close();
    runningApps.delete(id);
  });

  runningApps.set(id, { id, process: proc, port: config.port, proxy });

  if (config.port === 0) {
    // Hermes starts a managed gateway daemon and exits; OMP remains attached as
    // an RPC child. Both are verified through their upstream lifecycle shape.
    if (config.statusArgs) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      const up = await commandSucceeds(config.binary, config.statusArgs);
      if (!up) {
        await proxy?.close();
        return { success: false, error: `${config.binary} gateway did not report a running status` };
      }
    }
    onProgress?.({ id, line: `✓ ${config.binary} runtime started`, type: 'info' });
    return { success: true };
  }

  // Poll until port is open (max 10 seconds)
  for (let i = 0; i < 20; i++) {
    await new Promise((r) => setTimeout(r, 500));
    const up = await isPortOpen(config.port, 500);
    if (up) {
      if (id === 'vault-viewer') {
        const auth = await authenticateVaultViewer(config.port);
        if (!auth.success) {
          // Non-fatal: the webview falls back to showing its own login
          // screen (degraded, but not broken) if cookie priming fails.
          onProgress?.({ id, line: `⚠ vault-viewer auto-login failed: ${auth.error}`, type: 'stderr' });
        }
      }
      onProgress?.({ id, line: `✓ ${config.binary} is running on :${config.port}`, type: 'info' });
      return { success: true };
    }
  }

  await proxy?.close();
  return { success: false, error: `${config.binary} did not bind to port ${config.port} within 10 s` };
}

// ─── Stop ─────────────────────────────────────────────────────────────────────

export function stopMiniApp(id: string): void {
  const app = runningApps.get(id);
  if (app) {
    try { app.process.kill('SIGTERM'); } catch { /* ignore */ }
    void app.proxy?.close();
    runningApps.delete(id);
  }
  loadApprovals();
  const registration = approvedRuntimes.get(id)?.registration;
  const stopCommand = registration?.stopCommand;
  if (stopCommand) {
    const config = dynamicConfig(id);
    const command = config && sandboxCommand('/bin/sh', ['-lc', stopCommand], config.workingDirectory!, config.permissions || {});
    if (!command || 'error' in command) {
      log.warn(`[mini-apps] ${id} stop command blocked because its sandbox is unavailable`);
      return;
    }
    const proc = spawn(command.binary, command.args, { env: processEnvironment(config), cwd: config.workingDirectory, shell: false, detached: false });
    proc.once('error', (error) => log.warn(`[mini-apps] ${id} stop command failed`, error));
  }
  log.info(`[mini-apps] ${id} stopped`);
}

export function launchMiniAppDesktop(id: string): { success: boolean; error?: string } {
  if (id !== 'hermes') return { success: false, error: `No desktop launcher for mini-app: ${id}` };
  try {
    const proc = spawn('hermes', ['desktop'], { env: { ...process.env }, shell: true, detached: true, stdio: 'ignore' });
    proc.unref();
    return { success: true };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

// ─── Status ──────────────────────────────────────────────────────────────────

export async function getMiniAppStatus(id: string): Promise<{
  managed: boolean;
  running: boolean;
  port: number | null;
}> {
  const config = resolveConfig(id);
  if (!config) return { managed: false, running: false, port: null };

  const tracked = runningApps.has(id);
  const portOpen = config.statusArgs
    ? await commandSucceeds(config.binary, config.statusArgs)
    : config.port === 0 ? tracked : await isPortOpen(config.port, 800);
  return { managed: tracked, running: portOpen, port: config.port || null };
}

export function listKnownIds(): string[] {
  loadApprovals();
  return [...new Set([...Object.keys(MINI_APP_CONFIGS), ...approvedRuntimes.keys()])];
}
