import { createRequire } from 'node:module';
import { describe, expect, it } from 'vitest';

const { planPrune } = createRequire(import.meta.url)('../../scripts/runtime-package.cjs');

const man = (build: number, ...shas: string[]) => ({ build, files: Object.fromEntries(shas.map((s, i) => [`f${i}`, { sha256: s, size: 1 }])) });
const objs = (...shas: string[]) => shas.map((s) => `objects/${s}.gz`);

describe('planPrune', () => {
  const platforms = {
    'darwin-arm64': { latest: 'v3.json', manifests: { 'v1.json': man(1, 'shared', 'oldmac'), 'v2.json': man(2, 'a'), 'v3.json': man(3, 'b') } },
    'linux-x64': { latest: 'v2.json', manifests: { 'v1.json': man(1, 'lin1'), 'v2.json': man(2, 'shared') } },
  };
  const objects = objs('shared', 'oldmac', 'a', 'b', 'lin1');

  it('keeps an object used by an old darwin manifest when a kept linux manifest also uses it', () => {
    const p = planPrune({ platforms, objects, keep: 1 });
    expect(p.dropManifests).toEqual(['stable/darwin-arm64/manifests/v1.json', 'stable/darwin-arm64/manifests/v2.json', 'stable/linux-x64/manifests/v1.json']);
    expect(p.dropObjects.sort()).toEqual(objs('a', 'lin1', 'oldmac'));
    expect(p.keepObjects.has('objects/shared.gz')).toBe(true);
    expect(p.missing).toEqual([]);
  });

  it('always keeps the manifest latest.json names, even when old', () => {
    const p = planPrune({ platforms: { x: { latest: 'v1.json', manifests: { 'v1.json': man(1, 'a'), 'v2.json': man(2, 'b') } } }, objects: objs('a', 'b'), keep: 1 });
    expect(p.keepManifests).toEqual(['stable/x/manifests/v1.json', 'stable/x/manifests/v2.json']);
    expect(p.dropObjects).toEqual([]);
  });

  it('reports a kept manifest that references a missing object', () => {
    const p = planPrune({ platforms, objects: objs('shared', 'a', 'oldmac', 'lin1'), keep: 3 });
    expect(p.missing).toEqual(['objects/b.gz']);
  });

  it('orders by the version timestamp when a manifest has no build', () => {
    const m = { 'r20261101.0001.json': { version: 'r20261101.0001', files: {} }, 'r20261102.0001.json': { version: 'r20261102.0001', files: {} } };
    const p = planPrune({ platforms: { x: { latest: '', manifests: m } }, objects: [], keep: 1 });
    expect(p.keepManifests).toEqual(['stable/x/manifests/r20261102.0001.json']);
  });
});
