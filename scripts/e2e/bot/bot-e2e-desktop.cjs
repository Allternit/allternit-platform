const { _electron: electron } = require('@playwright/test');
const path = require('path');

const fs = require('fs');
// Default to THIS repo's desktop shell (surfaces/allternit-desktop, launched
// with `npm run dev` / electron .). Override with ALLTERNIT_DESKTOP_DIR.
const desktopDir = process.env.ALLTERNIT_DESKTOP_DIR
  || path.join(__dirname, '..', '..', '..', 'surfaces', 'allternit-desktop');
const userDataDir = process.env.ALLTERNIT_E2E_PROFILE
  || '/tmp/allternit-desktop-e2e-v2';
// The desktop shell defaults to its DEV_UI (localhost:3014, see
// surfaces/allternit-desktop/src/main/config.ts) and honors
// ALLTERNIT_PLATFORM_URL — mirror that convention here.
const PLATFORM_URL = process.env.ALLTERNIT_PLATFORM_URL
  || 'http://localhost:3014';
const platformOrigin = new URL(PLATFORM_URL).origin;

// Start from a clean profile so dev mode defaults apply and rail screenshots are clean.
try { fs.rmSync(userDataDir, { recursive: true, force: true }); } catch {}

const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
const BOT_A_ID = 'baf15c34-e675-4a75-af04-0632fd545985';
const BOT_B_ID = '1321cfe4-1c50-47c9-84c5-88018faaa7c4';
const GROUP_NAME = 'AlphaBeta Squad';

async function waitFor(loc, opts = {}) {
  await loc.waitFor({ timeout: 30000, ...opts });
}

