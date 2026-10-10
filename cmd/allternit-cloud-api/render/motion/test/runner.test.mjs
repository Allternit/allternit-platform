// node --test test/   (needs `npm ci` here; the render test also needs ffmpeg)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const entry = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'dist', 'render.mjs');
const run = (...a) => spawnSync(process.execPath, [entry, ...a], { encoding: 'utf8' });
const hasFfmpeg = spawnSync('ffmpeg', ['-version']).status === 0;

const comp = {
  version: 1, width: 320, height: 180, fps: 12,
  theme: { bg: '#0b0b0f', fg: '#f5f5f7', accent: '#6d7cff', muted: '#8e8ea0', font: 'sans' },
  scenes: [{ id: 'a', type: 'title', title: 'Hi', duration: 1, transition: { type: 'cut', duration: 0 }, props: { text: 'Hello', subtitle: '', align: 'left' } }],
};

test('refuses missing arguments', () => {
  const r = run();
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Missing --in/);
});

test('refuses a composition that is not valid', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'motion-'));
  writeFileSync(path.join(dir, 'c.json'), '{"nope":true}');
  const r = run('--in', path.join(dir, 'c.json'), '--out', path.join(dir, 'o.mp4'));
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Invalid composition/);
});

test('renders an MP4 and reports progress', { skip: !hasFfmpeg }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'motion-'));
  writeFileSync(path.join(dir, 'c.json'), JSON.stringify(comp));
  const out = path.join(dir, 'o.mp4');
  const r = run('--in', path.join(dir, 'c.json'), '--out', out);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /progress 1\.0000/);
  assert.ok(existsSync(out) && statSync(out).size > 0);
});
