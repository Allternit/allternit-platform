const { chromium } = require('@playwright/test');

const PLATFORM_URL = 'http://localhost:3014';
const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
const BOT_A_ID = 'baf15c34-e675-4a75-af04-0632fd545985';
const BOT_B_ID = '1321cfe4-1c50-47c9-84c5-88018faaa7c4';
const GROUP_NAME = 'AlphaBeta Squad';

async function waitFor(loc, opts = {}) {
  await loc.waitFor({ timeout: 30000, ...opts });
}

(async () => {
  const browser = await chromium.launch({
    channel: 'chrome',
    headless: false,
    args: ['--window-size=1440,900'],
  });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();

  page.on('console', (msg) => console.log('[Renderer]', msg.type(), msg.text()));
  page.on('pageerror', (err) => console.log('[PageError]', err.message));

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
    console.log('1. Navigate to platform...');
    await page.goto(PLATFORM_URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    await page.waitForTimeout(3000);
    try {
      await waitFor(page.getByText('Agent | Bot Hub'));
    } catch (e) {
      await dumpBody('initial');
      throw e;
    }

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
      (name) => document.body.innerText.includes(name),
      BOT_A,
      { timeout: 120000 }
    );
    const singleBody = await page.locator('body').innerText();
    if (!singleBody.includes(BOT_A)) throw new Error('Single bot reply missing identity');
    console.log('  Single-bot identity check passed.');

    console.log('7. Back to Bot Hub for group chat...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitFor(page.getByRole('button', { name: 'New group chat' }));

    console.log('8. Start group chat...');
    await page.getByRole('button', { name: 'New group chat' }).first().click();
    await waitFor(page.getByText('Start group chat'));

    console.log('9. Select bots and name group...');
    const modal = page.locator('[data-group-bot-id]').first().locator('..').locator('..');
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
      (a, b) => document.body.innerText.includes(a) && document.body.innerText.includes(b),
      BOT_A, BOT_B,
      { timeout: 180000 }
    );
    console.log('  Group replies from both bots detected.');

    console.log('13. Check Recents does not list bot/group sessions...');
    await page.getByTitle('Open all recents').first().click();
    await waitFor(page.getByText('Recents'));
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
    await dumpBody('failure');
    await page.screenshot({ path: '/tmp/bot-e2e-failure.png', fullPage: false });
    process.exitCode = 1;
  } finally {
    await browser.close();
  }
})();
