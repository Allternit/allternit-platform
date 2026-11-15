#!/usr/bin/env node
/**
 * Stage the Meetings bot (services/meeting-bot) for the app and the runtime
 * package. The bot runs on the user's own computer: allternit-api spawns
 * `<node> meeting-bot/dist/runner.js` (ALLTERNIT_MEETING_BOT_DIR and
 * ALLTERNIT_MEETING_NODE come from backend-manager.ts).
 *
 * Output (under the resources root, default surfaces/allternit-desktop/resources):
 *   meeting-bot/package.json, dist/, node_modules/
 *       `pnpm deploy --prod` of @allternit/meeting-bot, pinned by the workspace
 *       lockfile, flattened into a plain node_modules tree with no symlinks
 *       (the runtime manifest and electron-builder copy regular files only).
 *       Holds no per-OS native package.
 *   meeting-bot-native/<os>-<arch>/@livekit/rtc-ffi-bindings-<triple>/
 *       the @livekit/rtc-node native binding for one target. electron-builder
 *       overlays it into meeting-bot/node_modules/ per OS and arch
 *       (package.json mac/win/linux extraResources, `${arch}`), and
 *       runtime-package.cjs lists it at the same place in the manifest.
 *
 * Targets: --platform <darwin|linux|win32> --arch <arm64|x64>[,<arch>]
 * Default: ALLTERNIT_RUNTIME_PLATFORM when set (e.g. linux-x64 for cloud
 * computers), else the host. On a Mac host the default is both darwin arches,
 * because one macOS build makes the arm64 and the x64 app.
 * --out <dir> (or ALLTERNIT_RUNTIME_RESOURCES) stages into another resources root.
 *
 * The workspace installs every OS's binding (pnpm-workspace.yaml
 * supportedArchitectures), so any host can stage any target.
 */

const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const botDir = path.join(repoRoot, 'services', 'meeting-bot');
const BOT_PKG = '@allternit/meeting-bot';
const BINDING_PREFIX = '@livekit/rtc-ffi-bindings-';
// Never loaded at run time: source maps, type declarations, TypeScript sources
// and docs. Half the files in the tree; each one is a manifest entry and an
// upload on a runtime publish. Licenses stay.
const DEV_ONLY = /(\.map|\.d\.[mc]?ts|\.[mc]?ts|\.md)$/i;

const TRIPLES = {
  'darwin-arm64': 'darwin-arm64',
  'darwin-x64': 'darwin-x64',
  'linux-x64': 'linux-x64-gnu',
  'linux-arm64': 'linux-arm64-gnu',
  'win32-x64': 'win32-x64-msvc',
};

function log(message) {
  process.stdout.write(`[prepare-meeting-bot] ${message}\n`);
}

function fail(message) {
  process.stderr.write(`[prepare-meeting-bot] ✗ ${message}\n`);
  process.exit(1);
}

function arg(name) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return undefined;
  const v = process.argv[i + 1];
  if (!v || v.startsWith('--')) fail(`--${name} needs a value`);
  return v;
}

function targets() {
  const envPlatform = process.env.ALLTERNIT_RUNTIME_PLATFORM;
  const [envOs, envArch] = envPlatform ? envPlatform.split('-') : [];
  const platform = arg('platform') || envOs || process.platform;
  const archArg = arg('arch') || envArch;
  const arches = archArg
    ? archArg.split(',').map((a) => a.trim()).filter(Boolean)
    : platform === 'darwin' && process.platform === 'darwin'
      ? ['arm64', 'x64']
      : [process.arch];
  const list = arches.map((a) => `${platform}-${a}`);
  for (const t of list) if (!TRIPLES[t]) fail(`unsupported target ${t} (supported: ${Object.keys(TRIPLES).join(', ')})`);
  return list;
}

function pnpm(args, cwd = repoRoot) {
  // shell: true so `pnpm` resolves on Windows, where it is a .cmd shim.
  execFileSync('pnpm', args, { cwd, stdio: 'inherit', shell: process.platform === 'win32' });
}

/**
 * Copy a pnpm (isolated, symlinked) node_modules tree into a plain nested one.
 * In the isolated layout a package's dependencies, optional dependencies and
 * resolved peers are the siblings in `.pnpm/<id>/node_modules/`. Each one is
 * placed at the top level unless a different version already holds that name
 * on the way up, in which case it nests under the dependent (npm-style).
 */
