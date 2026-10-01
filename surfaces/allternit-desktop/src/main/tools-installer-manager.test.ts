import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { applyToolsEnvironment, installerArgs, toolsScriptPath } from './tools-installer-manager.js';

describe('tools-installer-manager', () => {
  it('resolves the packaged and dev installer paths', () => {
    expect(toolsScriptPath({ isPackaged: true, resourcesPath: '/R', appPath: '/A', execPath: '/E' })).toBe('/R/allternit-tools/allternit-tools.mjs');
    expect(toolsScriptPath({ isPackaged: false, resourcesPath: '/R', appPath: '/repo/surfaces/allternit-desktop', execPath: '/E' })).toBe('/repo/tools/allternit-tools/allternit-tools.mjs');
  });

  it('puts the tools bin first on PATH once and points the API at the installer', () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tools-env-'));
    const script = path.join(root, 'allternit-tools', 'allternit-tools.mjs');
    fs.mkdirSync(path.dirname(script), { recursive: true });
    fs.writeFileSync(script, '');
    const env: NodeJS.ProcessEnv = { PATH: '/usr/bin:/bin' };
    const input = { isPackaged: true, resourcesPath: root, appPath: '/A', execPath: '/E/Allternit', home: '/h' };
    applyToolsEnvironment(input, env);
    applyToolsEnvironment(input, env);
    expect(env.PATH).toBe('/h/.allternit/tools/bin:/usr/bin:/bin');
    expect(env.ALLTERNIT_TOOLS_SCRIPT).toBe(script);
    expect(env.ALLTERNIT_TOOLS_NODE).toBe('/E/Allternit');
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('leaves the API without an installer when the script is missing', () => {
    const env: NodeJS.ProcessEnv = { PATH: '' };
    applyToolsEnvironment({ isPackaged: true, resourcesPath: '/nonexistent', appPath: '/A', execPath: '/E', home: '/h' }, env);
    expect(env.ALLTERNIT_TOOLS_SCRIPT).toBeUndefined();
  });

  it('installs uv first, then only the selected tools (default selection is empty)', () => {
    expect(installerArgs('prereqs')).toEqual(['install', '--only', 'uv', '--json']);
    expect(installerArgs('selected')).toEqual(['install', '--selected', '--json']);
  });
});
