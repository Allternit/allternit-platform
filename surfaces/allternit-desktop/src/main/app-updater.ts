/**
 * App updates for Allternit Desktop on macOS, Windows and Linux.
 *
 * One updater everywhere: electron-updater against the GitHub releases of
 * Allternit/desktop (see app-updater-logic.ts for why each OS works the way
 * it does). It checks at start and every hour, downloads in the background,
 * and reports through the `app:update-status` broadcasts, so Settings and the
 * restart prompt work the same on every OS:
 *
 * - macOS: MacUpdater downloads the zip for this Mac from latest-mac.yml;
 *   mac-app-swap.ts stages and checks it, and swaps the bundle on Restart.
 *   Squirrel.Mac is never used (it refuses ad-hoc signed updates).
 * - Windows: NsisUpdater downloads Allternit-Setup-<v>.exe from latest.yml and
 *   runs it silently on Restart (or when the app quits), then relaunches.
 * - Linux .deb: DebUpdater downloads the .deb from latest-linux.yml and on
 *   Restart installs it with `pkexec dpkg -i` (the desktop's polkit agent asks
 *   for the password), then relaunches.
 *
 * When an install cannot happen (no pkexec, a cancelled prompt, macOS refusing
 * the swap, a missing installer) a notification opens the release page. The
 * app keeps running. Nothing fails silently.
 *
 * Desktop 1.1.15 and older used update-electron-app, which never updated
 * these builds: those users install the next version by hand once.
 *
 * Lab override: ALLTERNIT_UPDATE_FEED=http://host:port/ (Linux also accepts
 * ALLTERNIT_LINUX_UPDATE_FEED) points the updater at a generic feed that
 * serves the latest*.yml files and the packages.
 */
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { app, dialog, Notification } from 'electron';
import type { AppUpdater as ElectronAppUpdater, BaseUpdater, UpdateDownloadedEvent, UpdateInfo } from 'electron-updater';
import { openExternalAllowlisted } from './security.js';
import {
  chooseUpdateMode,
  detectElevation,
  fallbackNotice,
  installsOnQuit,
  LEGACY_LINUX_FEED_OVERRIDE_ENV,
  macBundleFromExecPath,
  needsElevation,
  releasePageUrl,
  resolveUpdateFeed,
  shouldAutoDownload,
  UPDATE_FEED_OVERRIDE_ENV,
  UPDATE_INTERVAL_MS,
  type AppUpdateMode,
  type UpdateFallbackReason,
} from './app-updater-logic.js';
import { cleanupOldMacBundles, stageMacUpdate, swapMacBundle } from './mac-app-swap.js';

export type AppUpdateStatus =
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

export interface AppUpdaterDeps {
  logger: Logger;
  /** unified-main's broadcastUpdateStatus (app:update-status to every window). */
  broadcast: (status: AppUpdateStatus) => void;
  /** Marks the app update as downloaded (unified-main's appUpdateDownloaded). */
  onDownloaded: () => void;
}

export interface AppUpdaterHandle {
  readonly mode: AppUpdateMode;
  /** Check now. `manual` (menu or Settings) shows the fallback notice again. */
  check: (manual?: boolean) => void;
  /**
   * Before quitting for an update. macOS swaps the bundle and Linux installs
   * the .deb (password prompt), each scheduling the relaunch; Windows checks
   * the installer is there. Returns false, after showing the fallback notice,
   * when it could not; the app keeps running then.
   */
  install: () => Promise<boolean>;
  /** Quit into the update (call after install() and the quit cleanup). */
  finish: () => void;
}

const TAG = '[appUpdater]';

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

