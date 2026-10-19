#!/usr/bin/env node
// Reuse a release Rust binary across Desktop builds when nothing it is built
// from has changed (release-desktop.yml). allternit-api alone is ~24 min of
// every Windows build, and most Desktop fixes never touch Rust.
//
//   save  --dir <cache dir> --bin <target/.../x[.exe]> [--bin ...]
//         Reads each binary's cargo dep-info (<bin>.d), widens every in-repo
//         source to the git-tracked files of its crate (build.rs inputs such as
//         factory/pane's vendored Zig live there), and stores the binaries with
//         a manifest: file list + hash of those files, Cargo.lock and rustc -V.
//   check --dir <cache dir>
//         Recomputes that hash. On a match it copies the binaries back to their
//         target/ paths and writes hit=true to $GITHUB_OUTPUT; otherwise
//         hit=false and the build runs as before.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(execFileSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' }).trim());
const [cmd, ...rest] = process.argv.slice(2);
const args = { bin: [] };
for (let i = 0; i < rest.length; i += 2) {
  if (rest[i] === '--bin') args.bin.push(rest[i + 1]);
  else args[rest[i].replace(/^--/, '')] = rest[i + 1];
}
const dir = path.resolve(root, args.dir);
const manifestPath = path.join(dir, 'manifest.json');

function output(key, value) {
  console.log(`${key}=${value}`);
  if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
}

function rel(p) {
  return path.relative(root, path.resolve(root, p)).split(path.sep).join('/');
}

// Dep-info lines are "target: dep dep ..." with spaces escaped as "\ ".
function depInfoFiles(dFile) {
  const files = new Set();
  for (const line of fs.readFileSync(dFile, 'utf8').split(/\r?\n/)) {
    const m = line.match(/^(.*?[^\\]):\s(.*)$/);
    if (!m) continue;
    for (const dep of m[2].split(/(?<!\\)\s+/).filter(Boolean)) files.add(dep.replace(/\\ /g, ' '));
  }
  return [...files];
}

function crateRoot(file) {
  let d = path.dirname(path.resolve(root, file));
  while (d.startsWith(root) && d !== root) {
    if (fs.existsSync(path.join(d, 'Cargo.toml'))) return d;
    d = path.dirname(d);
  }
  return null;
}

function inputsFor(bins) {
  const files = new Set(['Cargo.lock', 'Cargo.toml']);
  const crates = new Set();
  for (const bin of bins) {
    const dFile = bin.replace(/\.exe$/, '') + '.d';
    for (const dep of depInfoFiles(path.resolve(root, dFile))) {
      const abs = path.resolve(root, dep);
      if (!abs.startsWith(root + path.sep) || abs.includes(`${path.sep}target${path.sep}`)) continue;
      // A build script's `rerun-if-changed=<dir>` lands in the .d file as a
      // directory; its files are hashed through the crate's tracked files.
      if (fs.existsSync(abs) && fs.statSync(abs).isDirectory()) continue;
      files.add(rel(abs));
      const crate = crateRoot(abs);
      if (crate) crates.add(rel(crate));
    }
  }
  if (crates.size) {
    const tracked = execFileSync('git', ['ls-files', '-z', '--', ...crates], { cwd: root, encoding: 'utf8' });
    for (const f of tracked.split('\0').filter(Boolean)) files.add(f);
  }
  return [...files].sort();
}

function hashOf(files) {
  const h = createHash('sha256');
  h.update(execFileSync('rustc', ['-V'], { encoding: 'utf8' }));
  for (const f of files) {
    const abs = path.join(root, f);
    h.update(`\0${f}\0`);
    const st = fs.existsSync(abs) ? fs.statSync(abs) : null;
    h.update(st?.isFile() ? fs.readFileSync(abs) : st ? 'dir' : 'missing');
  }
  return h.digest('hex');
}

if (cmd === 'save') {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const files = inputsFor(args.bin);
  const bins = args.bin.map(rel);
  bins.forEach((b, i) => fs.copyFileSync(path.join(root, b), path.join(dir, `bin-${i}`)));
  fs.writeFileSync(manifestPath, JSON.stringify({ hash: hashOf(files), bins, files }, null, 1));
  console.log(`saved ${bins.join(', ')} (${files.length} input files)`);
} else if (cmd === 'check') {
  if (!fs.existsSync(manifestPath)) {
    output('hit', 'false');
    process.exit(0);
  }
  const { hash, bins, files } = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  if (hashOf(files) !== hash || !bins.every((_, i) => fs.existsSync(path.join(dir, `bin-${i}`)))) {
    console.log('Rust inputs changed since the cached build; building.');
    output('hit', 'false');
    process.exit(0);
  }
  bins.forEach((b, i) => {
    fs.mkdirSync(path.dirname(path.join(root, b)), { recursive: true });
    fs.copyFileSync(path.join(dir, `bin-${i}`), path.join(root, b));
    fs.chmodSync(path.join(root, b), 0o755);
  });
  console.log(`reusing ${bins.join(', ')}: ${files.length} input files unchanged`);
  output('hit', 'true');
} else {
  console.error('usage: prebuilt-rust-bin.mjs save|check --dir <dir> [--bin <path>]...');
  process.exit(2);
}
