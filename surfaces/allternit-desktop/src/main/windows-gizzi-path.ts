/**
 * Windows: put the bundled gizzi CLI on the user's PATH.
 *
 * macOS/Linux link ~/.local/bin/gizzi to the bundled binary
 * (factory-engine-manager.ts installGizziLink). Windows has no ~/.local/bin on
 * PATH and symlinks need Developer Mode, so a packaged Desktop instead:
 *
 *   1. writes %LOCALAPPDATA%\Allternit\bin\gizzi.cmd, a two-line shim that
 *      runs resources\bin\gizzi-code.exe with the same arguments (the same
 *      shape npm uses for global CLIs), and
 *   2. appends that folder to the per-user PATH (HKCU\Environment\Path) when
 *      it isn't there yet.
 *
 * PATH is read and written with `reg`, keeping the value's type
 * (REG_EXPAND_SZ stays expandable) and never truncating it (setx cuts values
 * at 1024 characters). The new entry goes last, so a gizzi the user already
 * installed (npm, scoop) keeps winning. After a PATH change, setting the
 * per-user ALLTERNIT_BIN variable through .NET broadcasts WM_SETTINGCHANGE,
 * so terminals opened afterwards see the new PATH without signing out.
 *
 * A gizzi.cmd that isn't ours (no marker line) is kept. The NSIS uninstaller
 * removes the shim (build/installer.nsh customUnInstall).
 */

import { execFile } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

export const SHIM_MARKER = 'REM allternit-desktop gizzi shim';

export type RunCommand = (file: string, args: string[]) => Promise<string>;

export interface WindowsGizziPathDeps {
  env?: NodeJS.ProcessEnv;
  exists?: (p: string) => boolean;
  readFile?: (p: string) => string | null;
  writeFile?: (p: string, data: string) => void;
  mkdirp?: (p: string) => void;
  run?: RunCommand;
}

export type WindowsGizziPathResult =
  | { action: 'skipped'; reason: string }
  | { action: 'kept'; shim: string; reason: string }
  | { action: 'installed'; shim: string; target: string; pathUpdated: boolean };

/** Per-user folder that holds the gizzi.cmd shim. */
export function windowsBinDir(env: NodeJS.ProcessEnv): string | null {
  const local = env.LOCALAPPDATA?.trim() || (env.USERPROFILE ? path.win32.join(env.USERPROFILE, 'AppData', 'Local') : '');
  return local ? path.win32.join(local, 'Allternit', 'bin') : null;
}

export function gizziShimContents(gizziExe: string): string {
  return ['@echo off', SHIM_MARKER, `"${gizziExe}" %*`, ''].join('\r\n');
}

/** Parse `reg query HKCU\Environment /v Path` output. Null when the value doesn't exist. */
export function parseRegPath(output: string): { type: 'REG_SZ' | 'REG_EXPAND_SZ'; value: string } | null {
  for (const line of output.split(/\r?\n/)) {
    const m = /^\s*Path\s+(REG_EXPAND_SZ|REG_SZ)\s*(.*)$/i.exec(line);
    if (m) return { type: m[1].toUpperCase() as 'REG_SZ' | 'REG_EXPAND_SZ', value: m[2].trim() };
  }
  return null;
}

function normalizeEntry(entry: string, env: NodeJS.ProcessEnv): string {
  const expanded = entry.replace(/%([^%]+)%/g, (whole, name: string) => {
    const hit = Object.keys(env).find((k) => k.toLowerCase() === name.toLowerCase());
    return hit ? (env[hit] ?? whole) : whole;
  });
  return expanded.trim().replace(/^"|"$/g, '').replace(/[\\/]+$/, '').toLowerCase();
}

/** The new PATH value with `dir` appended, or null when it's already there. */
export function appendPathEntry(current: string, dir: string, env: NodeJS.ProcessEnv = {}): string | null {
  const want = normalizeEntry(dir, env);
  const entries = current.split(';').filter((e) => e.trim() !== '');
  if (entries.some((e) => normalizeEntry(e, env) === want)) return null;
  return [...entries, dir.replace(/[\\/]+$/, '')].join(';');
}

const defaultRun: RunCommand = (file, args) =>
  new Promise((resolve, reject) => {
    execFile(file, args, { windowsHide: true, encoding: 'utf8' }, (err, stdout) => {
      if (err) reject(err);
      else resolve(String(stdout));
    });
  });

export async function installWindowsGizziOnPath(
  resourcesPath: string | undefined,
  deps: WindowsGizziPathDeps = {},
): Promise<WindowsGizziPathResult> {
  const env = deps.env ?? process.env;
  const exists = deps.exists ?? fs.existsSync;
  const readFile = deps.readFile ?? ((p: string) => {
    try {
      return fs.readFileSync(p, 'utf8');
    } catch {
      return null;
    }
  });
  const writeFile = deps.writeFile ?? ((p: string, d: string) => fs.writeFileSync(p, d, 'utf8'));
  const mkdirp = deps.mkdirp ?? ((p: string) => fs.mkdirSync(p, { recursive: true }));
  const run = deps.run ?? defaultRun;

  if (!resourcesPath) return { action: 'skipped', reason: 'no resources path' };
  const target = path.win32.join(resourcesPath, 'bin', 'gizzi-code.exe');
  if (!exists(target)) return { action: 'skipped', reason: `no bundled gizzi at ${target}` };
  const dir = windowsBinDir(env);
  if (!dir) return { action: 'skipped', reason: 'LOCALAPPDATA and USERPROFILE are unset' };

  const shim = path.win32.join(dir, 'gizzi.cmd');
  const existing = readFile(shim);
  if (existing !== null && !existing.includes(SHIM_MARKER)) {
    return { action: 'kept', shim, reason: 'a gizzi.cmd that Allternit Desktop did not write is already there' };
  }
  const contents = gizziShimContents(target);
  if (existing !== contents) {
    mkdirp(dir);
    writeFile(shim, contents);
  }

  let current: { type: 'REG_SZ' | 'REG_EXPAND_SZ'; value: string } | null = null;
  try {
    current = parseRegPath(await run('reg', ['query', 'HKCU\\Environment', '/v', 'Path']));
  } catch {
    current = null; // No per-user Path value yet: reg query exits 1.
  }
  const next = appendPathEntry(current?.value ?? '', dir, env);
  if (next === null) return { action: 'installed', shim, target, pathUpdated: false };

  await run('reg', ['add', 'HKCU\\Environment', '/v', 'Path', '/t', current?.type ?? 'REG_EXPAND_SZ', '/d', next, '/f']);
  try {
    // Broadcasts WM_SETTINGCHANGE so new terminals pick up the PATH change.
    const quoted = dir.replace(/'/g, "''");
    await run('powershell.exe', [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `[Environment]::SetEnvironmentVariable('ALLTERNIT_BIN', '${quoted}', 'User')`,
    ]);
  } catch {
    // PATH is saved; new terminals see it after the next sign-in.
  }
  return { action: 'installed', shim, target, pathUpdated: true };
}
