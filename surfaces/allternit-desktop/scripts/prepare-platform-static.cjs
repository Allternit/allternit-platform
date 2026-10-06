#!/usr/bin/env node
/**
 * Build the platform static export and copy it into resources/ for the packaged desktop app.
 *
 * The Rust API serves the platform static export directly via tower-http ServeDir
 * when the desktop app runs offline.
 */

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const platformResourcesDir = path.join(desktopDir, 'resources', 'platform');

function resolveHostedUiDir() {
  // One resolver for every build path (scripts/hosted-ui.sh): ALLTERNIT_AI_PATH,
  // else .hosted-ui, else a clean worktree on origin/main — never the shared
  // allternit-ai checkout, whose stale commit kept reverting merged UI.
  //
  // The first two cases are resolved here rather than through bash: on the
  // Windows release runner bash prints a POSIX path (/d/a/...) that Node
  // cannot open, so the .hosted-ui checkout CI prepared was reported missing.
  for (const candidate of [process.env.ALLTERNIT_AI_PATH, path.join(repoRoot, '.hosted-ui')]) {
    if (candidate && fs.existsSync(path.join(candidate, 'package.json'))) return path.resolve(candidate);
  }
  try {
    const out = execFileSync('bash', ['-c', '. "$0"; resolve_hosted_ui "$1"', path.join(repoRoot, 'scripts', 'hosted-ui.sh'), repoRoot], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'inherit'],
    }).trim();
    const dir = out.split('\n').pop();
    if (dir && fs.existsSync(path.join(dir, 'package.json'))) return dir;
  } catch {
    // fall through to the error below
  }
  log('ERROR: ai.allternit.com UI not found or its build worktree could not be prepared.');
  log('Clone Allternit/allternit-ai next to this repo, or set ALLTERNIT_AI_PATH.');
  log('Do not package surfaces/platform.allternit.com — that is the cloud console.');
  process.exit(1);
}

