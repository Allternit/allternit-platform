const { _electron: electron } = require('@playwright/test');
const path = require('path');

const desktopDir = '~/Desktop/allternit-workspace/allternit-session-desktop-cloud-mvp/surfaces/allternit-desktop';
const userDataDir = '/tmp/allternit-desktop-e2e';

const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
const GROUP_NAME = 'AlphaBeta Squad';

async function waitForLocator(loc, opts = {}) {
  await loc.waitFor({ timeout: 30000, ...opts });
}

async function sendAndWaitForReply(page, inputSelector, message, replyPredicate, maxWait = 120000) {
  const composer = page.locator(inputSelector).first();
  await waitForLocator(composer);
  await composer.fill(message);
  await composer.press('Enter');
  await page.waitForFunction(
    (sel, pred) => {
      const messages = Array.from(document.querySelectorAll(sel));
      const texts = messages.map((m) => m.textContent || '');
      return texts.some((t) => pred(t));
    },
    `[role="listitem"], [class*="message"], .whitespace-pre-wrap, .text-sm`,
    replyPredicate.toString(),
    { timeout: maxWait }
  );
}

(async () => {
  const app = await electron.launch({
    args: ['.', `--user-data-dir=${userDataDir}`],
    cwd: desktopDir,
    env: {
      ...process.env,
      NODE_ENV: 'development',
      ALLTERNIT_PLATFORM_URL: 'http://localhost:3013',
      ELECTRON_ENABLE_LOGGING: '1',
    },
  });

  const page = await app.firstWindow();
  page.on('console', (msg) => console.log('[Renderer]', msg.type(), msg.text()));
  page.on('pageerror', (err) => console.log('[PageError]', err.message));

  try {
    console.log('1. Waiting for shell...');
    await page.waitForLoadState('domcontentloaded');
    await page.waitForTimeout(2000);
    try {
      await waitForLocator(page.getByText('Agent | Bot Hub'));
    } catch (e) {
      const bodyText = await page.locator('body').innerText();
      console.error('=== BODY TEXT (first 2000 chars) ===');
      console.error(bodyText.slice(0, 2000));
      console.error('=====================================');
      throw e;
    }

    console.log('2. Open Bot Hub...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitForLocator(page.getByText(BOT_A));

    console.log('3. Open bot home for', BOT_A);
    await page.getByText(BOT_A).first().click();
    await waitForLocator(page.getByRole('button', { name: 'Chat' }));

    console.log('4. Start chat...');
    await page.getByRole('button', { name: 'Chat' }).first().click();
    const singleComposer = page.locator('textarea[placeholder*="Type your message"]').first();
    await waitForLocator(singleComposer);

    console.log('5. Send single-bot message...');
    await singleComposer.fill('Introduce yourself and tell me your name.');
    await singleComposer.press('Enter');

    console.log('6. Wait for', BOT_A, 'reply...');
    await page.waitForFunction(
      () => {
        const text = document.body.innerText;
        return text.includes('Echo Alpha') && /assistant|Echo Alpha/.test(text);
      },
      { timeout: 120000 }
    );
    const singleBodyText = await page.locator('body').innerText();
    if (!singleBodyText.includes('Echo Alpha')) {
      throw new Error('Single bot reply did not contain Echo Alpha name');
    }
    console.log('  Single-bot identity check passed.');

    console.log('7. Back to Bot Hub for group chat...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitForLocator(page.getByRole('button', { name: 'New group chat' }));

    console.log('8. Start group chat...');
    await page.getByRole('button', { name: 'New group chat' }).first().click();
    await waitForLocator(page.getByText('Start group chat'));

    console.log('9. Select bots and name group...');
    await page.getByText(BOT_A).first().click();
    await page.getByText(BOT_B).first().click();
    await page.locator('input[placeholder*="e.g."]').first().fill(GROUP_NAME);
    await page.getByRole('button', { name: 'Start chat' }).first().click();

    console.log('10. Wait for group session...');
    const groupComposer = page.locator('textarea[placeholder*="Message the group"]').first();
    await waitForLocator(groupComposer);

    console.log('11. Send group message...');
    await groupComposer.fill('Hello everyone, please say your names.');
    await groupComposer.press('Enter');

    console.log('12. Wait for both bot replies...');
    await page.waitForFunction(
      () => {
        const text = document.body.innerText;
        return text.includes('Echo Alpha') && text.includes('Echo Beta');
      },
      { timeout: 180000 }
    );
    console.log('  Group replies from both bots detected.');

    console.log('13. Check Recents does not list bot/group sessions...');
    await page.getByTitle('Open all recents').first().click();
    await waitForLocator(page.getByText('Recents'));
    const recentsText = await page.locator('body').innerText();
    if (recentsText.includes(GROUP_NAME) || recentsText.includes('Echo Alpha Session')) {
      throw new Error('Recents incorrectly contains bot/group session');
    }
    console.log('  Recents purity check passed.');

    console.log('14. Screenshot...');
    await page.screenshot({ path: '/tmp/bot-e2e-result.png', fullPage: false });

    console.log('E2E PASSED');
  } catch (err) {
    console.error('E2E FAILED:', err.message);
    await page.screenshot({ path: '/tmp/bot-e2e-failure.png', fullPage: false });
    process.exitCode = 1;
  } finally {
    await app.close();
  }
})();
