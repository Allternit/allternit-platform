import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readlinkSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({
  app: { isPackaged: false },
}));

vi.mock('./runtime-home.js', () => ({
  runtimeResource: (...segments: string[]) => join('/nonexistent-runtime', ...segments),
}));

vi.mock('electron-log', () => ({
  default: { info: () => {}, warn: () => {}, error: () => {} },
}));

import {
  FactoryEngineManager,
  bundledGizziBinary,
  factoryPeerPort,
  factoryServeArgs,
  factoryWorkspaceRoot,
  installGizziLink,
  removeStaleTools,
  resolveFactoryBinary,
} from './factory-engine-manager.js';
import { PORTS } from './config.js';

function tmp(prefix: string): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

function file(p: string, body: string | Buffer = '#!/bin/sh\n'): string {
  mkdirSync(join(p, '..'), { recursive: true });
  writeFileSync(p, body);
  chmodSync(p, 0o755);
  return p;
}

describe('resolveFactoryBinary', () => {
  it('uses only resources/bin when packaged', () => {
    const resources = tmp('allternit-res-');
    expect(resolveFactoryBinary({ packaged: true, resourcesPath: resources, repoRoot: '/nope', platform: 'darwin' })).toBeNull();
    const bin = file(join(resources, 'bin', 'allternit-factory'));
    expect(resolveFactoryBinary({ packaged: true, resourcesPath: resources, repoRoot: '/nope', platform: 'darwin' })).toBe(bin);
  });

  it('prefers the active runtime package copy when packaged', () => {
    const resources = tmp('allternit-res-');
    file(join(resources, 'bin', 'allternit-factory'));
    const runtimeBin = file(join(tmp('allternit-runtime-'), 'bin', 'allternit-factory'));
    expect(
      resolveFactoryBinary({ packaged: true, resourcesPath: resources, runtimeBinary: runtimeBin, repoRoot: '/nope', platform: 'darwin' }),
    ).toBe(runtimeBin);
  });

  it('never falls back to a binary on PATH', () => {
    const repo = tmp('allternit-repo-');
    const onPath = file(join(tmp('allternit-path-'), 'allternit-factory'));
    expect(
      resolveFactoryBinary({ packaged: false, repoRoot: repo, platform: 'darwin', env: { PATH: join(onPath, '..') } }),
    ).toBeNull();
  });

  it('prefers $ALLTERNIT_FACTORY_BIN, then the cargo targets, in dev', () => {
    const repo = tmp('allternit-repo-');
    const release = file(join(repo, 'target', 'release', 'allternit-factory'));
    expect(resolveFactoryBinary({ packaged: false, repoRoot: repo, platform: 'darwin', env: {} })).toBe(release);
    const explicit = file(join(tmp('allternit-bin-'), 'allternit-factory'));
    expect(
      resolveFactoryBinary({ packaged: false, repoRoot: repo, platform: 'darwin', env: { ALLTERNIT_FACTORY_BIN: explicit } }),
    ).toBe(explicit);
  });
});

describe('engine process', () => {
  it('serves its own workspace on the Desktop port, not 3011', () => {
    expect(PORTS.FACTORY).not.toBe(PORTS.EXTENSION_BRIDGE);
    expect(factoryServeArgs('/w', PORTS.FACTORY)).toEqual(['--root', '/w', 'serve', '--port', String(PORTS.FACTORY)]);
    expect(factoryServeArgs('/w', PORTS.FACTORY, 3019)).toEqual(['--root', '/w', 'serve', '--port', String(PORTS.FACTORY), '--peer-port', '3019']);
    // Only a paired computer takes peer calls.
    expect(factoryPeerPort('/Users/u', (p) => p === '/Users/u/.allternit/computer/paired.json')).toBe(3019);
    expect(factoryPeerPort('/Users/u', () => false)).toBeNull();
    expect(factoryWorkspaceRoot({}, '/Users/u')).toBe('/Users/u/.allternit/factory/workspace');
    expect(factoryWorkspaceRoot({ ALLTERNIT_FACTORY_ROOT: '/srv/w' }, '/Users/u')).toBe('/srv/w');
  });

  it('adopts an engine already answering health', async () => {
    const fetchImpl = (async (url: string) => {
      expect(url).toBe(`http://127.0.0.1:${PORTS.FACTORY}/api/factory/health`);
      return new Response(JSON.stringify({ ok: true, service: 'allternit-factory', panes: true }), { status: 200 });
    }) as unknown as typeof fetch;
    const m = new FactoryEngineManager({ fetchImpl });
    expect(await m.start()).toBe(`http://127.0.0.1:${PORTS.FACTORY}`);
    expect(m.getMode()).toBe('adopted');
  });

  it('does not adopt another service on the port, and says why it has no engine', async () => {
    const fetchImpl = (async () =>
      new Response(JSON.stringify({ ok: true, service: 'something-else' }), { status: 200 })) as unknown as typeof fetch;
    const m = new FactoryEngineManager({
      fetchImpl,
      binaryContext: { packaged: true, resourcesPath: tmp('allternit-res-'), repoRoot: '/nope', platform: 'darwin' },
    });
    expect(await m.start()).toBeNull();
    expect(m.getMode()).toBeNull();
    expect((await m.getStatus()).error).toMatch(/not bundled/);
  });
});

