/**
 * Linux app updates for Allternit Desktop.
 *
 * Electron's built-in autoUpdater (used on macOS and Windows through
 * update-electron-app) has no Linux support, so a Linux install never updated.
 * On Linux only, this module runs electron-updater's DebUpdater against the
 * same GitHub releases (Allternit/desktop): it reads latest-linux.yml, downloads
 * the .deb, and on Restart installs it with `pkexec dpkg -i` (the desktop's
 * polkit agent asks for the password), then relaunches.
 *
 * Its events go to the same `app:update-status` broadcasts as the macOS and
 * Windows handlers, so the UI and the restart prompt are identical. When the
 * install cannot elevate (no pkexec, the prompt was cancelled, no polkit agent)
 * a notification opens the release page instead. Nothing fails silently.
 *
 * Desktop 1.1.15 and older have no Linux updater: those users install the next
 * .deb by hand once (`sudo apt install ./Allternit-Desktop-<v>-amd64.deb`).
 * Every version after that updates itself.
 *
 * Lab override: ALLTERNIT_LINUX_UPDATE_FEED=http://host:port/ points the
 * updater at a generic feed that serves latest-linux.yml and the .deb.
 */
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { app, dialog, Notification } from 'electron';
import type { BaseUpdater, UpdateInfo } from 'electron-updater';
import { openExternalAllowlisted } from './security.js';
import {
  chooseLinuxUpdateMode,
  detectElevation,
  fallbackNotice,
  LINUX_FEED_OVERRIDE_ENV,
  LINUX_UPDATE_INTERVAL_MS,
  releasePageUrl,
  resolveLinuxFeed,
  shouldAutoDownload,
  type LinuxFallbackReason,
} from './linux-updater-logic.js';

export type LinuxUpdateStatus =
  | { state: 'checking' }
  | { state: 'available' }
  | { state: 'up-to-date' }
  | { state: 'downloaded'; version?: string; releaseNotes?: string; updateURL?: string }
  | { state: 'error'; message: string };

type Logger = {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
  error: (...args: unknown[]) => void;
  debug?: (...args: unknown[]) => void;
};

export interface LinuxUpdaterDeps {
  logger: Logger;
  /** The same broadcast the macOS/Windows autoUpdater handlers use. */
  broadcast: (status: LinuxUpdateStatus) => void;
  /** Marks the app update as downloaded (unified-main's appUpdateDownloaded). */
  onDownloaded: () => void;
}

export interface LinuxUpdater {
  /** Check now. `manual` (menu or Settings) shows the fallback notice again. */
  check: (manual?: boolean) => void;
  /**
   * Install the downloaded .deb (asks for the password through pkexec) and
   * schedule the relaunch. Returns false, after showing the fallback notice,
   * when it could not install; the app keeps running then.
   */
  install: () => Promise<boolean>;
}

const TAG = '[linuxUpdater]';

// Held until clicked or closed so the click handler is not garbage-collected.
const liveNotices = new Set<Notification>();

