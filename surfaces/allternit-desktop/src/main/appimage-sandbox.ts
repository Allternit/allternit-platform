/**
 * AppImage sandbox check for Allternit Desktop on Linux. Runs once after
 * `ready` and before the first window (unified-main's startup gate).
 *
 * electron-builder's AppRun starts the app with --no-sandbox when
 * `unshare -Ur true` fails (user namespaces blocked), so the AppImage never
 * crashes at start. This module notices that state and, when AppArmor is the
 * cause, offers to install a `userns,` profile for this AppImage through
 * pkexec and relaunches; otherwise it keeps running without the sandbox, logs
 * it, and shows one notice recommending the .deb or .rpm. The decisions live
 * in appimage-sandbox-logic.ts (unit-tested).
 */
import { spawn, spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { app, dialog, Notification } from 'electron';
import {
  APPIMAGE_APPARMOR_PROFILE_PATH,
  buildAppImageAppArmorProfile,
  decideAppImageSandbox,
  LINUX_DOWNLOAD_PAGE,
  noSandboxNotice,
  relaunchArgs,
  relaunchEnv,
  SANDBOX_PROMPT_ENV,
  SANDBOX_RELAUNCH_ENV,
  type AppImageSandboxState,
  type SandboxCause,
  type UsernsProbe,
} from './appimage-sandbox-logic.js';

type Logger = { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void; error: (...a: unknown[]) => void };

const TAG = '[appimageSandbox]';
const STATE_FILE = 'appimage-sandbox.json';
const liveNotices = new Set<Notification>();

function readText(path: string): string | null {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return null;
  }
}

function hasCommand(cmd: string): boolean {
  // cmd is one of our constant names, never user input.
  return spawnSync('/bin/sh', ['-c', `command -v ${cmd}`], { stdio: 'ignore' }).status === 0;
}

function probeUserns(): UsernsProbe {
  if (!hasCommand('unshare')) return 'missing';
  const r = spawnSync('unshare', ['-Ur', 'true'], { stdio: 'ignore', timeout: 5000 });
  return r.status === 0 ? 'ok' : 'failed';
}

function statePath(): string {
  return join(app.getPath('userData'), STATE_FILE);
}

function readState(): AppImageSandboxState {
  try {
    return JSON.parse(readFileSync(statePath(), 'utf8')) as AppImageSandboxState;
  } catch {
    return {};
  }
}

function writeState(patch: AppImageSandboxState, log: Logger): void {
  try {
    writeFileSync(statePath(), JSON.stringify({ ...readState(), ...patch }));
  } catch (error) {
    log.warn(`${TAG} could not save state`, error);
  }
}

function showNotice(cause: SandboxCause, log: Logger): void {
  const { title, body } = noSandboxNotice(cause);
  try {
    if (Notification.isSupported()) {
      const n = new Notification({ title, body });
      liveNotices.add(n);
      n.on('click', () => {
        liveNotices.delete(n);
        void import('./security.js').then((m) => m.openExternalAllowlisted(LINUX_DOWNLOAD_PAGE)).catch(() => {});
      });
      n.on('close', () => liveNotices.delete(n));
      n.show();
      return;
    }
  } catch (error) {
    log.warn(`${TAG} notification failed`, error);
  }
  log.warn(`${TAG} ${title}: ${body}`);
}

