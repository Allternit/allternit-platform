/**
 * macOS app update without Squirrel.Mac.
 *
 * Release builds are ad-hoc signed. Squirrel.Mac (what electron-updater's
 * MacUpdater.quitAndInstall uses) only installs a bundle that meets the
 * running app's designated requirement, and an ad-hoc requirement is the
 * exact cdhash, so it refuses every new build. app-updater.ts therefore lets
 * MacUpdater download and sha512-check the zip from latest-mac.yml, and this
 * module does the install:
 *
 * 1. stageMacUpdate (right after the download, in the background): `ditto -x -k`
 *    the zip into the updater cache, `codesign --verify --deep --strict` the new
 *    bundle, and check its bundle id, version and (for a Developer ID build)
 *    team against the running app.
 * 2. swapMacBundle (when the user clicks Restart, before any cleanup): rename
 *    the running bundle to a hidden sibling, move the staged bundle into its
 *    place, and put the old one back if that fails. A failure (EPERM from App
 *    Management, a read-only volume, a translocated app) leaves the app
 *    running, and the caller shows the manual-download notice.
 * 3. cleanupOldMacBundles (next launch): delete the hidden old bundle.
 */
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import { promisify } from 'node:util';
import {
  isOldMacBundleName,
  macOldBundlePath,
  macStagedBundleProblem,
  macSwapBlocker,
  parseCodesignTeamId,
  type MacBundleFacts,
} from './app-updater-logic.js';

const run = promisify(execFile);

type Logger = {
  info: (...args: unknown[]) => void;
  warn: (...args: unknown[]) => void;
  error: (...args: unknown[]) => void;
};

const TAG = '[appUpdater:mac]';
const MARKER = '.allternit-staged.json';

async function plistValue(bundle: string, key: string): Promise<string | null> {
  try {
    const { stdout } = await run('/usr/libexec/PlistBuddy', ['-c', `Print :${key}`, join(bundle, 'Contents', 'Info.plist')], {
      timeout: 15_000,
    });
    return stdout.trim() || null;
  } catch {
    return null;
  }
}

async function teamId(bundle: string): Promise<string | null> {
  try {
    // codesign -d writes its report to stderr.
    const { stdout, stderr } = await run('/usr/bin/codesign', ['-dv', '--verbose=2', bundle], { timeout: 60_000 });
    return parseCodesignTeamId(`${stdout}\n${stderr}`);
  } catch {
    return null;
  }
}

async function bundleFacts(bundle: string): Promise<MacBundleFacts> {
  const [bundleId, version, team] = await Promise.all([
    plistValue(bundle, 'CFBundleIdentifier'),
    plistValue(bundle, 'CFBundleShortVersionString'),
    teamId(bundle),
  ]);
  return { bundleId, version, teamId: team };
}

/**
 * Extract and check the downloaded zip. Resolves to the staged .app path;
 * throws with the reason when the bundle must not be installed.
 */