function hasCommand(cmd: string): boolean {
  try {
    // Same probe as electron-updater's LinuxUpdater.hasCommand. cmd is one of
    // our constant names, never user input.
    execFileSync('/bin/sh', ['-c', `command -v ${cmd}`], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function readPackageType(): string | null {
  try {
    return readFileSync(join(process.resourcesPath, 'package-type'), 'utf8');
  } catch {
    return null;
  }
}

function showFallback(log: Logger, reason: LinuxFallbackReason, version?: string | null): void {
  const notice = fallbackNotice(reason, version);
  log.warn(`${TAG} fallback (${reason}): ${notice.title} -> ${notice.url}`);
  try {
    if (Notification.isSupported()) {
      const n = new Notification({ title: notice.title, body: notice.body });
      liveNotices.add(n);
      n.on('click', () => {
        liveNotices.delete(n);
        openExternalAllowlisted(notice.url);
      });
      n.on('close', () => liveNotices.delete(n));
      n.show();
      return;
    }
  } catch (error) {
    log.warn(`${TAG} notification failed, using a dialog`, error);
  }
  void dialog
    .showMessageBox({
      type: 'info',
      message: notice.title,
      detail: notice.body,
      buttons: ['Open download page', 'Later'],
      defaultId: 0,
      cancelId: 1,
    })
    .then(({ response }) => {
      if (response === 0) openExternalAllowlisted(notice.url);
    })
    .catch((error) => log.error(`${TAG} fallback dialog failed`, error));
}

/**
 * Start the Linux updater. Returns null on macOS/Windows, in dev builds, and
 * on Linux installs that are not the .deb (those cannot self-update).
 */
export function startLinuxUpdater(deps: LinuxUpdaterDeps): LinuxUpdater | null {
  const log = deps.logger;
  const mode = chooseLinuxUpdateMode({
    platform: process.platform,
    isPackaged: app.isPackaged,
    packageType: process.platform === 'linux' && app.isPackaged ? readPackageType() : null,
  });
  if (mode === 'off') return null;
  if (mode === 'unsupported') {
    log.warn(`${TAG} not a .deb install (no resources/package-type=deb): app updates are off; download new versions from ${releasePageUrl()}`);
    return null;
  }

  const elevation = detectElevation({ isRoot: process.getuid?.() === 0, hasCommand });
  const autoDownload = shouldAutoDownload(elevation);
  const { feed, ignoredOverride } = resolveLinuxFeed(process.env[LINUX_FEED_OVERRIDE_ENV]);
  if (ignoredOverride) log.warn(`${TAG} ignoring ${LINUX_FEED_OVERRIDE_ENV}=${ignoredOverride}: not an http(s) URL`);
  log.info(`${TAG} deb updater: feed ${JSON.stringify(feed)}, elevation ${JSON.stringify(elevation)}, autoDownload ${autoDownload}`);

  let installing = false;
  let installErrorSeen = false;
  let noticeShownFor: string | null = null;
  let downloadedVersion: string | null = null;

  const ready: Promise<BaseUpdater | null> = (async () => {
    // Loaded only on Linux so macOS/Windows never load electron-updater.
    const mod = await import('electron-updater');
    const DebUpdaterClass = mod.DebUpdater ?? (mod as unknown as { default: typeof mod }).default.DebUpdater;
    const updater: BaseUpdater = new DebUpdaterClass();
    updater.logger = log;
    updater.setFeedURL(feed);
    updater.autoDownload = autoDownload;
    // Install only when the user clicks Restart: a password prompt during
    // quit or logout would be a surprise (and could hold the session open).
    updater.autoInstallOnAppQuit = false;
    updater.autoRunAppAfterInstall = true;

    updater.on('checking-for-update', () => deps.broadcast({ state: 'checking' }));
    updater.on('update-available', (info: UpdateInfo) => {
      deps.broadcast({ state: 'available' });
      if (!autoDownload && noticeShownFor !== info.version) {
        noticeShownFor = info.version;
        showFallback(log, 'no-elevation', info.version);
      }
    });
    updater.on('update-not-available', () => deps.broadcast({ state: 'up-to-date' }));
    updater.on('update-downloaded', (info: UpdateInfo) => {
      log.info(`${TAG} downloaded ${info.version}; waiting for Restart`);
      downloadedVersion = info.version;
      deps.onDownloaded();
      deps.broadcast({
        state: 'downloaded',
        version: info.version,
        releaseNotes: typeof info.releaseNotes === 'string' ? info.releaseNotes : undefined,
        updateURL: releasePageUrl(info.version),
      });
    });
    updater.on('error', (error: Error) => {
      log.error(TAG, error);
      deps.broadcast({ state: 'error', message: error?.message ?? String(error) });
      if (installing && !installErrorSeen) {
        installErrorSeen = true;
        showFallback(log, 'install-failed', downloadedVersion);
      }
    });
    return updater;
  })().catch((error) => {
    log.error(`${TAG} could not start electron-updater`, error);
    deps.broadcast({ state: 'error', message: `Linux updater failed to start: ${error?.message ?? String(error)}` });
    return null;
  });

  const check = (manual = false) => {
    if (manual) noticeShownFor = null;
    void ready.then((u) =>
      u?.checkForUpdates().catch((error: unknown) => {
        // Also reported through the 'error' event; logged here so a rejected
        // promise is never unhandled.
        log.warn(`${TAG} check failed`, error);
      }),
    );
  };

  void app.whenReady().then(() => {
    check();
    setInterval(() => check(), LINUX_UPDATE_INTERVAL_MS).unref?.();
  });

  return {
    check,
    install: async () => {
      const updater = await ready;
      if (!updater) {
        showFallback(log, 'install-failed', downloadedVersion);
        return false;
      }
      installing = true;
      installErrorSeen = false;
      let ok = false;
      try {
        // Synchronous: runs `pkexec /bin/bash -c 'dpkg -i <deb>'` and calls
        // app.relaunch() when it succeeds. The caller then quits.
        ok = updater.install(false, true);
      } catch (error) {
        log.error(`${TAG} install threw`, error);
      } finally {
        installing = false;
      }
      if (!ok) {
        // install() leaves its once-only guard set after a failure; clear it
        // so Restart can try again (e.g. after a cancelled password prompt).
        (updater as unknown as { quitAndInstallCalled: boolean }).quitAndInstallCalled = false;
        if (!installErrorSeen) showFallback(log, 'install-failed', downloadedVersion);
        log.warn(`${TAG} install failed; the app keeps running`);
        return false;
      }
      log.info(`${TAG} installed ${downloadedVersion ?? 'update'}; quitting to relaunch`);
      return true;
    },
  };
}
