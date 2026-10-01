#!/usr/bin/env node
/**
 * Pre-build guard for the Electron packaging pipeline.
 *
 * Fails fast when the resources that electron-builder copies into the
 * packaged app are missing. This catches the common failure mode where
 * `npm run dist` is invoked before `scripts/build-desktop.sh` has staged
 * the Rust API binary, gizzi-code brain, voice service, and platform static
 * export.
 */

const fs = require('fs');
const path = require('path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const resourcesDir = path.join(desktopDir, 'resources');
const connectorCatalogDir = path.join(repoRoot, 'services', 'open-connector', 'catalog', 'apps');

function log(message) {
  process.stdout.write(`[verify-packaged-resources] ${message}\n`);
}

function errorAndExit(message) {
  process.stderr.write(`[verify-packaged-resources] ✗ ${message}\n`);
  process.exit(1);
}

const binaryName = process.platform === 'win32' ? 'allternit-api.exe' : 'allternit-api';
const localEngineName = process.platform === 'win32' ? 'allternit-local-engine.exe' : 'allternit-local-engine';
const gizziName = process.platform === 'win32' ? 'gizzi-code.exe' : 'gizzi-code';
const voiceName = process.platform === 'win32' ? 'allternit-voice-service.exe' : 'allternit-voice-service';
const whisperName = process.platform === 'win32' ? 'whisper-cli.exe' : 'whisper-cli';

const required = [
  {
    path: path.join(resourcesDir, 'bin', binaryName),
    label: 'Rust API binary (allternit-api)',
    buildStep: 'scripts/build-desktop.sh (or npm run stage:api-binary)',
  },
  {
    path: path.join(resourcesDir, 'bin', localEngineName),
    label: 'Local engine binary (allternit-local-engine)',
    buildStep: 'scripts/build-desktop.sh (or npm run stage:local-engine)',
  },
  {
    path: path.join(resourcesDir, 'bin', gizziName),
    label: 'Gizzi Code brain binary (gizzi-code)',
    buildStep: 'scripts/build-desktop.sh',
  },
  {
    path: path.join(resourcesDir, 'bin', voiceName),
    label: 'Voice service binary (allternit-voice-service, Rust + whisper.cpp)',
    buildStep: 'scripts/build-desktop.sh',
  },
  {
    path: path.join(resourcesDir, 'bin', whisperName),
    label: 'whisper-cli (local STT engine)',
    buildStep: 'services/voice/build-whisper.sh (via scripts/build-desktop.sh)',
  },
  // allternit-mux is Unix-only (its API is a Unix socket), so Windows ships without it.
  ...(process.platform === 'win32'
    ? []
    : [
        {
          path: path.join(resourcesDir, 'bin', 'allternit-mux'),
          label: 'allternit-mux (PTY daemon gizzi auto-spawns for /pty)',
          buildStep: 'npm run prepare:mux (or scripts/build-desktop.sh)',
        },
      ]),
  {
    path: path.join(resourcesDir, 'platform', 'index.html'),
    label: 'Platform static export',
    buildStep: 'npm run prepare:platform-static (or scripts/build-desktop.sh)',
  },
  {
    path: path.join(resourcesDir, 'platform', 'companion.html'),
    label: 'Desktop companion entry',
    buildStep: 'Build the allternit-ai workspace with its companion.html entry, then npm run prepare:platform-static',
  },
  {
    path: path.join(resourcesDir, 'computer-use', 'acu', 'launch.py'),
    label: 'ACU computer-use gateway (launch.py)',
    buildStep: 'npm run prepare:acu-gateway',
  },
  {
    path: path.join(repoRoot, 'surfaces', 'phone-remote', 'client', 'index.html'),
    label: 'phone-remote viewer (client/index.html)',
    buildStep: 'surfaces/phone-remote/client must ship in extraResources',
  },
  {
    path: path.join(repoRoot, 'tools', 'allternit-tools', 'allternit-tools.mjs'),
    label: 'allternit-tools installer (ships in extraResources as allternit-tools/)',
    buildStep: 'tools/allternit-tools must ship in extraResources',
  },
  {
    path: path.join(repoRoot, 'infrastructure', 'executor', 'ao-engine', 'src', 'ao', 'harness', 'harness.json'),
    label: 'ao harness.json (tool wiring data for allternit-tools)',
    buildStep: 'infrastructure/executor/ao-engine/src/ao/harness/harness.json must ship in extraResources',
  },
  // NOTE: the compiled sc_capture helper is deliberately NOT hard-required
  // here — it is a gitignored runtime artifact (capture.mjs swiftc-builds it
  // on first launch) and no CI step produces it. Requiring it would red the
  // release workflow. extraResources ships the prebuilt binary when present.
];

let failed = false;

const allowMissingApi = process.env.ALLTERNIT_ALLOW_MISSING_API === '1';
const allowMissingLocalEngine = process.env.ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE === '1';

for (const item of required) {
  if (fs.existsSync(item.path)) {
    log(`✓ ${item.label}: ${item.path}`);
    continue;
  }
  const isApi = item.path.endsWith(binaryName);
  if (isApi && allowMissingApi) {
    process.stderr.write(
      `[verify-packaged-resources] ⚠ Missing ${item.label} (allowed by ALLTERNIT_ALLOW_MISSING_API=1)\n` +
      `    Expected at: ${item.path}\n` +
      `    Packaged app will fail closed at boot until a native CI/OS build stages this binary.\n`
    );
    continue;
  }
  const isLocalEngine = item.path.endsWith(localEngineName);
  if (isLocalEngine && allowMissingLocalEngine) {
    process.stderr.write(
      `[verify-packaged-resources] ⚠ Missing ${item.label} (allowed by ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE=1)\n` +
      `    Expected at: ${item.path}\n` +
      `    Model Lab telemetry will show "Unavailable" until a native CI/OS build stages this binary.\n`
    );
    continue;
  }
  failed = true;
  process.stderr.write(
    `[verify-packaged-resources] ✗ Missing ${item.label}\n` +
    `    Expected at: ${item.path}\n` +
    `    Build it with: ${item.buildStep}\n`
  );
}


if (process.platform === 'darwin') {
  const hostArch = process.arch === 'arm64' ? 'arm64' : 'x64';
  const lumeArchs = process.env.ALLTERNIT_LUME_ARCHS
    ? process.env.ALLTERNIT_LUME_ARCHS.split(/[,\s]+/).filter(Boolean)
    : [hostArch];
  for (const arch of lumeArchs) {
    const launcher = path.join(resourcesDir, 'lume', arch, 'lume');
    const realBin = path.join(resourcesDir, 'lume', arch, 'lume.app', 'Contents', 'MacOS', 'lume');
    const launcherOk = fs.existsSync(launcher);
    let realOk = false;
    try {
      realOk = fs.existsSync(realBin) && fs.statSync(realBin).size > 1024 * 1024;
    } catch {
      realOk = false;
    }
    if (launcherOk && realOk) {
      log(`✓ Lume ${arch} (${fs.statSync(realBin).size} bytes) (${realBin})`);
    } else {
      failed = true;
      process.stderr.write(
        `[verify-packaged-resources] ✗ Missing Lume ${arch} (launcher + lume.app Mach-O)\n` +
        `    Expected launcher: ${launcher}\n` +
        `    Expected binary:   ${realBin} (>1MB)\n` +
        '    Build it with: npm run prepare:lume\n'
      );
    }
  }
}

const catalogFiles = fs.existsSync(connectorCatalogDir)
  ? fs.readdirSync(connectorCatalogDir).filter((name) => name.endsWith('.json'))
  : [];
if (catalogFiles.length === 0) {
  failed = true;
  process.stderr.write(
    `[verify-packaged-resources] ✗ Missing connector sidecar catalog\n` +
    `    Expected JSON files in: ${connectorCatalogDir}\n` +
    `    Build it with: npm run prepare:connector-catalog\n`
  );
} else {
  log(`✓ Connector sidecar catalog: ${catalogFiles.length} providers (${connectorCatalogDir})`);
}

// The voice sidecar must be the Rust binary, not the PyInstaller-packaged
// Python tree that predates the voice-cleanup. Stale copies of the old
// bootloader can survive in resources/bin (copied from an old checkout) —
// it crashes at boot on older macOS (pyexpat built for a newer SDK) and
// Voice Mode silently dies.
function isPyInstallerBootloader(filePath) {
  // One-file PyInstaller bootchains embed these marker strings; the Rust
  // binary never does. Scan the first 64 MB — markers live in the early
  // LOAD segments.
  const handle = fs.openSync(filePath, 'r');
  try {
    const size = Math.min(fs.fstatSync(handle).size, 64 * 1024 * 1024);
    const buf = Buffer.alloc(size);
    fs.readSync(handle, buf, 0, size, 0);
    const text = buf.toString('latin1');
    return text.includes('_MEIPASS') || text.includes('pyi_rth') || text.includes('PyInstaller');
  } finally {
    fs.closeSync(handle);
  }
}

const voicePath = path.join(resourcesDir, 'bin', voiceName);
if (fs.existsSync(voicePath) && isPyInstallerBootloader(voicePath)) {
  failed = true;
  process.stderr.write(
    `[verify-packaged-resources] ✗ ${voicePath} is the PRE-CLEANUP PyInstaller voice binary, not the Rust voice-service.\n` +
    `    It crashes at boot (pyexpat SDK mismatch) and Voice Mode will not start.\n` +
    '    Rebuild it with: cargo build --release -p voice-service && cp target/release/voice-service ' +
    path.join('surfaces', 'allternit-desktop', 'resources', 'bin', voiceName) + '\n'
  );
} else if (fs.existsSync(voicePath)) {
  log(`✓ Voice service binary is not a PyInstaller bootloader (${voicePath})`);
}

// The connector sidecar ships as a single esbuild bundle — no src/ tree, no
// node_modules copy (that silently-empty copy is what crash-looped the
// sidecar before the bundle existed). The bundle must exist and carry the
// runtime markers prepare-connector-sidecar.cjs checks for.
const connectorBundlePath = path.join(
  resourcesDir, 'connector-sidecar', 'dist', 'server.mjs'
);
const connectorBundleMarkers = ['allternitAnnounce', 'connect server listening', 'node:sqlite'];
if (!fs.existsSync(connectorBundlePath)) {
  failed = true;
  process.stderr.write(
    `[verify-packaged-resources] ✗ Connector sidecar bundle missing\n` +
    `    Expected at: ${connectorBundlePath}\n` +
    '    Build it with: npm run prepare:connector-sidecar\n'
  );
} else {
  const bundleText = fs.readFileSync(connectorBundlePath, 'utf8');
  const missingMarkers = connectorBundleMarkers.filter((m) => !bundleText.includes(m));
  if (missingMarkers.length > 0) {
    failed = true;
    process.stderr.write(
      `[verify-packaged-resources] ✗ Connector sidecar bundle is missing runtime markers: ${missingMarkers.join(', ')}\n` +
      `    The bundle is stale or was built from the wrong entry point.\n` +
      '    Rebuild it with: npm run prepare:connector-sidecar\n'
    );
  } else {
    const mb = (fs.statSync(connectorBundlePath).size / 1024 / 1024).toFixed(1);
    log(`✓ Connector sidecar bundle (${mb} MB, markers ok) (${connectorBundlePath})`);
  }
}

if (failed) {
  process.stderr.write(
    '\n[verify-packaged-resources] Packaged resources are incomplete. ' +
    'Run the full staging pipeline first:\n' +
    '    bash scripts/build-desktop.sh\n' +
    'Or, for local development packaging only, see npm run stage:api-binary.\n'
  );
  process.exit(1);
}

log('All packaged resources present.');