function git(dir, args) {
  return execFileSync('git', ['-C', dir, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

/**
 * Refuse to package a stale or dirty workspace UI. Builds that fell back to
 * the shared allternit-ai checkout (parked on an old commit, with other
 * sessions' WIP) shipped week-old screens and "reverted" merged fixes on
 * every install. The UI must contain origin/main and be clean; the commit is
 * stamped into resources/platform/ui-source.json so a build can be checked.
 * ALLTERNIT_ALLOW_STALE_UI=1 overrides (e.g. testing a branch on purpose).
 */
function checkUiSource(uiDir) {
  const allowStale = process.env.ALLTERNIT_ALLOW_STALE_UI === '1';
  let head = 'unknown';
  let branch = 'unknown';
  let dirty = 0;
  let behind = null;
  try {
    head = git(uiDir, ['rev-parse', 'HEAD']);
    branch = git(uiDir, ['rev-parse', '--abbrev-ref', 'HEAD']);
    dirty = git(uiDir, ['status', '--porcelain', '--untracked-files=no']).split('\n').filter(Boolean).length;
    try {
      git(uiDir, ['fetch', '--quiet', 'origin', 'main']);
    } catch (e) {
      log(`note: could not fetch origin/main in ${uiDir} (${e.message.split('\n')[0]}); checking against the last fetch`);
    }
    behind = Number(git(uiDir, ['rev-list', '--count', 'HEAD..origin/main']));
  } catch (e) {
    log(`note: ${uiDir} is not a git checkout (${e.message.split('\n')[0]}); cannot verify it is current`);
  }
  const problems = [];
  // CI checks the UI out fresh from origin/main at the start of the job. Being
  // "behind" there only means someone merged to allternit-ai main while the
  // release was building (the release took ~1 h and failed on exactly this), so
  // it is a warning in CI. Local builds keep failing: a stale local checkout is
  // the bug this guard exists for. A dirty tree still fails everywhere.
  const inCi = process.env.GITHUB_ACTIONS === 'true' || process.env.CI === 'true';
  if (behind && inCi && !dirty) {
    log(`note (CI): Workspace UI ${head.slice(0, 9)} is ${behind} commit(s) behind origin/main because main moved during this run; packaging the commit checked out at job start.`);
  } else if (behind) {
    problems.push(`${behind} commit(s) behind origin/main`);
  }
  if (dirty) problems.push(`${dirty} uncommitted change(s)`);
  if (problems.length) {
    const msg = `Workspace UI at ${uiDir} (${branch} ${head.slice(0, 9)}) is ${problems.join(' and ')}.`;
    if (!allowStale) {
      log(`ERROR: ${msg}`);
      log('Build from a clean worktree on origin/main:');
      log('  git -C <allternit-ai> worktree add --detach ../allternit-ai-wt-build origin/main');
      log('  ALLTERNIT_AI_PATH=<that worktree> node scripts/prepare-platform-static.cjs');
      log('Or set ALLTERNIT_ALLOW_STALE_UI=1 to package it anyway.');
      process.exit(1);
    }
    log(`WARNING (ALLTERNIT_ALLOW_STALE_UI=1): ${msg}`);
  }
  return { commit: head, branch, dirty, behindMain: behind, path: uiDir };
}

function log(message) {
  process.stdout.write(`[prepare-platform-static] ${message}\n`);
}

function copyDir(src, dest) {
  if (!fs.existsSync(dest)) {
    fs.mkdirSync(dest, { recursive: true });
  }
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const srcPath = path.join(src, entry.name);
    const destPath = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      copyDir(srcPath, destPath);
    } else {
      fs.copyFileSync(srcPath, destPath);
    }
  }
}

function rmrf(p) {
  if (fs.existsSync(p)) {
    fs.rmSync(p, { recursive: true });
  }
}

function runBuild(cwd, script, envExtra = {}) {
  log(`Running pnpm ${script} in ${cwd}...`);
  try {
    execFileSync('pnpm', ['run', script], {
      cwd,
      // Windows resolves pnpm only as a .cmd shim, which execFileSync can't
      // spawn without a shell (spawnSync pnpm ENOENT in CI).
      shell: true,
      stdio: 'inherit',
      env: {
        ...process.env,
        ...envExtra,
      },
    });
  } catch (e) {
    log(`Failed to run pnpm ${script}: ${e.message}`);
    process.exit(1);
  }
}

function copyExport(src, dest, label) {
  if (!fs.existsSync(src)) {
    log(`Output directory ${src} not found for ${label}.`);
    process.exit(1);
  }
  rmrf(dest);
  fs.mkdirSync(dest, { recursive: true });
  log(`Copying ${src} -> ${dest}...`);
  copyDir(src, dest);
  log(`${label} static export ready at ${dest}`);
}

function checkCompanionExport(dir) {
  const entry = path.join(dir, 'companion.html');
  if (!fs.existsSync(entry)) {
    throw new Error(`Desktop companion entry is missing: ${entry}. Build the allternit-ai workspace with its companion.html entry.`);
  }
  const html = fs.readFileSync(entry, 'utf8');
  if (!html.includes('data-desktop-companion="true"')) {
    throw new Error(`Desktop companion entry is invalid: ${entry}`);
  }
  for (const [, asset] of html.matchAll(/(?:src|href)="\/(assets\/[^"?#]+)"/g)) {
    if (!fs.existsSync(path.join(dir, asset))) {
      throw new Error(`Desktop companion asset is missing: ${asset}`);
    }
  }
}

function checkRequiredBinaries() {
  // Fail fast if the gizzi-code brain binary is missing. A packaged app without it
  // throws at runtime ("gizzi-code binary not found" in GizziManager) — catch that
  // here, at build time, with a clear remediation. The canonical pipeline
  // (scripts/build-desktop.sh) stages this binary before the electron build.
  const resourcesBin = path.join(desktopDir, 'resources', 'bin');
  const gizziBin = path.join(resourcesBin, process.platform === 'win32' ? 'gizzi-code.exe' : 'gizzi-code');
  if (!fs.existsSync(gizziBin)) {
    log('ERROR: resources/bin/gizzi-code is missing — the packaged app would ship without a brain.');
    log('Build it first via the canonical pipeline: ../../scripts/build-desktop.sh');
    log('(which runs cmd/gizzi-code/build-production.js and copies dist/gizzi-code into resources/bin/).');
    process.exit(1);
  }
  log(`gizzi-code brain present at ${gizziBin}`);

  const voiceBin = path.join(resourcesBin, process.platform === 'win32' ? 'allternit-voice-service.exe' : 'allternit-voice-service');
  if (!fs.existsSync(voiceBin)) {
    log('ERROR: resources/bin/allternit-voice-service is missing — packaged Voice Mode would not start.');
    log('Build it first via the canonical pipeline: ../../scripts/build-desktop.sh');
    process.exit(1);
  }
  log(`voice service present at ${voiceBin}`);
  // TTS runs in a separate GPL-3.0 program the voice service starts from
  // the same directory (services/voice-tts).
  const ttsBin = path.join(resourcesBin, process.platform === 'win32' ? 'allternit-tts.exe' : 'allternit-tts');
  if (!fs.existsSync(ttsBin)) {
    log('ERROR: resources/bin/allternit-tts is missing — packaged text-to-speech would not start.');
    log('Build it first via the canonical pipeline: ../../scripts/build-desktop.sh');
    process.exit(1);
  }
  log(`TTS program present at ${ttsBin}`);

  const apiBin = path.join(resourcesBin, process.platform === 'win32' ? 'allternit-api.exe' : 'allternit-api');
  if (!fs.existsSync(apiBin)) {
    if (process.env.ALLTERNIT_ALLOW_MISSING_API === '1') {
      log('WARNING: resources/bin/allternit-api is missing; continuing because ALLTERNIT_ALLOW_MISSING_API=1.');
      log('The packaged app will fail closed at boot until a native CI/OS build stages this binary.');
    } else {
      log('ERROR: resources/bin/allternit-api is missing — the packaged app would ship without the Rust API backend.');
      log('Build it first via the canonical pipeline: ../../scripts/build-desktop.sh');
      log('Cross-packs from macOS cannot produce Windows/Linux allternit-api; set ALLTERNIT_ALLOW_MISSING_API=1 to pack anyway.');
      process.exit(1);
    }
  } else {
    log(`allternit-api present at ${apiBin}`);
  }

  const localEngineBin = path.join(resourcesBin, process.platform === 'win32' ? 'allternit-local-engine.exe' : 'allternit-local-engine');
  if (!fs.existsSync(localEngineBin)) {
    if (process.env.ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE === '1') {
      log('WARNING: resources/bin/allternit-local-engine is missing; continuing because ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE=1.');
      log('Model Lab telemetry will show "Unavailable" until a native CI/OS build stages this binary.');
    } else {
      log('ERROR: resources/bin/allternit-local-engine is missing — the packaged app would ship without the local model engine.');
      log('Build it first via the canonical pipeline: ../../scripts/build-desktop.sh');
      log('Cross-packs from macOS cannot produce Windows/Linux allternit-local-engine; set ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE=1 to pack anyway.');
      process.exit(1);
    }
  } else {
    log(`allternit-local-engine present at ${localEngineBin}`);
  }
}

function loadCompanyClerkKey() {
  const companyPath = path.resolve(desktopDir, '..', '..', 'resources', 'company.json');
  try {
    const company = JSON.parse(fs.readFileSync(companyPath, 'utf8'));
    return company.clerkPublishableKey || '';
  } catch {
    return '';
  }
}

function main() {
  checkRequiredBinaries();

  const platformDir = resolveHostedUiDir();
  log(`Workspace UI (ai.allternit.com) at ${platformDir}`);
  const uiSource = checkUiSource(platformDir);
  log(`Workspace UI commit ${uiSource.commit.slice(0, 9)} (${uiSource.branch})`);
  const ossLink = path.join(platformDir, '.oss-platform');
  try {
    fs.symlinkSync(repoRoot, ossLink, 'dir');
  } catch (e) {
    if (e && e.code !== 'EEXIST') {
      log(`note: could not link .oss-platform (${e.message})`);
    }
  }

  // NEXT_PUBLIC_ALLTERNIT_DESKTOP_AUTH activates the renderer's desktop
  // runtime-pairing bridge (useDesktopSession/buildDesktopAuthValue in
  // platform-auth-client.tsx). Without it, every packaged desktop build falls
  // through to requiring a full Clerk sign-in and the runtime-pairing screen
  // is a dead end even after pairing succeeds in the main process.
  const clerkKey = process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY || loadCompanyClerkKey();
  const buildEnv = {
    CLOUDFLARE_PAGES: '1',
    NEXT_PUBLIC_ALLTERNIT_DESKTOP_AUTH: '1',
    // Operator data plane is the local kernel. Cloud is the control plane only.
    VITE_ALLTERNIT_GATEWAY_URL: 'http://127.0.0.1:8013',
    NEXT_PUBLIC_ALLTERNIT_GATEWAY_URL: 'http://127.0.0.1:8013',
    NEXT_PUBLIC_ALLTERNIT_CLOUD_API_URL: 'https://api.allternit.com',
    VITE_CLOUD_API_URL: 'https://api.allternit.com',
    NEXT_PUBLIC_CLERK_SIGN_IN_URL: '/sign-in',
    NEXT_PUBLIC_CLERK_SIGN_UP_URL: '/sign-up',
    // Packaged desktop always ships the voice sidecar. Override .env.local
    // (which disables the probe for browser-only Vite) so Settings Voice
    // can reach the local service.
    VITE_ENABLE_VOICE_SERVICE: 'true',
  };
  if (clerkKey) {
    buildEnv.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY = clerkKey;
    buildEnv.VITE_CLERK_PUBLISHABLE_KEY = clerkKey;
  }
  runBuild(platformDir, 'build', buildEnv);
  checkCompanionExport(path.join(platformDir, 'dist'));
  copyExport(path.join(platformDir, 'dist'), platformResourcesDir, 'Workspace UI');
  fs.writeFileSync(
    path.join(platformResourcesDir, 'ui-source.json'),
    JSON.stringify({ ...uiSource, builtAt: new Date().toISOString() }, null, 2) + '\n',
  );
}

if (require.main === module) {
  main();
} else {
  module.exports = { checkUiSource };
}