/** Write the profile through pkexec (the desktop's polkit agent asks for the password). */
function installProfile(appImagePath: string, log: Logger): boolean {
  const profile = buildAppImageAppArmorProfile(appImagePath);
  if (!profile) return false;
  const dir = mkdtempSync(join(tmpdir(), 'allternit-aa-'));
  const file = join(dir, 'profile');
  try {
    writeFileSync(file, profile);
    chmodSync(dir, 0o755);
    chmodSync(file, 0o644);
    const script = `install -m 0644 "$1" '${APPIMAGE_APPARMOR_PROFILE_PATH}' && apparmor_parser --replace --write-cache --skip-read-cache '${APPIMAGE_APPARMOR_PROFILE_PATH}'`;
    const r = spawnSync('pkexec', ['/bin/sh', '-c', script, 'sh', file], { encoding: 'utf8', timeout: 5 * 60_000 });
    if (r.status === 0) {
      log.info(`${TAG} installed ${APPIMAGE_APPARMOR_PROFILE_PATH} for ${appImagePath}`);
      return true;
    }
    // 126: the password prompt was dismissed; 127: no polkit agent / not authorized.
    log.warn(`${TAG} profile install failed (pkexec exit ${r.status}): ${(r.stderr || '').trim().slice(0, 500)}`);
    return false;
  } catch (error) {
    log.warn(`${TAG} profile install failed`, error);
    return false;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * Start the AppImage again once this process has exited (the single-instance
 * lock and the FUSE mount both belong to this process until then).
 */
function relaunch(appImagePath: string, log: Logger): void {
  const args = relaunchArgs(process.argv);
  log.info(`${TAG} relaunching ${appImagePath} ${args.join(' ')}`);
  const child = spawn(
    '/bin/sh',
    ['-c', 'while kill -0 "$ALLTERNIT_OLD_PID" 2>/dev/null; do sleep 0.2; done; exec "$@"', 'sh', appImagePath, ...args],
    { detached: true, stdio: 'ignore', env: relaunchEnv(process.env, process.pid) },
  );
  child.unref();
  app.exit(0);
}

/**
 * Returns false when the app is relaunching (the caller stops starting up),
 * true otherwise. Never throws: any failure here only costs the sandbox.
 */
export async function passAppImageSandboxGate(log: Logger): Promise<boolean> {
  try {
    const appImagePath = process.env.APPIMAGE?.trim() || null;
    if (process.platform !== 'linux' || !appImagePath) return true;
    const noSandboxSwitch = app.commandLine.hasSwitch('no-sandbox') || process.argv.includes('--no-sandbox');
    const usernsProbe = noSandboxSwitch ? probeUserns() : 'ok';
    const state = readState();
    const decision = decideAppImageSandbox({
      platform: process.platform,
      appImagePath,
      noSandboxSwitch,
      usernsProbe,
      apparmorRestrictUserns: readText('/proc/sys/kernel/apparmor_restrict_unprivileged_userns'),
      unprivilegedUsernsClone: readText('/proc/sys/kernel/unprivileged_userns_clone'),
      maxUserNamespaces: readText('/proc/sys/user/max_user_namespaces'),
      apparmorEnabled: readText('/sys/module/apparmor/parameters/enabled')?.trim() === 'Y',
      hasPkexec: noSandboxSwitch && hasCommand('pkexec'),
      hasApparmorParser: noSandboxSwitch && (hasCommand('apparmor_parser') || hasCommand('/sbin/apparmor_parser') || hasCommand('/usr/sbin/apparmor_parser')),
      promptDisabled: process.env[SANDBOX_PROMPT_ENV] === '0',
      relaunchedAfterProfile: process.env[SANDBOX_RELAUNCH_ENV] === '1',
      state,
    });
    log.info(`${TAG} ${appImagePath}: no-sandbox=${noSandboxSwitch} userns=${usernsProbe} -> ${JSON.stringify(decision)}`);
    if (decision.action === 'none') return true;

    if (decision.action === 'offer-profile') {
      const { response } = await dialog.showMessageBox({
        type: 'warning',
        message: 'Turn on the security sandbox?',
        detail:
          'AppArmor on this computer blocks the Chromium sandbox for apps it does not know. '
          + 'Allternit can add an AppArmor rule for this AppImage (it asks for your password), then restart with the sandbox on.\n\n'
          + 'If you skip this, Allternit runs without the sandbox. The .deb install sets this up for you.',
        buttons: ['Add rule and restart', 'Run without sandbox'],
        defaultId: 0,
        cancelId: 1,
      });
      if (response === 0 && installProfile(appImagePath, log)) {
        relaunch(appImagePath, log);
        return false;
      }
      writeState({ declinedFor: appImagePath }, log);
      log.warn(`${TAG} running without the Chromium sandbox (${response === 0 ? 'profile install failed' : 'user declined'})`);
      if (state.noticeShownFor !== appImagePath) {
        showNotice('apparmor', log);
        writeState({ noticeShownFor: appImagePath }, log);
      }
      return true;
    }

    log.warn(`${TAG} running without the Chromium sandbox (cause: ${decision.cause})`);
    if (decision.showNotice) {
      showNotice(decision.cause, log);
      writeState({ noticeShownFor: appImagePath }, log);
    }
    return true;
  } catch (error) {
    log.error(`${TAG} check failed; continuing`, error);
    return true;
  }
}
