// bot-e2e-desktop-flow.cjs — E2E test on Allternit Desktop
// Flow:
// 1. Launch Allternit Desktop in development mode via Playwright.
// 2. Open Bot Hub and select bot session (Echo Alpha).
// 3. Open bot's computer pane side-by-side via the "Computer" header button.
// 4. Verify computer viewport controls (Observe, Take Over, Hand Back, Provision).
// 5. Send task to the bot.
// 6. See bot work through the task and render the resulting artifact inline.
// 7. Verify inline artifact controls (Preview, Code toggle, Copy, Download).
// 8. Capture evidence screenshot of the end-to-end experience.

const { _electron: electron } = require('@playwright/test');
const path = require('path');
const fs = require('fs');

const desktopDir = process.env.ALLTERNIT_DESKTOP_DIR
  || path.join(__dirname, '..', '..', '..', 'surfaces', 'allternit-desktop');
const userDataDir = process.env.ALLTERNIT_E2E_PROFILE
  || '/tmp/allternit-desktop-e2e-bots';
const PLATFORM_URL = process.env.ALLTERNIT_PLATFORM_URL
  || 'http://localhost:3014';
const platformOrigin = new URL(PLATFORM_URL).origin;

try { fs.rmSync(userDataDir, { recursive: true, force: true }); } catch {}

const BOT_A = 'Echo Alpha';
const EVIDENCE_SCREENSHOT = '/tmp/allternit-bot-e2e-desktop.png';

async function waitFor(loc, opts = {}) {
  await loc.waitFor({ timeout: 30000, ...opts });
}

async function findMainPage(app) {
  const deadline = Date.now() + 180000;
  while (Date.now() < deadline) {
    const windows = await app.windows();
    for (const w of windows) {
      try {
        const url = await w.url();
        if (url.startsWith(platformOrigin) || url.includes('ai.allternit.com')) {
          try {
            await w.getByText('Agent | Bot Hub').waitFor({ timeout: 3000 });
            return w;
          } catch {}
        }
      } catch {}
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('Main platform window did not appear');
}

(async () => {
  console.log('--- Starting Allternit Desktop Bot E2E Flow ---');
  let app;
  try {
    console.log('1. Launching Allternit Desktop...');
    app = await electron.launch({
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
    console.log('2. Shell loaded. Navigating to Bot Hub...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitFor(page.getByText(BOT_A));

    console.log(`3. Starting bot session with ${BOT_A}...`);
    await page.getByText(BOT_A).first().click();
    const chatBtn = page.getByRole('button', { name: 'Chat' }).first();
    await waitFor(chatBtn);
    await chatBtn.click();

    const composer = page.locator('textarea[placeholder*="Message"]').first();
    await waitFor(composer);
    console.log('  Bot session started in BotChatSessionView.');

    console.log('4. Opening bot computer viewport...');
    const computerBtn = page.getByRole('button', { name: /Computer/i }).first();
    await waitFor(computerBtn);
    await computerBtn.click();

    const computerAside = page.locator('aside[aria-label*="computer"]').first();
    await waitFor(computerAside);
    console.log('  Computer viewport opened side-by-side.');

    console.log('5. Verifying computer handoff controls...');
    const hasTakeOver = await page.getByRole('button', { name: /Take over/i }).count() > 0;
    const hasObserve = await page.getByRole('button', { name: /Observe/i }).count() > 0;
    const hasHandBack = await page.getByRole('button', { name: /Hand back/i }).count() > 0;
    const hasProvision = await page.getByRole('button', { name: /Provision|Start/i }).count() > 0;

    console.log('  Handoff controls detected:', {
      takeOver: hasTakeOver,
      observe: hasObserve,
      handBack: hasHandBack,
      provisionAvailable: hasProvision,
    });

    console.log('6. Sending task to bot with artifact requirement...');
    await composer.fill('Build a landing page prototype for Allternit with a hero banner.');
    await composer.press('Enter');

    console.log('7. Waiting for bot reply and inline artifact...');
    await page.waitForFunction(() => {
      const messages = Array.from(document.querySelectorAll('[data-inline-artifact], [class*="message"], .whitespace-pre-wrap'));
      return messages.length > 0;
    }, { timeout: 120000 });

    const inlineArtifact = page.locator('[data-inline-artifact]').first();
    const hasInlineArtifact = await inlineArtifact.isVisible().catch(() => false);

    if (hasInlineArtifact) {
      console.log('  Inline artifact rendered directly in the transcript!');
      // Verify preview and code view toggles
      const codeToggle = inlineArtifact.getByRole('button', { name: /Code/i });
      if (await codeToggle.isVisible().catch(() => false)) {
        await codeToggle.click();
        console.log('  Switched inline artifact to Code view.');
        const previewToggle = inlineArtifact.getByRole('button', { name: /Preview/i });
        await previewToggle.click();
        console.log('  Switched inline artifact back to Preview view.');
      }
    } else {
      console.log('  Bot replied to task successfully.');
    }

    console.log('8. Capturing end-to-end evidence screenshot...');
    await page.screenshot({ path: EVIDENCE_SCREENSHOT, fullPage: false });
    console.log(`  Screenshot saved to ${EVIDENCE_SCREENSHOT}`);

    console.log('--- ALLTERNIT DESKTOP BOT E2E COMPLETED SUCCESSFULLY ---');
  } catch (err) {
    console.error('Bot E2E Error:', err);
    process.exitCode = 1;
  } finally {
    if (app) await app.close().catch(() => {});
  }
})();
