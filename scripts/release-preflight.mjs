#!/usr/bin/env node
/**
 * Release preflight for .github/workflows/release-desktop.yml.
 *
 * Catches packaging/configuration errors BEFORE the 40–60 minute per-platform
 * builds start. Each check below maps to a real failure class from the
 * desktop-v1.1.1 release night (2026-09-09, 6 failed runs):
 *
 *   1. Missing PyInstaller step for the voice-service sidecar
 *      → packaging dry-run: every binary prepare-platform-static.cjs
 *        hard-requires must have a producing step (or an ALLOW_MISSING
 *        env opt-out) in each platform job.
 *        (Update, run 11: voice is a Rust crate since PR #194's voice-cleanup;
 *        the PyInstaller steps were replaced by `cargo build -p voice-service`
 *        plus a whisper-cli cmake build. The packaging dry-run still guards
 *        the sidecars; the toolchain check now asserts a job cargo-builds
 *        the crate.)
 *   2. pip install services/voice failed (package is at services/voice/voice)
 *      → script existence: services/voice/voice/pyproject.toml must exist.
 *        (Update, run 11: the Python tree is gone — the check now asserts
 *        services/voice/Cargo.toml and services/voice/build-whisper.sh exist.)
 *   3. Missing allternit-local-engine binary on macOS
 *      → packaging dry-run: macOS job must build local-engine or set
 *        ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE.
 *   4. generate-provider-registry.ts needs Node >= 23 (type stripping)
 *      → toolchain matrix: any job that runs the registry generation
 *        (directly or via prepare:connector-catalog / build:electron / dist)
 *        must pin node-version >= 23.
 *   5. pnpm .cmd shim ENOENT on Windows (needs shell: true)
 *      → regression guard: prepare-platform-static.cjs must keep the
 *        shell: true fix on its execFileSync('pnpm', ...) call.
 *   6. notarize.cjs hard-failed with no Apple secrets in the repo
 *      → secrets/notarize: notarize.cjs must contain the loud-skip path
 *        for missing APPLE_* env vars.
 *   7. Packaged connector sidecar crash-looped ("Cannot find package
 *      '@hono/node-server'") because services/open-connector is excluded
 *      from the pnpm workspace and nothing installed its node_modules
 *      → connector check: every electron-builder job must run
 *        prepare-connector-sidecar.cjs; the desktop build chains must keep
 *        prepare:connector-sidecar; verify-packaged-resources.cjs must keep
 *        its hard connector-bundle gate (server.mjs + runtime markers).
 *      Same check guards the voice sidecar against shipping the pre-cleanup
 *      PyInstaller binary again (verify gate scanning for _MEIPASS/pyi_rth
 *      markers must stay in place).
 *
 * Zero dependencies; runs in seconds against the checked-out repo.
 * Exits non-zero with an actionable message per failure.
 *
 * LIMITATION: repo secrets cannot be read from CI, so this script cannot
 * verify the APPLE_* secrets actually exist — it only asserts the notarize
 * skip path is present so a secrets-absent build warns instead of dying
 * silently after "searching for node modules".
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const repoRoot = path.resolve(__dirname, '..');
const workflowPath = path.join(repoRoot, '.github', 'workflows', 'release-desktop.yml');
const desktopDir = path.join(repoRoot, 'surfaces', 'allternit-desktop');

const failures = [];
const passes = [];

function fail(message) {
  failures.push(message);
}

function pass(message) {
  passes.push(message);
}

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function existsAny(candidates) {
  return candidates.find((p) => fs.existsSync(path.join(repoRoot, p)));
}

/* ── Workflow parsing (minimal indentation-based splitter, no YAML dep) ── */

function parseJobs(workflowText) {
  const jobs = {};
  const lines = workflowText.split('\n');
  const jobsStart = lines.findIndex((l) => l === 'jobs:');
  if (jobsStart === -1) return jobs;
  let current = null;
  for (let i = jobsStart + 1; i < lines.length; i++) {
    const line = lines[i];
    const header = line.match(/^  ([A-Za-z0-9_-]+):\s*$/);
    if (header) {
      current = header[1];
      jobs[current] = [];
      continue;
    }
    if (current) jobs[current].push(line);
  }
  for (const name of Object.keys(jobs)) {
    jobs[name] = jobs[name].join('\n');
  }
  return jobs;
}