function showFallback(log: Logger, mode: AppUpdateMode, reason: UpdateFallbackReason, version?: string | null): void {
  const notice = fallbackNotice(reason, version, mode);
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
 * Start the app updater. Returns null in dev builds and on Linux installs
 * that are not a .deb (those cannot self-update).
 */
export function startAppUpdater(deps: AppUpdaterDeps): AppUpdaterHandle | null {
  const log = deps.logger;
  const mode = chooseUpdateMode({
    platform: process.platform,
    isPackaged: app.isPackaged,
    packageType: process.platform === 'linux' && app.isPackaged ? readPackageType() : null,
  });
  if (mode === 'off') return null;
  if (mode === 'unsupported') {
    log.warn(`${TAG} not a .deb install (no resources/package-type=deb): app updates are off; download new versions from ${releasePageUrl()}`);
    return null;
  }

  const macBundle = mode === 'mac' ? macBundleFromExecPath(process.execPath) : null;
  if (mode === 'mac' && !macBundle) {
    log.warn(`${TAG} cannot find the .app bundle from ${process.execPath}: app updates are off`);
    return null;
  }

  // Only the .deb install needs root; macOS and Windows update as the user.
  const elevation = needsElevation(mode)
    ? detectElevation({ isRoot: process.getuid?.() === 0, hasCommand })
    : ({ kind: 'root' } as const);
  const autoDownload = shouldAutoDownload(elevation);
  const override = process.env[UPDATE_FEED_OVERRIDE_ENV]
    ?? (mode === 'deb' ? process.env[LEGACY_LINUX_FEED_OVERRIDE_ENV] : undefined);
  const { feed, ignoredOverride } = resolveUpdateFeed(override);
  if (ignoredOverride) log.warn(`${TAG} ignoring feed override ${ignoredOverride}: not an http(s) URL`);
  log.info(`${TAG} ${mode} updater: feed ${JSON.stringify(feed)}, elevation ${JSON.stringify(elevation)}, autoDownload ${autoDownload}`);

  let installing = false;
  let installErrorSeen = false;
  let noticeShownFor: string | null = null;
  let downloadedVersion: string | null = null;
  let stagedMacApp: string | null = null;

  const ready: Promise<ElectronAppUpdater | null> = (async () => {
    // Loaded lazily so a broken updater module never blocks startup.
    const imported = await import('electron-updater');
    const mod = (imported.DebUpdater ? imported : (imported as unknown as { default: typeof imported }).default);
    const updater: ElectronAppUpdater = mode === 'mac'
      ? new mod.MacUpdater()
      : mode === 'windows'
        ? new mod.NsisUpdater()
        : new mod.DebUpdater();

    if (mode === 'mac') {
      // MacUpdater hands the downloaded zip to Squirrel.Mac in a private
      // updateDownloaded(); Squirrel refuses ad-hoc signed updates, so report
      // the download straight away and let mac-app-swap.ts install it.
      const internal = updater as unknown as {
        updateDownloaded?: (zip: unknown, event: UpdateDownloadedEvent) => Promise<string[]>;
        dispatchUpdateDownloaded: (event: UpdateDownloadedEvent) => void;
      };
      if (typeof internal.updateDownloaded !== 'function') {
        throw new Error('electron-updater MacUpdater.updateDownloaded is gone (version changed?)');
      }
      internal.updateDownloaded = async (_zip, event) => {
        internal.dispatchUpdateDownloaded(event);
        return [];
      };
    }

    updater.logger = log;
    updater.setFeedURL(feed);
    updater.autoDownload = autoDownload;
    // macOS and Linux install only when the user clicks Restart (a swap or a
    // password prompt during quit could not report a failure). Windows runs
    // the installer silently on quit too.
    updater.autoInstallOnAppQuit = installsOnQuit(mode);
    updater.autoRunAppAfterInstall = true;

    updater.on('checking-for-update', () => deps.broadcast({ state: 'checking' }));
    updater.on('update-available', (info: UpdateInfo) => {
      deps.broadcast({ state: 'available' });
      if (!autoDownload && noticeShownFor !== info.version) {
        noticeShownFor = info.version;
        showFallback(log, mode, 'no-elevation', info.version);
      }
    });
    updater.on('update-not-available', () => deps.broadcast({ state: 'up-to-date' }));
    updater.on('update-downloaded', (event: UpdateDownloadedEvent) => {
      void (async () => {
        if (mode === 'mac') {
          try {
            stagedMacApp = await stageMacUpdate({
              zipPath: event.downloadedFile,
              stageRoot: join(dirname(event.downloadedFile), 'staged'),
              expectedVersion: event.version,
              sha512: event.sha512,
              currentBundle: macBundle!,
              log,
            });
          } catch (error) {
            stagedMacApp = null;
            const message = (error as Error)?.message ?? String(error);
            log.error(`${TAG} could not stage ${event.version}: ${message}`);
            deps.broadcast({ state: 'error', message: `Update ${event.version} could not be prepared: ${message}` });
            return;
          }
        }
        log.info(`${TAG} downloaded ${event.version}; waiting for Restart`);
        downloadedVersion = event.version;
        deps.onDownloaded();
        deps.broadcast({
          state: 'downloaded',
          version: event.version,
          releaseNotes: typeof event.releaseNotes === 'string' ? event.releaseNotes : undefined,
          updateURL: releasePageUrl(event.version),
        });
      })();
    });
    updater.on('error', (error: Error) => {
      log.error(TAG, error);
      deps.broadcast({ state: 'error', message: error?.message ?? String(error) });
      if (installing && !installErrorSeen) {
        installErrorSeen = true;
        showFallback(log, mode, 'install-failed', downloadedVersion);
      }
    });
    return updater;
  })().catch((error) => {
    log.error(`${TAG} could not start electron-updater`, error);
    deps.broadcast({ state: 'error', message: `App updater failed to start: ${error?.message ?? String(error)}` });
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
    if (macBundle) void cleanupOldMacBundles(macBundle, log);
    check();
    setInterval(() => check(), UPDATE_INTERVAL_MS).unref?.();
  });

  const installMac = async (): Promise<boolean> => {
    if (!stagedMacApp) {
      showFallback(log, mode, 'install-failed', downloadedVersion);
      return false;
    }
    const result = await swapMacBundle({ currentBundle: macBundle!, stagedBundle: stagedMacApp, log });
    if (!result.ok) {
      log.warn(`${TAG} swap failed: ${result.error}; the app keeps running`);
      showFallback(log, mode, 'install-failed', downloadedVersion);
      return false;
    }
    stagedMacApp = null;
    // execPath now points into the new bundle.
    app.relaunch();
    log.info(`${TAG} installed ${downloadedVersion ?? 'update'}; quitting to relaunch`);
    return true;
  };

  const installDeb = async (updater: BaseUpdater): Promise<boolean> => {
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
      if (!installErrorSeen) showFallback(log, mode, 'install-failed', downloadedVersion);
      log.warn(`${TAG} install failed; the app keeps running`);
      return false;
    }
    log.info(`${TAG} installed ${downloadedVersion ?? 'update'}; quitting to relaunch`);
    return true;
  };

  return {
    mode,
    check,
    install: async () => {
      const updater = await ready;
      if (!updater) {
        showFallback(log, mode, 'install-failed', downloadedVersion);
        return false;
      }
      if (mode === 'mac') return installMac();
      if (mode === 'windows') {
        const installer = (updater as unknown as { installerPath: string | null }).installerPath;
        if (!installer || !existsSync(installer)) {
          log.warn(`${TAG} downloaded installer missing (${installer ?? 'none'})`);
          showFallback(log, mode, 'install-failed', downloadedVersion);
          return false;
        }
        return true;
      }
      return installDeb(updater as BaseUpdater);
    },
    finish: () => {
      if (mode === 'windows') {
        void ready.then((u) => {
          // Silent NSIS install, then start the new version.
          if (u) u.quitAndInstall(true, true);
          else app.quit();
        });
        return;
      }
      // macOS: bundle swapped, Linux: .deb installed; both scheduled the relaunch.
      app.quit();
    },
  };
}