function flatten(deployDir, outNodeModules, skip) {
  const placed = new Map(); // node_modules dir -> Map<name, realpath>
  const slot = (dir) => {
    if (!placed.has(dir)) placed.set(dir, new Map());
    return placed.get(dir);
  };
  const siblingsOf = (real, name) => {
    // real = …/.pnpm/<id>/node_modules/<name>; name may be scoped.
    const dir = path.resolve(real, ...name.split('/').map(() => '..'));
    const out = [];
    for (const e of fs.readdirSync(dir)) {
      if (e.startsWith('.')) continue;
      if (e.startsWith('@')) {
        for (const s of fs.readdirSync(path.join(dir, e))) out.push(`${e}/${s}`);
      } else out.push(e);
    }
    return out.filter((n) => n !== name).map((n) => [n, path.join(dir, n)]);
  };
  let count = 0;
  const queue = [];
  const place = (name, real, nmDir, chain) => {
    const dest = path.join(nmDir, ...name.split('/'));
    slot(nmDir).set(name, real);
    fs.cpSync(real, dest, {
      recursive: true,
      dereference: true,
      filter: (src) => !path.relative(real, src).split(path.sep).includes('node_modules') && !DEV_ONLY.test(src),
    });
    count += 1;
    queue.push({ name, real, chain: [path.join(dest, 'node_modules'), ...chain] });
  };

  const rootPkg = JSON.parse(fs.readFileSync(path.join(deployDir, 'package.json'), 'utf8'));
  for (const name of Object.keys({ ...rootPkg.dependencies, ...rootPkg.optionalDependencies })) {
    if (skip(name)) continue;
    const link = path.join(deployDir, 'node_modules', ...name.split('/'));
    if (!fs.existsSync(link)) fail(`deploy is missing ${name}`);
    place(name, fs.realpathSync(link), outNodeModules, [outNodeModules]);
  }
  while (queue.length) {
    const { name, real, chain } = queue.shift();
    for (const [dep, link] of siblingsOf(real, name)) {
      if (skip(dep) || !fs.existsSync(link)) continue; // an optional dep for another OS
      const depReal = fs.realpathSync(link);
      const holder = chain.find((dir) => slot(dir).has(dep));
      if (holder && slot(holder).get(dep) === depReal) continue;
      if (holder) place(dep, depReal, chain[0], chain); // a different version is visible: nest
      else place(dep, depReal, outNodeModules, [outNodeModules]);
    }
  }
  return count;
}

function main() {
  const list = targets();
  const resources = path.resolve(arg('out') || process.env.ALLTERNIT_RUNTIME_RESOURCES || path.join(desktopDir, 'resources'));
  const outDir = path.join(resources, 'meeting-bot');
  const nativeRoot = path.join(resources, 'meeting-bot-native');
  if (!fs.existsSync(path.join(botDir, 'package.json'))) fail(`missing ${botDir}`);

  if (!fs.existsSync(path.join(botDir, 'node_modules', '@livekit', 'rtc-node'))) {
    log(`Installing ${BOT_PKG} dependencies…`);
    pnpm(['install', '--frozen-lockfile', '--ignore-scripts', '--filter', `${BOT_PKG}...`]);
  }
  log('Building (tsc)…');
  pnpm(['--filter', BOT_PKG, 'run', 'build']);
  if (!fs.existsSync(path.join(botDir, 'dist', 'runner.js'))) fail('build did not produce dist/runner.js');

  // pnpm deploy from the shared workspace lockfile (pinned versions). The
  // workspace does not set inject-workspace-packages, so pass it here; the
  // --legacy deploy would re-resolve without the lockfile.
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'meeting-bot-deploy-'));
  const deployDir = path.join(tmp, 'deploy');
  try {
    log('pnpm deploy --prod…');
    pnpm(['--filter', BOT_PKG, 'deploy', '--prod', '--ignore-scripts', '--config.inject-workspace-packages=true', deployDir]);

    fs.rmSync(outDir, { recursive: true, force: true });
    fs.mkdirSync(outDir, { recursive: true });
    const pkg = JSON.parse(fs.readFileSync(path.join(deployDir, 'package.json'), 'utf8'));
    delete pkg.devDependencies;
    delete pkg.scripts;
    fs.writeFileSync(path.join(outDir, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
    fs.cpSync(path.join(deployDir, 'dist'), path.join(outDir, 'dist'), {
      recursive: true,
      // dist/test is the test build (`pnpm test`), not part of the bot.
      filter: (src) => path.relative(path.join(deployDir, 'dist'), src).split(path.sep)[0] !== 'test',
    });
    const n = flatten(deployDir, path.join(outDir, 'node_modules'), (name) => name.startsWith(BINDING_PREFIX));
    log(`Flattened ${n} packages into ${path.relative(repoRoot, outDir)}/node_modules`);

    fs.rmSync(nativeRoot, { recursive: true, force: true });
    // rtc-node -> rtc-ffi-bindings -> rtc-ffi-bindings-<triple> (an optional dep of the bindings
    // package): each hop is a sibling in that package's `.pnpm/<id>/node_modules/@livekit/`.
    const rtcNode = fs.realpathSync(path.join(deployDir, 'node_modules', '@livekit', 'rtc-node'));
    const ffi = fs.realpathSync(path.join(path.dirname(rtcNode), 'rtc-ffi-bindings'));
    for (const target of list) {
      const triple = TRIPLES[target];
      const name = `${BINDING_PREFIX}${triple}`;
      const link = path.join(path.dirname(ffi), name.split('/')[1]);
      if (!fs.existsSync(link)) fail(`${name} is not installed (pnpm-workspace.yaml supportedArchitectures must include ${target})`);
      const dest = path.join(nativeRoot, target, '@livekit', name.split('/')[1]);
      fs.cpSync(fs.realpathSync(link), dest, { recursive: true, dereference: true });
      const nodeFile = path.join(dest, `rtc-node.${triple}.node`);
      if (!fs.existsSync(nodeFile)) fail(`${name} has no rtc-node.${triple}.node`);
      log(`✓ ${target}: ${path.relative(repoRoot, dest)} (${(fs.statSync(nodeFile).size / 1e6).toFixed(1)} MB)`);
    }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }

  for (const required of ['dist/runner.js', 'node_modules/@livekit/rtc-node/package.json', 'node_modules/@livekit/rtc-ffi-bindings/package.json', 'node_modules/playwright-core/package.json']) {
    if (!fs.existsSync(path.join(outDir, required))) fail(`staged bot is missing ${required}`);
  }
  log(`✓ meeting-bot staged for ${list.join(', ')} → ${path.relative(repoRoot, outDir) || outDir}`);
}

main();
