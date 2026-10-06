// bot-e2e-pwa-fabric.cjs — Fabric Session PWA bot-mode smoke (Phase 1C).
// Viewport 390×844. Hits the dedicated fabric-session entry (not the full
// web shell). Asserts a Bots section after sign-in when a saved session
// exists. Does NOT start a Vite server.
//
// PREREQUISITES:
//   - Vite dev server on :3013 (`cd surfaces/ai.allternit.com && pnpm dev`)
//   - Optional signed-in storageState at /tmp/botmode-e2e-state.json
//
// RUN: node bot-e2e-pwa-fabric.cjs
// ENTRY: http://localhost:3013/fabric-session.html
const path = require('path');
const fs = require('fs');
const http = require('http');

const WORKTREE_NM = path.join(__dirname, '..', '..', '..', 'node_modules');
const SURFACE_NM = path.join(__dirname, '..', '..', '..', 'surfaces/ai.allternit.com/node_modules');
const MODULE_FALLBACK = '~/altw/allternit/node_modules';

function requireDep(name) {
  const tries = [name, path.join(WORKTREE_NM, name), path.join(SURFACE_NM, name), path.join(MODULE_FALLBACK, name)];
  let last;
  for (const candidate of tries) {
    try {
      return require(candidate);
    } catch (err) {
      last = err;
    }
  }
  throw last;
}

const { chromium } = requireDep('@playwright/test');

const BASE_URL = process.env.PLATFORM_URL || 'http://localhost:3013';
const FABRIC_URL = process.env.FABRIC_SESSION_URL || `${BASE_URL.replace(/\/$/, '')}/fabric-session.html`;
const STATE_PATH = '/tmp/botmode-e2e-state.json';
const SCREENSHOT_PATH = path.join(
  process.env.HOME || '/tmp',
  '.agent-orchestrator/evidence/fabric-pwa-bot-mode-ui/pwa-fabric-390x844.png',
);

const RESULT = {
  skipped: false,
  skipReason: null,
  fabricUrl: FABRIC_URL,
  botsSection: false,
  screenshotPath: SCREENSHOT_PATH,
  errors: [],
};

function log(...args) {
  console.log('[PWA-FABRIC]', ...args);
}

function probe(url) {
  return new Promise((resolve) => {
    const req = http.get(url, (res) => {
      res.resume();
      resolve(res.statusCode && res.statusCode < 500);
    });
    req.on('error', () => resolve(false));
    req.setTimeout(2000, () => {
      req.destroy();
      resolve(false);
    });
  });
}

(async () => {
  const up = await probe(FABRIC_URL);
  if (!up) {
    RESULT.skipped = true;
    RESULT.skipReason = `Server not running at ${FABRIC_URL}. Start with: cd surfaces/ai.allternit.com && pnpm dev`;
    log('SKIP', RESULT.skipReason);
    console.log(JSON.stringify(RESULT, null, 2));
    process.exit(0);
  }

  const hasState = fs.existsSync(STATE_PATH);
  if (!hasState) {
    RESULT.skipped = true;
    RESULT.skipReason = `No storageState at ${STATE_PATH}. Sign in once via bot-e2e-live.cjs, then re-run. Command: node bot-e2e-pwa-fabric.cjs`;
    log('SKIP', RESULT.skipReason);
    console.log(JSON.stringify(RESULT, null, 2));
    process.exit(0);
  }

  fs.mkdirSync(path.dirname(SCREENSHOT_PATH), { recursive: true });

  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    storageState: STATE_PATH,
  });
  const page = await context.newPage();
  await page.emulateMedia({
    features: [{ name: 'display-mode', value: 'standalone' }],
  });

  try {
    log('Opening Fabric Session PWA at', FABRIC_URL);
    await page.goto(FABRIC_URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    const bots = page.getByText('Bots', { exact: true });
    await bots.waitFor({ timeout: 15000 });
    RESULT.botsSection = await bots.isVisible();
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
    log('Bots section visible:', RESULT.botsSection);
  } catch (err) {
    RESULT.errors.push(err instanceof Error ? err.message : String(err));
    try {
      await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
    } catch {
      // ignore
    }
  } finally {
    await browser.close();
  }

  console.log(JSON.stringify(RESULT, null, 2));
  process.exit(RESULT.botsSection && RESULT.errors.length === 0 ? 0 : 1);
})();