export async function stageMacUpdate(opts: {
  zipPath: string;
  /** Directory to extract into (emptied first). */
  stageRoot: string;
  expectedVersion: string;
  /** sha512 from latest-mac.yml: a staged copy of the same zip is reused. */
  sha512?: string;
  currentBundle: string;
  log: Logger;
}): Promise<string> {
  const { zipPath, stageRoot, expectedVersion, currentBundle, log } = opts;
  const stamp = JSON.stringify({ version: expectedVersion, sha512: opts.sha512 ?? null, zip: basename(zipPath) });
  const stagedApp = join(stageRoot, basename(currentBundle));

  // Same zip already staged on an earlier launch: skip the minute-long
  // extract and deep verify.
  try {
    if (existsSync(stagedApp) && (await readFile(join(stageRoot, MARKER), 'utf8')) === stamp) {
      log.info(`${TAG} reusing staged ${expectedVersion} at ${stagedApp}`);
      return stagedApp;
    }
  } catch {
    // not staged yet
  }

  await rm(stageRoot, { recursive: true, force: true });
  await mkdir(stageRoot, { recursive: true });
  log.info(`${TAG} extracting ${zipPath}`);
  await run('/usr/bin/ditto', ['-x', '-k', zipPath, stageRoot], { timeout: 15 * 60_000 });

  const apps = (await readdir(stageRoot)).filter((n) => n.endsWith('.app'));
  if (apps.length !== 1) throw new Error(`update zip holds ${apps.length} .app bundles, expected 1`);
  const extracted = join(stageRoot, apps[0]);
  if (extracted !== stagedApp) await rename(extracted, stagedApp);

  try {
    await run('/usr/bin/codesign', ['--verify', '--deep', '--strict', stagedApp], { timeout: 15 * 60_000 });
  } catch (error) {
    const detail = (error as { stderr?: string }).stderr?.trim() || (error as Error).message;
    throw new Error(`new app fails codesign --verify: ${detail}`);
  }

  const [current, staged] = await Promise.all([bundleFacts(currentBundle), bundleFacts(stagedApp)]);
  const problem = macStagedBundleProblem({ current, staged, expectedVersion });
  if (problem) throw new Error(`new app refused: ${problem}`);

  await writeFile(join(stageRoot, MARKER), stamp);
  log.info(`${TAG} staged ${expectedVersion} (${staged.bundleId}, team ${staged.teamId ?? 'ad-hoc'}) at ${stagedApp}`);
  return stagedApp;
}

export type MacSwapResult = { ok: true; oldBundle: string } | { ok: false; error: string };

async function moveBundle(from: string, to: string): Promise<void> {
  try {
    await rename(from, to);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EXDEV') throw error;
    // Different volume (app on an external disk): copy, then drop the staged copy.
    await run('/usr/bin/ditto', [from, to], { timeout: 15 * 60_000 });
    await rm(from, { recursive: true, force: true });
  }
}

/**
 * Put the staged bundle where the running one is. On any failure the running
 * bundle is back in place and the result says why.
 */
export async function swapMacBundle(opts: { currentBundle: string; stagedBundle: string; log: Logger }): Promise<MacSwapResult> {
  const { currentBundle, stagedBundle, log } = opts;
  const blocker = macSwapBlocker(currentBundle);
  if (blocker) return { ok: false, error: blocker };
  if (!existsSync(stagedBundle)) return { ok: false, error: `staged bundle missing: ${stagedBundle}` };

  const oldBundle = macOldBundlePath(currentBundle, `${process.pid}-${Date.now()}`);
  try {
    await rename(currentBundle, oldBundle);
  } catch (error) {
    return { ok: false, error: `cannot move ${currentBundle} aside: ${(error as Error).message}` };
  }
  try {
    await moveBundle(stagedBundle, currentBundle);
  } catch (error) {
    const reason = (error as Error).message;
    try {
      await rm(currentBundle, { recursive: true, force: true });
      await rename(oldBundle, currentBundle);
    } catch (restoreError) {
      log.error(`${TAG} could not put the old app back from ${oldBundle}`, restoreError);
    }
    return { ok: false, error: `cannot move the new app into place: ${reason}` };
  }
  log.info(`${TAG} swapped in the new app; old bundle at ${oldBundle}`);
  return { ok: true, oldBundle };
}

/** Delete hidden old bundles left by an earlier swap (runs once per launch). */
export async function cleanupOldMacBundles(currentBundle: string, log: Logger): Promise<void> {
  const dir = dirname(currentBundle);
  const base = basename(currentBundle);
  let entries: string[];
  try {
    entries = await readdir(dir);
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!isOldMacBundleName(entry, base)) continue;
    try {
      await rm(join(dir, entry), { recursive: true, force: true });
      log.info(`${TAG} removed old bundle ${entry}`);
    } catch (error) {
      log.warn(`${TAG} could not remove old bundle ${entry}`, error);
    }
  }
}
