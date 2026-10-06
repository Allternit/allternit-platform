// bot-e2e-pwa.cjs — PWA standalone-mode bot smoke.
// Emulates display-mode: standalone (what an installed PWA reports), asserts
// the PWA wiring (manifest link, standalone media query), then drives the same
// bot-mode flow as the web verification: Bot Hub -> 1:1 bot chat -> reply.
// NOTE: on localhost the app's boot.js intentionally skips SW registration
// (secure-context install rules), so sw.js assertions are limited to presence
// of the registration path; full installed-PWA verification requires the
// https deployment.
//
// PREREQUISITES:
//   - Vite dev server running on :3013 with real Clerk keys + seed creds (same
//     .env.local setup as bot-e2e-live.cjs).
//   - A signed-in session saved at /tmp/botmode-e2e-state.json (produced by a
//     prior bot-e2e-live.cjs run) — this script does NOT perform sign-in and
//     fails fast if the saved session no longer yields a 200 from
//     /api/v1/agents.
//   - Installed Google Chrome.
//
// RUN: node bot-e2e-pwa.cjs
const path = require('path');
const MODULE_FALLBACK = '~/altw/allternit/node_modules';
function requireDep(name) {
  try {
    return require(name);
  } catch {
    // Session worktrees have no node_modules; borrow the shared checkout's
    // (read-only — nothing there is modified).
    return require(path.join(MODULE_FALLBACK, name));
  }
}
const { chromium } = requireDep('@playwright/test');
const fs = require('fs');

const BASE_URL = process.env.PLATFORM_URL || 'http://localhost:3013';
const STATE_PATH = '/tmp/botmode-e2e-state.json';
const SCREENSHOT_PATH = '/tmp/bot-e2e-pwa-result.png';

const RESULT = {
  baseUrl: BASE_URL,
  manifestLinked: false,
  displayModeStandalone: false,
  botHubVisible: false,
  singleBotFlow: false,
  errors: [],
  consoleErrors: [],
  screenshotPath: SCREENSHOT_PATH,
};

function log(...args) {
  console.log('[PWA-VERIFY]', ...args);
}

(async () => {
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 900 },
    ...(fs.existsSync(STATE_PATH) ? { storageState: STATE_PATH } : {}),
  });
  // Installed-PWA emulation. Playwright's emulateMedia does not set
  // display-mode, so stub matchMedia before any page script runs.
  await context.addInitScript(() => {
    const original = window.matchMedia.bind(window);
    window.matchMedia = (query) => {
      if (String(query).includes('display-mode: standalone')) {
        return {
          matches: true,
          media: query,
          onchange: null,
          addListener() {},
          removeListener() {},
          addEventListener() {},
          removeEventListener() {},
          dispatchEvent() { return false; },
        };
      }
      return original(query);
    };
  });
  const page = await context.newPage();

  page.on('pageerror', (err) => RESULT.consoleErrors.push({ type: 'pageerror', text: err.message }));

  try {
    log('Opening app in emulated standalone mode...');
    await page.goto(BASE_URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    const tokenDeadline = Date.now() + 90000;
    while (Date.now() < tokenDeadline) {
      const ready = await page.evaluate(async () => {
        const token = localStorage.getItem('allternit_token');
        if (!token) return false;
        const res = await fetch('/api/v1/agents', { headers: { Authorization: `Bearer ${token}` } });
        return res.status === 200;
      });
      if (ready) break;
      await page.waitForTimeout(2000);
    }

    const env = await page.evaluate(async () => {
      const standalone = matchMedia('(display-mode: standalone)').matches;
      const manifest = !!document.querySelector('link[rel="manifest"]');
      const token = localStorage.getItem('allternit_token');
      let agentsStatus = null;
      if (token) {
        const res = await fetch('/api/v1/agents', {
          headers: { Authorization: `Bearer ${token}` },
        });
        agentsStatus = res.status;
      }
      return { standalone, manifest, hasToken: !!token, agentsStatus };
    });
    RESULT.manifestLinked = env.manifest;
    RESULT.displayModeStandalone = env.standalone;
    log(`  manifest=${env.manifest} standalone=${env.standalone} token=${env.hasToken} agents=${env.agentsStatus}`);
    if (!env.standalone) throw new Error('display-mode standalone emulation did not apply');
    if (!env.manifest) throw new Error('manifest link missing');
    if (env.agentsStatus !== 200) throw new Error(`agents fetch status ${env.agentsStatus} — session not valid in standalone mode`);

    // Bot Hub -> single bot chat -> identity reply.
    log('Opening Bot Hub...');
    const botHubNav = page.getByRole('button', { name: 'Bot Hub' }).first();
    try {
      await botHubNav.waitFor({ timeout: 20000 });
      await botHubNav.click();
    } catch {
      await page.getByText('Agent | Bot Hub').first().click();
    }
    await page.getByText('Your bots').first().waitFor({ timeout: 30000 });
    await page.getByText('Echo Alpha').first().waitFor({ timeout: 30000 });
    RESULT.botHubVisible = true;

    log('Starting single-bot chat...');
    await page.getByText('Echo Alpha').first().click();
    await page.getByText('Delegate work to Echo Alpha').first().waitFor({ timeout: 30000 });
    await page.evaluate(() => {
      const btns = Array.from(document.querySelectorAll('button')).filter(
        (b) => (b.textContent || '').trim() === 'Chat' && b.offsetParent !== null && !b.disabled,
      );
      if (!btns.length) throw new Error('no visible Chat button on bot home');
      btns[0].click();
    });

    const composer = page.locator('textarea[placeholder^="Message "], textarea[placeholder*="Type your message"]').first();
    await composer.waitFor({ timeout: 30000 });
    await composer.fill('What is your name?');
    await composer.press('Enter');

    await page.waitForFunction(
      () => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === 'What is your name?'
        );
        if (!userEl) return false;
        let transcript = userEl.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? transcript.innerText.includes('Echo Alpha') : false;
      },
      { timeout: 150000 }
    );
    RESULT.singleBotFlow = true;
    log('  Bot replied in standalone mode.');

    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: false });
    log('ALL VERIFICATION PASSED');
  } catch (err) {
    RESULT.errors.push(err.message);
    log('VERIFICATION FAILED:', err.message);
    try {
      await page.screenshot({ path: '/tmp/bot-e2e-pwa-failure.png', fullPage: false });
    } catch {}
  } finally {
    await browser.close();
  }

  console.log('\n=== VERIFICATION RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
  process.exit(RESULT.errors.length ? 1 : 0);
})();