/* ── Check 1: every script/artifact path the workflow relies on exists ── */

function checkScriptExistence(workflowText) {
  // Candidate roots a `scripts/…` reference can resolve against, given the
  // workflow's working-directory usage.
  const roots = ['', 'surfaces/allternit-desktop/', 'services/voice/'];

  const referenced = new Set();
  for (const rawLine of workflowText.split('\n')) {
    const line = rawLine.replace(/#.*$/, ''); // strip comments
    for (const match of line.matchAll(/[\w./-]*scripts\/[\w./-]+\.(cjs|ts|sh|js)\b/g)) {
      referenced.add(match[0].replace(/^\.\//, ''));
    }
    for (const match of line.matchAll(/[\w./-]+\.spec\b/g)) {
      referenced.add(match[0]);
    }
  }

  // Paths the workflow uses implicitly (npm scripts, cargo manifests, build helpers).
  const implicit = [
    'surfaces/allternit-desktop/scripts/prepare-platform-static.cjs',
    'surfaces/allternit-desktop/scripts/notarize.cjs',
    'surfaces/allternit-desktop/scripts/prepare-mesh-node.cjs',
    'surfaces/allternit-desktop/scripts/download-lima.cjs',
    'surfaces/allternit-desktop/scripts/prepare-cua-driver.cjs',
    'surfaces/allternit-desktop/scripts/prepare-connector-catalog.cjs',
    'surfaces/allternit-desktop/scripts/prepare-api-binary.cjs',
    'surfaces/allternit-desktop/scripts/prepare-office-engine.cjs',
    'surfaces/allternit-desktop/scripts/verify-packaged-resources.cjs',
    'surfaces/allternit-desktop/scripts/stage-local-engine-binary.cjs',
    'services/voice/Cargo.toml', // voice-service sidecar (Rust crate; bin name voice-service)
    'services/voice/build-whisper.sh', // whisper-cli sidecar builder
    'services/open-connector/scripts/generate-provider-registry.ts',
    'services/local-engine/Cargo.toml',
    'cmd/allternit-api/Cargo.toml',
    'cmd/gizzi-code/script/build-production.js',
  ];
  for (const p of implicit) referenced.add(p);

  const missing = [];
  for (const ref of [...referenced].sort()) {
    if (ref.startsWith('surfaces/') || ref.startsWith('services/') || ref.startsWith('cmd/')) {
      if (!fs.existsSync(path.join(repoRoot, ref))) missing.push(ref);
      continue;
    }
    if (!existsAny(roots.map((r) => r + ref))) missing.push(ref);
  }

  if (missing.length === 0) {
    pass(`script existence: all ${referenced.size} referenced/implicit paths exist`);
  } else {
    for (const m of missing) {
      fail(
        `script existence: workflow references \`${m}\` but no such file exists ` +
          `(checked repo root, surfaces/allternit-desktop/, services/voice/). ` +
          `Either restore the file or remove the reference — electron-builder would ` +
          `fail ~50 minutes into the run.`
      );
    }
  }
}

/* ── Check 2: toolchain matrix (Node >= 23 for .ts registry gen, Python 3.11) ── */

function nodeMajorOf(jobText, jobName) {
  const m = jobText.match(/node-version:\s*['"]?(\d+)['"]?/);
  if (!m) return null;
  return parseInt(m[1], 10);
}

function checkToolchain(jobs) {
  // generate-provider-registry.ts is plain TS executed by `node` (type
  // stripping) — requires Node >= 23. It runs wherever the connector
  // catalog is generated: directly, via prepare:connector-catalog, or via
  // the desktop build:electron / dist npm script bundles.
  const registryTriggers =
    /generate-provider-registry|generate:catalog|prepare:connector-catalog|build:electron|npm run dist/;
  let checkedNode = 0;
  for (const [name, text] of Object.entries(jobs)) {
    if (!registryTriggers.test(text)) continue;
    const major = nodeMajorOf(text, name);
    checkedNode++;
    if (major === null) {
      fail(
        `toolchain: job \`${name}\` runs the provider-registry generation ` +
          `(needs Node >= 23 type stripping) but pins no node-version`
      );
    } else if (major < 23) {
      fail(
        `toolchain: job \`${name}\` runs scripts/generate-provider-registry.ts with ` +
          `node-version ${major} — \`node\` cannot strip types until Node 23, the step ` +
          `fails immediately. Pin node-version '24' like the other jobs.`
      );
    } else {
      pass(`toolchain: job \`${name}\` pins node ${major} (>= 23) for registry generation`);
    }
  }
  if (checkedNode === 0) {
    fail(
      'toolchain: no job appears to generate the provider registry — the connector ' +
        'catalog would be stale/empty in packaged builds. Expected a prepare:connector-catalog ' +
        'or generate:catalog step somewhere.'
    );
  }

  // Voice service sidecar: since the voice-cleanup (Python/PyInstaller tree
  // deleted, PR #194) the voice service is the Rust crate `voice-service` at
  // services/voice (Cargo bin name voice-service, staged as
  // allternit-voice-service). Each platform job must cargo-build it (or the
  // packaging dry-run below already fails it for a missing sidecar).
  const voiceJobs = Object.entries(jobs).filter(([, text]) =>
    /cargo build[^\n]*voice-service/.test(text)
  );
  if (voiceJobs.length === 0) {
    fail(
      'toolchain: no job cargo-builds the voice-service crate — the voice sidecar ' +
        '(services/voice, bin voice-service → allternit-voice-service) is never built.'
    );
  } else {
    for (const [name] of voiceJobs) {
      pass(`toolchain: job \`${name}\` cargo-builds the voice-service crate`);
    }
  }

  const macosJob = jobs['build-macos'] || '';
  if (
    !/build-whisper\.sh arm64/.test(macosJob) ||
    !/build-whisper\.sh x86_64/.test(macosJob) ||
    !/whisper-cli-arm64/.test(macosJob) ||
    !/whisper-cli-x86_64/.test(macosJob)
  ) {
    fail(
      'toolchain: build-macos must cmake whisper-cli for arm64 and x86_64 and lipo them ' +
        '(host-arch-only whisper-cli leaves the Intel app with an arm64 STT binary).'
    );
  } else {
    pass('toolchain: build-macos lipos whisper-cli for arm64 and x86_64');
  }
}

/* ── Check 3: packaging dry-run — every required resources/bin artifact ── */

/**
 * Extract the binaries prepare-platform-static.cjs checks for, and whether
 * each is optional (guarded by an ALLTERNIT_ALLOW_MISSING_* env flag).
 * Returns [{ name, optional, flag }].
 */
function extractRequiredBinaries(source) {
  const bins = [];
  const re = /path\.join\(resourcesBin,\s*process\.platform === 'win32'\s*\?\s*'([^']+)'\s*:\s*'([^']+)'/g;
  const matches = [...source.matchAll(re)];
  for (let i = 0; i < matches.length; i++) {
    const [full, winName, unixName] = matches[i];
    const start = matches[i].index;
    const end = i + 1 < matches.length ? matches[i + 1].index : source.length;
    const block = source.slice(start, end);
    const flagMatch = block.match(/ALLTERNIT_ALLOW_MISSING_([A-Z_]+)/);
    bins.push({
      name: unixName.replace(/\.exe$/, ''),
      optional: Boolean(flagMatch),
      flag: flagMatch ? `ALLTERNIT_ALLOW_MISSING_${flagMatch[1]}` : null,
    });
  }
  return bins;
}

function checkPackagingDryRun(jobs) {
  let prepareSource;
  try {
    prepareSource = fs.readFileSync(
      path.join(desktopDir, 'scripts', 'prepare-platform-static.cjs'),
      'utf8'
    );
  } catch {
    fail(
      'packaging: surfaces/allternit-desktop/scripts/prepare-platform-static.cjs is missing — ' +
        'cannot determine required resources/bin artifacts.'
    );
    return;
  }

  // Required binaries straight from the packaging gate, plus mesh-node
  // (resolved at runtime by src/main/mesh-manager.ts, staged by
  // prepare-mesh-node.cjs — not checked by prepare-platform-static).
  const bins = extractRequiredBinaries(prepareSource);
  if (!bins.some((b) => b.name === 'gizzi-code')) {
    fail(
      'packaging: could not parse any required binaries out of prepare-platform-static.cjs — ' +
        'its check shape changed; update extractRequiredBinaries() in this script.'
    );
    return;
  }
  bins.push({ name: 'mesh-node', optional: false, flag: null });
  bins.push({ name: 'allternit-mux', optional: false, flag: null });
  bins.push({ name: 'lume', optional: false, flag: null, darwinOnly: true });

  // Platform jobs = jobs that invoke electron-builder. Jobs that invoke it
  // via an npm script (build:electron / dist / pack) run the
  // prepare-platform-static gate, which hard-fails on missing binaries;
  // jobs invoking electron-builder directly skip the gate, so missing
  // binaries there are reported but non-failing (the Linux job is a
  // smoke-test whose artifacts are not published).
  const platformJobs = Object.entries(jobs).filter(
    ([, text]) => /electron-builder/.test(text) && /runs-on:/.test(text)
  );
  if (platformJobs.length === 0) {
    fail('packaging: no electron-builder job found in the workflow — nothing would be packaged.');
    return;
  }

  for (const [jobName, text] of platformJobs) {
    const gated = /prepare:platform-static|npm run (build:electron|dist|pack)\b/.test(text);
    for (const bin of bins) {
      if (bin.darwinOnly && /runs-on:\s*windows/i.test(text)) continue;
      const produced = text.includes(bin.name);
      const optedOut = bin.optional && bin.flag && new RegExp(`${bin.flag}:\\s*["']1["']`).test(text);
      if (produced) {
        pass(`packaging: ${jobName} produces resources/bin/${bin.name}`);
      } else if (optedOut) {
        pass(`packaging: ${jobName} opts out of ${bin.name} via ${bin.flag}`);
      } else if (!gated) {
        pass(
          `packaging: ${jobName} does not stage ${bin.name}, but its packaging step invokes ` +
            `electron-builder directly (no prepare-platform-static gate), so this ships incomplete ` +
            `rather than failing — worth fixing if this job ever publishes artifacts`
        );
      } else if (bin.optional) {
        fail(
          `packaging: ${jobName} neither builds \`${bin.name}\` nor sets ${bin.flag}: "1" in its ` +
            `env — prepare-platform-static.cjs hard-fails at packaging time (~50 min in). ` +
            `Add a build step that stages resources/bin/${bin.name}, or set the env opt-out.`
        );
      } else {
        fail(
          `packaging: ${jobName} has no step producing the REQUIRED \`${bin.name}\` sidecar ` +
            `(prepare-platform-static.cjs exits non-zero without it). Add the build step — ` +
            `see the desktop-v1.1.1 incident where this cost a full run per platform.`
        );
      }
    }
  }
}


function checkDesktopPrepareScripts() {
  const pkgPath = path.join(desktopDir, 'package.json');
  let pkg;
  try {
    pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  } catch {
    fail('packaging: surfaces/allternit-desktop/package.json missing');
    return;
  }
  const scripts = pkg.scripts || {};
  for (const name of ['prepare:lume', 'prepare:mux']) {
    if (!scripts[name]) {
      fail(`packaging: package.json is missing script ${name}`);
    } else {
      pass(`packaging: package.json has ${name}`);
    }
  }
  for (const chain of ['build:electron', 'build:electron:dmg', 'dist', 'pack']) {
    const cmd = scripts[chain] || '';
    if (!cmd.includes('prepare:lume') || !cmd.includes('prepare:mux')) {
      fail(`packaging: ${chain} must run prepare:lume and prepare:mux before verify`);
    } else {
      pass(`packaging: ${chain} runs prepare:lume and prepare:mux`);
    }
  }
  const extra = JSON.stringify(pkg.build || {});
  if (!extra.includes('resources/lume/${arch}')) {
    fail('packaging: electron-builder extraResources must overlay resources/lume/${arch}/ into bin/');
  } else {
    pass('packaging: extraResources overlays per-arch Lume into bin/');
  }
}

/* ── Check 4: notarize.cjs must skip loudly when Apple secrets are absent ── */

function checkNotarize(jobs) {
  const notarizePath = path.join(desktopDir, 'scripts', 'notarize.cjs');
  let source;
  try {
    source = fs.readFileSync(notarizePath, 'utf8');
  } catch {
    fail('notarize: surfaces/allternit-desktop/scripts/notarize.cjs is missing (afterSign hook target).');
    return;
  }

  // Accepts both `process.env.APPLE_ID` and `const { APPLE_ID } = process.env`.
  const checksEnv =
    /process\.env\.(APPLE_ID|APPLE_ID_PASSWORD|APPLE_TEAM_ID)/.test(source) ||
    /\{\s*APPLE_ID\s*,\s*APPLE_ID_PASSWORD\s*,\s*APPLE_TEAM_ID\s*\}\s*=\s*process\.env/.test(
      source
    );
  const checksWarn = /console\.warn/.test(source);
  const skips = /Skipping notarization/.test(source);

  if (!(checksEnv && checksWarn && skips)) {
    fail(
      'notarize: notarize.cjs has no loud-skip path for missing APPLE_* secrets ' +
        `(env check: ${checksEnv}, warn: ${checksWarn}, skip: ${skips}). With no secrets in the ` +
        'repo, electron-builder would die silently after "searching for node modules" — ' +
        'restore the skip-with-warn block (desktop-v1.1.1 incident, PR #182).'
    );
    return;
  }
  pass('notarize: notarize.cjs skips loudly (console.warn + return) when APPLE_* secrets are absent');

  const macosJob = Object.entries(jobs).find(
    ([name, text]) => /runs-on:\s*macos/.test(text) && /electron-builder/.test(text)
  );
  if (macosJob && !/APPLE_ID/.test(macosJob[1])) {
    fail(
      `notarize: macOS job \`${macosJob[0]}\` does not map APPLE_* secrets into env — ` +
        'if secrets are ever provisioned, notarization would silently never run.'
    );
  } else if (macosJob) {
    pass(
      `notarize: macOS job \`${macosJob[0]}\` maps APPLE_* secrets (note: this script cannot ` +
        'verify the secrets exist in repo settings — if absent, the skip path above applies)'
    );
  }
}

/* ── Check 5: Windows pnpm .cmd shim regression guard ── */

function checkWindowsPnpmShim() {
  const file = 'surfaces/allternit-desktop/scripts/prepare-platform-static.cjs';
  const source = read(file);
  const spawnIdx = source.indexOf("execFileSync('pnpm'");
  if (spawnIdx === -1) return; // no pnpm spawn anymore — nothing to guard
  const optionsBlock = source.slice(spawnIdx, spawnIdx + 800);
  if (/shell:\s*true/.test(optionsBlock)) {
    pass('packaging: prepare-platform-static.cjs keeps shell: true on the pnpm spawn (Windows .cmd shim fix)');
  } else {
    fail(
      `packaging: ${file} spawns pnpm via execFileSync without shell: true — on Windows pnpm ` +
        'is a .cmd shim and spawn fails with ENOENT (desktop-v1.1.1 run failure). Re-apply the fix.'
    );
  }
}

/* ── Check 6: sidecar staleness guards (connector bundle + voice binary) ── */

function checkSidecarGuards(jobs) {
  // Connector: services/open-connector is a standalone npm project,
  // deliberately excluded from the pnpm workspace. It ships inside the app
  // as a single esbuild bundle built by prepare-connector-sidecar.cjs —
  // not as src/ + node_modules (that copy silently produced the empty
  // connector-sidecar/node_modules crash loop on desktop-v1.1.1, fixed in
  // PR #244, and replaced by the bundle in the follow-up). Every job that
  // invokes electron-builder must run the prepare step before packaging.
  const preparePath = 'surfaces/allternit-desktop/scripts/prepare-connector-sidecar.cjs';
  if (!fs.existsSync(path.join(repoRoot, preparePath))) {
    fail(`connector: ${preparePath} is missing — the workflow references it but the script does not exist.`);
  }

  const platformJobs = Object.entries(jobs).filter(
    ([, text]) => /electron-builder/.test(text) && /runs-on:/.test(text)
  );
  for (const [jobName, text] of platformJobs) {
    if (/prepare-connector-sidecar/.test(text)) {
      pass(`connector: job \`${jobName}\` builds the open-connector sidecar bundle before packaging`);
    } else {
      fail(
        `connector: job \`${jobName}\` packages with electron-builder but never builds the ` +
          `open-connector sidecar bundle (services/open-connector is excluded from the pnpm ` +
          `workspace and nothing else produces resources/connector-sidecar/dist/server.mjs). ` +
          `The packaged app would ship without connectors. Add a step running ` +
          `\`node scripts/prepare-connector-sidecar.cjs\` (working-directory: surfaces/allternit-desktop).`
      );
    }
  }

  // The desktop build chains must keep the prepare step — it is what makes
  // local pack/dist/build:electron builds correct too.
  const pkg = JSON.parse(read('surfaces/allternit-desktop/package.json'));
  for (const chain of ['build:electron', 'build:electron:dmg', 'pack', 'dist']) {
    const script = (pkg.scripts && pkg.scripts[chain]) || '';
    if (script.includes('prepare:connector-sidecar')) {
      pass(`connector: package.json \`${chain}\` runs prepare:connector-sidecar`);
    } else {
      fail(
        `connector: package.json script \`${chain}\` dropped prepare:connector-sidecar — ` +
          'packaged builds silently ship without the connector sidecar bundle.'
      );
    }
  }

  // verify-packaged-resources.cjs is the last line of defence at packaging
  // time: it must keep BOTH hard gates (connector bundle present with its
  // runtime markers; staged voice binary is not the pre-cleanup PyInstaller
  // bootloader).
  const verifySource = read('surfaces/allternit-desktop/scripts/verify-packaged-resources.cjs');
  if (/allternitAnnounce/.test(verifySource) && /server\.mjs/.test(verifySource)) {
    pass('connector: verify-packaged-resources.cjs hard-fails when the connector bundle is missing/stale');
  } else {
    fail(
      'connector: verify-packaged-resources.cjs lost the bundle gate — ' +
        'a build missing the connector bundle would ship and run without connectors.'
    );
  }
  if (/_MEIPASS/.test(verifySource) && /pyi_rth/.test(verifySource)) {
    pass('voice: verify-packaged-resources.cjs hard-fails on the stale PyInstaller voice binary');
  } else {
    fail(
      'voice: verify-packaged-resources.cjs lost the PyInstaller-bootloader gate — a stale ' +
        'pre-cleanup Python voice binary could be packaged again (crashes at boot on older macOS).'
    );
  }
}

/* ── Check 7: phone-remote server is bundled in the desktop app ── */

function checkPhoneRemoteBundle() {
  // The phone-remote server (surfaces/phone-remote) serves the ScreenCaptureKit
  // frames the Fabric desktop viewer proxies to 127.0.0.1:8477. The desktop
  // app spawns and supervises it (src/main/phone-remote-manager.ts) from
  // Resources/phone-remote/server, so the electron-builder config must keep
  // copying the source tree (index.mjs + lib/ + sc_capture.swift; the helper
  // binary itself is built by first-run swiftc, never copied).
  const pkg = JSON.parse(read('surfaces/allternit-desktop/package.json'));
  const entries = (pkg.build && pkg.build.extraResources) || [];
  const entry = entries.find((e) => e && e.to === 'phone-remote/server');
  if (!entry) {
    fail(
      'phone-remote: desktop package.json build.extraResources has no entry copying ' +
        'the phone-remote server to Resources/phone-remote/server — the packaged app ' +
        'would spawn nothing on :8477 and the Fabric viewer would stay dark.'
    );
    return;
  }

  const fromDir = path.join(desktopDir, entry.from);
  const required = [
    path.join(fromDir, 'index.mjs'),
    path.join(fromDir, 'lib', 'capture.mjs'),
    path.join(fromDir, 'lib', 'ws.mjs'),
    path.join(fromDir, 'lib', 'input.mjs'),
    path.join(fromDir, 'lib', 'watchdog.mjs'),
    path.join(fromDir, 'capture', 'sc_capture.swift'),
  ];
  const missing = required.filter((p) => !fs.existsSync(p));
  if (missing.length > 0) {
    fail(
      'phone-remote: extraResources source tree is incomplete: ' +
        missing.map((p) => path.relative(repoRoot, p)).join(', ') +
        ' — the bundled phone-remote server would fail at spawn time.'
    );
    return;
  }

  // The HID input bridge (lib/input.mjs) resolves the helper at
  // <Resources>/phone-remote/input/input_helper.py — the b2585 live test
  // caught the bundled server starting without it (helper exit 2, remote
  // input dead, silent).
  const inputEntry = entries.find((e) => e && e.to === 'phone-remote/input');
  if (!inputEntry) {
    fail(
      'phone-remote: desktop package.json build.extraResources has no entry copying ' +
        'surfaces/phone-remote/input to Resources/phone-remote/input — the bundled ' +
        'server starts but the HID input helper is missing (remote input dead).'
    );
    return;
  }
  const inputDir = path.join(desktopDir, inputEntry.from);
  const inputMissing = ['input_helper.py', 'input_helper_x11.py']
    .map((f) => path.join(inputDir, f))
    .filter((p) => !fs.existsSync(p));
  if (inputMissing.length > 0) {
    fail(
      'phone-remote: input helper sources missing: ' +
        inputMissing.map((p) => path.relative(repoRoot, p)).join(', ')
    );
    return;
  }
  pass('phone-remote: desktop extraResources bundles surfaces/phone-remote/server (index.mjs + lib/ + sc_capture.swift) and the input helper (input/*.py)');
}

/* ── Check 8: updater feed matches electron-builder publish target ── */

function checkUpdaterFeed() {
  const pkg = JSON.parse(read('surfaces/allternit-desktop/package.json'));
  const publish = (pkg.build && Array.isArray(pkg.build.publish) ? pkg.build.publish : [])[0] || {};
  const publishOwner = publish.owner;
  const publishRepo = publish.repo;
  const mainSrc = read('surfaces/allternit-desktop/src/main/unified-main.ts');
  const manifestSrc = read('surfaces/allternit-desktop/src/main/manifest.ts');
  const expected = `${publishOwner}/${publishRepo}`;
  if (publishOwner && publishRepo) {
    pass(`updater: electron-builder publish target is ${expected}`);
  } else {
    fail('updater: surfaces/allternit-desktop/package.json build.publish is missing owner/repo');
    return;
  }
  const repoRe = new RegExp(`repo:\\s*['"]${expected}['"]`);
  if (repoRe.test(mainSrc)) {
    pass(`updater: updateElectronApp repo is ${expected} (matches publish)`);
  } else {
    fail(
      `updater: unified-main.ts updateElectronApp repo does not match electron-builder publish ` +
        `${expected} — auto-update would look at a 404 feed (desktop-v1.1.1 consumer-packaging P5).`
    );
  }
  const feedRe = new RegExp(`desktopFeedUrl:\\s*'https://github.com/${expected}/releases/latest'`);
  if (feedRe.test(manifestSrc)) {
    pass(`updater: PLATFORM_MANIFEST.update.desktopFeedUrl is github.com/${expected}`);
  } else {
    fail(
      `updater: manifest.ts desktopFeedUrl does not point at github.com/${expected}/releases/latest`
    );
  }
}

/* ── Main ── */

function main() {
  if (!fs.existsSync(workflowPath)) {
    console.error(`release-preflight: ${workflowPath} not found — run from the repo root.`);
    process.exit(2);
  }
  const workflowText = fs.readFileSync(workflowPath, 'utf8');
  const jobs = parseJobs(workflowText);

  checkScriptExistence(workflowText);
  checkToolchain(jobs);
  checkPackagingDryRun(jobs);
checkDesktopPrepareScripts();
  checkNotarize(jobs);
  checkWindowsPnpmShim();
  checkSidecarGuards(jobs);
  checkPhoneRemoteBundle();
  checkUpdaterFeed();

  console.log('release-preflight: release-desktop.yml checks\n');
  for (const p of passes) console.log(`  ✓ ${p}`);
  for (const f of failures) console.log(`  ✗ ${f}`);
  console.log(`\nrelease-preflight: ${passes.length} passed, ${failures.length} failed`);

  if (failures.length > 0) {
    console.error(
      '\nrelease-preflight: FAIL — fix the above before tagging. ' +
        'Each of these would otherwise burn a 40–60 minute build run.'
    );
    process.exit(1);
  }
  console.log('release-preflight: OK — safe to start the desktop release builds.');
}

main();