describe('gizzi on PATH', () => {
  it('links gizzi into ~/.local/bin on first run', () => {
    const home = tmp('allternit-home-');
    const gizzi = file(join(tmp('Allternit Desktop.app-'), 'Allternit Desktop.app', 'Contents', 'Resources', 'bin', 'gizzi-code'));
    const r = installGizziLink(gizzi, home);
    expect(r.action).toBe('linked');
    expect(readlinkSync(join(home, '.local', 'bin', 'gizzi'))).toBe(gizzi);
    expect(installGizziLink(gizzi, home).action).toBe('kept');
  });

  it('refreshes a link into an older Desktop bundle, keeps anything else', () => {
    const home = tmp('allternit-home-');
    const bin = join(home, '.local', 'bin');
    mkdirSync(bin, { recursive: true });
    symlinkSync('/Volumes/Old/Allternit Desktop.app/Contents/Resources/bin/gizzi-code', join(bin, 'gizzi'));
    const gizzi = '/Applications/Allternit Desktop.app/Contents/Resources/bin/gizzi-code';
    expect(installGizziLink(gizzi, home).action).toBe('relinked');
    expect(readlinkSync(join(bin, 'gizzi'))).toBe(gizzi);

    const brewHome = tmp('allternit-home-');
    const brewBin = join(brewHome, '.local', 'bin');
    mkdirSync(brewBin, { recursive: true });
    symlinkSync('/opt/homebrew/bin/gizzi', join(brewBin, 'gizzi'));
    expect(installGizziLink(gizzi, brewHome).action).toBe('kept');
    expect(readlinkSync(join(brewBin, 'gizzi'))).toBe('/opt/homebrew/bin/gizzi');
  });

  it('links only gizzi, never the engine', () => {
    const home = tmp('allternit-home-');
    installGizziLink('/Applications/Allternit Desktop.app/Contents/Resources/bin/gizzi-code', home);
    expect(existsSync(join(home, '.local', 'bin', 'allternit-factory'))).toBe(false);
  });

  it('finds the bundled gizzi only in a packaged resources dir', () => {
    const resources = tmp('allternit-res-');
    expect(bundledGizziBinary(resources, 'darwin')).toBeNull();
    const g = file(join(resources, 'bin', 'gizzi-code'));
    expect(bundledGizziBinary(resources, 'darwin')).toBe(g);
    expect(bundledGizziBinary(resources, 'win32')).toBeNull();
    expect(bundledGizziBinary(undefined, 'darwin')).toBeNull();
  });
});

describe('removeStaleTools', () => {
  it('removes only the retired tools, after checking each target', () => {
    const home = tmp('allternit-home-');
    const bin = join(home, '.local', 'bin');
    mkdirSync(bin, { recursive: true });
    // Old tools.
    file(join(bin, 'allternit-rails'), Buffer.concat([Buffer.from([0xcf, 0xfa, 0xed, 0xfe]), Buffer.from('…allternit_commrails::cli…')])); // old-names: keep (installer removes the old binaries)
    symlinkSync('/Users/u/.claude/skills/agent-orchestrator/scripts/ao-send', join(bin, 'ao-send')); // old-names: keep (installer removes the old binaries)
    symlinkSync('/Users/u/allternit/tools/agent-orchestrator/scripts/ao-consult', join(bin, 'ao-consult.repo-link')); // old-names: keep (installer removes the old binaries)
    file(join(bin, 'ao-consult'), '#!/usr/bin/env bash\nexport AO_CONSULT_ACTIVE=1\nexec "$HOME/.local/bin/ao-consult.repo-link" "$@"\n'); // old-names: keep (installer removes the old binaries)
    // Not old tools: same-looking names the user owns.
    symlinkSync('/opt/homebrew/bin/ao-thing', join(bin, 'ao-thing'));
    file(join(bin, 'ao-notes'), '#!/bin/sh\necho mine\n');
    file(join(bin, 'gizzi-coder'), '#!/bin/zsh\n');

    const r = removeStaleTools(home);
    expect(r.removed.map((p) => p.slice(bin.length + 1)).sort()).toEqual(
      ['allternit-rails', 'ao-consult', 'ao-consult.repo-link', 'ao-send'], // old-names: keep (installer removes the old binaries)
    );
    expect(r.kept.map((k) => k.path.slice(bin.length + 1)).sort()).toEqual(['ao-notes', 'ao-thing']);
    expect(existsSync(join(bin, 'gizzi-coder'))).toBe(true);
    expect(lstatSync(join(bin, 'ao-thing')).isSymbolicLink()).toBe(true);
  });

  it('keeps an old-engine-named file that is not the old engine binary', () => {
    const home = tmp('allternit-home-');
    file(join(home, '.local', 'bin', 'allternit-rails'), '#!/bin/sh\nexec gizzi agents "$@"\n'); // old-names: keep (installer removes the old binaries)
    const r = removeStaleTools(home);
    expect(r.removed).toEqual([]);
    expect(r.kept[0].reason).toMatch(/not the old engine binary/);
  });

  it('does nothing when ~/.local/bin does not exist', () => {
    expect(removeStaleTools(tmp('allternit-home-'))).toEqual({ removed: [], kept: [] });
  });
});
