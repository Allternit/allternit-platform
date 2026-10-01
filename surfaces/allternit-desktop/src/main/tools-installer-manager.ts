/**
 * Tools installer hook (plan D0): Desktop runs `allternit-tools` — the
 * zero-dependency Node installer in tools/allternit-tools — with its own
 * Electron binary as Node (ELECTRON_RUN_AS_NODE=1), so nothing else needs
 * to be on the machine.
 *
 * On launch:
 *  1. `applyToolsEnvironment()` puts ~/.allternit/tools/bin first on PATH and
 *     tells allternit-api where the installer is (ALLTERNIT_TOOLS_SCRIPT /
 *     ALLTERNIT_TOOLS_NODE) so `POST /providers/:id/install` works. Must run
 *     before the backend and gizzi sidecars spawn (they inherit process.env).
 *  2. `runStartupToolsInstall()` makes sure `uv` exists (System One needs it;
 *     Laya itself stays with system-one-manager) and installs whatever the
 *     user selected in onboarding / Settings. Selection lives in
 *     ~/.allternit/tools/selection.json, written by the installer's
 *     `--select` (the API route) and `select` command — default is none.
 */

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import log from 'electron-log';

export interface ToolsEnvInput {
  isPackaged: boolean;
  resourcesPath: string;
  appPath: string;
  execPath: string;
  home?: string;
}

export function toolsPrefix(env: NodeJS.ProcessEnv = process.env, home = os.homedir()): string {
  return env.ALLTERNIT_TOOLS_PREFIX || path.join(home, '.allternit', 'tools');
}

/** Packaged: <resources>/allternit-tools/allternit-tools.mjs; dev: the repo copy. */
export function toolsScriptPath(input: ToolsEnvInput): string {
  return input.isPackaged
    ? path.join(input.resourcesPath, 'allternit-tools', 'allternit-tools.mjs')
    : path.resolve(input.appPath, '..', '..', 'tools', 'allternit-tools', 'allternit-tools.mjs');
}

export function applyToolsEnvironment(input: ToolsEnvInput, env: NodeJS.ProcessEnv = process.env): { script: string; bin: string } {
  const prefix = toolsPrefix(env, input.home);
  const bin = path.join(prefix, 'bin');
  const parts = (env.PATH || '').split(path.delimiter).filter((p) => p && p !== bin);
  env.PATH = [bin, ...parts].join(path.delimiter);
  env.ALLTERNIT_TOOLS_PREFIX = prefix;
  const script = toolsScriptPath(input);
  if (fs.existsSync(script)) {
    env.ALLTERNIT_TOOLS_SCRIPT = script;
    env.ALLTERNIT_TOOLS_NODE = input.execPath;
  } else {
    log.warn(`[ToolsInstaller] installer not found at ${script}; tool installs disabled`);
  }
  return { script, bin };
}

export function installerArgs(kind: 'prereqs' | 'selected'): string[] {
  return kind === 'prereqs'
    ? ['install', '--only', 'uv', '--json']
    : ['install', '--selected', '--json'];
}

function runInstaller(execPath: string, script: string, args: string[], env: NodeJS.ProcessEnv): Promise<number> {
  return new Promise((resolve) => {
    const child = spawn(execPath, [script, ...args], {
      env: { ...env, ELECTRON_RUN_AS_NODE: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let buf = '';
    child.stdout.on('data', (chunk: Buffer) => {
      buf += chunk.toString();
      let nl: number;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        try {
          const ev = JSON.parse(line);
          if (ev.type === 'log' || ev.type === 'step') continue;
          const msg = `[ToolsInstaller] ${ev.type} ${ev.tool ?? ''} ${ev.reason ?? ev.code ?? ev.version ?? ''}`.trim();
          if (ev.type === 'error') log.warn(msg, ev.message ?? '');
          else log.info(msg);
        } catch {
          /* non-JSON noise */
        }
      }
    });
    child.stderr.on('data', (c: Buffer) => log.debug(`[ToolsInstaller] ${c.toString().trim()}`));
    child.on('error', (e) => {
      log.warn('[ToolsInstaller] could not start installer', e);
      resolve(-1);
    });
    child.on('close', (code) => resolve(code ?? -1));
  });
}

let started = false;

/** Fire-and-forget; never blocks or fails app startup. */
export async function runStartupToolsInstall(input: ToolsEnvInput, env: NodeJS.ProcessEnv = process.env): Promise<void> {
  if (started || env.ALLTERNIT_TOOLS_AUTOINSTALL === '0') return;
  started = true;
  const script = env.ALLTERNIT_TOOLS_SCRIPT || toolsScriptPath(input);
  if (!fs.existsSync(script)) return;
  const prereq = await runInstaller(input.execPath, script, installerArgs('prereqs'), env);
  if (prereq !== 0) log.warn(`[ToolsInstaller] uv prerequisite exited ${prereq}`);
  const code = await runInstaller(input.execPath, script, installerArgs('selected'), env);
  log.info(`[ToolsInstaller] selected-tools install finished (exit ${code})`);
}