async function findMainPage(app) {
  // The desktop main process currently waits for optional services (Voice, Gizzi)
  // before it shows the platform window, which can take ~2 minutes.
  const deadline = Date.now() + 300000;
  while (Date.now() < deadline) {
    const windows = await app.windows();
    for (const w of windows) {
      try {
        const url = await w.url();
        if (url.startsWith(platformOrigin) || url.includes('ai.allternit.com')) {
          // Don't return until the shell has actually rendered.
          try {
            await w.getByText('Agent | Bot Hub').waitFor({ timeout: 5000 });
            return w;
          } catch {
            // keep waiting for the page to finish loading
          }
        }
      } catch {
        // window may be closing
      }
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('Main platform window did not appear');
}

(async () => {
  const app = await electron.launch({
    args: ['.', `--user-data-dir=${userDataDir}`],
    cwd: desktopDir,
    env: {
      ...process.env,
      NODE_ENV: 'development',
      ALLTERNIT_PLATFORM_URL: PLATFORM_URL,
      ELECTRON_ENABLE_LOGGING: '1',
    },
  });

  const page = await findMainPage(app);
  page.on('console', (msg) => console.log('[Renderer]', msg.type(), msg.text()));
  page.on('pageerror', (err) => console.log('[PageError]', err.message));
  page.on('response', (res) => {
    const status = res.status();
    const url = res.url();
    if (status === 0 || (status >= 400 && !url.includes('/health') && !url.includes('clerk') && !url.includes('sentry'))) {
      console.log('[Response]', status, url);
    }
  });

  async function dumpBody(label) {
    try {
      const text = await page.locator('body').innerText({ timeout: 5000 });
      console.error(`=== ${label} BODY TEXT (first 2500) ===`);
      console.error(text.slice(0, 2500));
      console.error('==================================');
    } catch (e) {
      console.error(`[dumpBody] ${label} failed:`, e.message);
    }
  }

  try {
    console.log('1. Waiting for shell...');
    // Wait for the platform UI to render rather than a load-state event,
    // because Electron may navigate after the initial domcontentloaded.
    await waitFor(page.getByText('Agent | Bot Hub'));
    await page.waitForTimeout(2000);

    console.log('2. Open Bot Hub...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitFor(page.getByText(BOT_A));

    console.log('3. Open bot home for', BOT_A);
    await page.getByText(BOT_A).first().click();
    const chatBtn = page.getByRole('button', { name: 'Chat' }).first();
    await waitFor(chatBtn);

    console.log('4. Start chat...');
    await chatBtn.click();
    const singleComposer = page.locator('textarea[placeholder*="message"]').first();
    await waitFor(singleComposer);

    console.log('5. Send single-bot message...');
    await singleComposer.fill('Introduce yourself and tell me your name.');
    await singleComposer.press('Enter');

    console.log('6. Wait for', BOT_A, 'reply...');
    await page.waitForFunction(
      (name) => {
        const messages = Array.from(document.querySelectorAll('[role="listitem"], [class*="message"], .whitespace-pre-wrap, .text-sm'));
        return messages.some((m) => m.textContent?.includes(name));
      },
      BOT_A,
      { timeout: 120000 }
    );
    console.log('  Single-bot identity check passed.');

    console.log('7. Back to Bot Hub for group chat...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitFor(page.getByRole('button', { name: 'New group chat' }));

    console.log('8. Start group chat...');
    await page.getByRole('button', { name: 'New group chat' }).first().click();
    await waitFor(page.getByText('Start group chat'));

    console.log('9. Select bots and name group...');
    await page.locator(`[data-group-bot-id="${BOT_A_ID}"]`).click();
    await page.locator(`[data-group-bot-id="${BOT_B_ID}"]`).click();
    await page.locator('input[placeholder*="e.g."]').first().fill(GROUP_NAME);
    await page.locator('button:has-text("Start chat")').last().click();

    console.log('10. Wait for group session...');
    const groupComposer = page.locator('textarea[placeholder*="group"]').first();
    await waitFor(groupComposer);

    console.log('11. Send group message...');
    await groupComposer.fill('Hello everyone, please say your names.');
    await groupComposer.press('Enter');

    console.log('12. Wait for both bot replies...');
    await page.waitForFunction(
      () => {
        const text = document.body.textContent || '';
        const hasA = /Echo Alpha.{0,40}(I'm|I’m|I am)/i.test(text) || /(I'm|I’m|I am).{0,40}Echo Alpha/i.test(text);
        const hasB = /Echo Beta.{0,40}(I'm|I’m|I am)/i.test(text) || /(I'm|I’m|I am).{0,40}Echo Beta/i.test(text);
        return hasA && hasB;
      },
      { timeout: 180000 }
    );
    console.log('  Group replies from both bots detected.');

    console.log('12b. Wait for spinner to clear...');
    await page.waitForFunction(
      () => !document.body.innerText.includes('Bots are thinking'),
      { timeout: 60000 }
    );
    console.log('  Spinner cleared.');

    console.log('13. Check Recents does not list bot/group sessions...');
    await page.getByTitle('Open all recents').first().click();
    await waitFor(page.getByRole('heading', { name: 'Recents' }).first());
    // Only inspect the Recents list/panel, not the left rail or body.
    const recentsPanelText = await page.locator('[role="tabpanel"], [data-testid="recents-panel"], .recents-list').first().innerText({ timeout: 5000 }).catch(() => '');
    if (recentsPanelText.includes(GROUP_NAME) || recentsPanelText.includes('Echo Alpha Session')) {
      throw new Error('Recents incorrectly contains bot/group session');
    }
    console.log('  Recents purity check passed.');

    console.log('14. Screenshot...');
    await page.screenshot({ path: '/tmp/bot-e2e-result.png', fullPage: false });

    console.log('E2E PASSED');
  } catch (err) {
    console.error('E2E FAILED:', err.message);
    await dumpBody('failure');
    await page.screenshot({ path: '/tmp/bot-e2e-failure.png', fullPage: false });
    process.exitCode = 1;
  } finally {
    await app.close();
  }
})();
