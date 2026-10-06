const { chromium } = require('@playwright/test');

const BASE_URL = 'http://localhost:3014';
const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
const GROUP_NAME = 'AlphaBeta Verification';
const SCREENSHOT_PATH = '/tmp/allternit-group-chat.png';

const RESULT = {
  botIds: {
    alpha: null,
    beta: null,
  },
  botHubVisible: false,
  singleBotFlow: false,
  groupChatFlow: false,
  railLayoutOk: false,
  consoleErrors: [],
  errors: [],
  screenshotPath: SCREENSHOT_PATH,
};

function log(...args) {
  console.log('[VERIFY]', ...args);
}

async function waitForText(page, text, opts = {}) {
  const locator = page.getByText(text).first();
  await locator.waitFor({ timeout: 30000, ...opts });
  return locator;
}

async function waitForBodyContains(page, opts, timeout = 120000) {
  await page.waitForFunction(
    (options) => {
      const text = document.body.innerText;
      return options.every((o) => text.includes(o));
    },
    opts,
    { timeout }
  );
}

async function waitForChatTranscriptContains(page, userMessage, opts, timeout = 180000) {
  // Scope to the main chat transcript by finding the scrollable ancestor of the user message.
  await page.waitForFunction(
    ({ userMsg, options }) => {
      const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
        (el) => el.textContent?.trim() === userMsg
      );
      if (!userEl) return false;
      let transcript = userEl.parentElement;
      while (transcript && !transcript.classList.contains('overflow-y-auto')) {
        transcript = transcript.parentElement;
      }
      if (!transcript) return false;
      const text = transcript.innerText;
      return options.every((o) => text.includes(o));
    },
    { userMsg: userMessage, options: opts },
    { timeout }
  );
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();

  page.on('console', (msg) => {
    const text = msg.text();
    const type = msg.type();
    if (type === 'error' || type === 'warning') {
      RESULT.consoleErrors.push({ type, text });
    }
  });
  page.on('pageerror', (err) => {
    RESULT.consoleErrors.push({ type: 'pageerror', text: err.message });
  });

  try {
    // 1. Fetch bot IDs from the API for reporting.
    log('Fetching packaged bot IDs from API...');
    const agentsRes = await page.request.get(`${BASE_URL}/api/v1/agents`);
    const agentsData = await agentsRes.json();
    const alpha = agentsData.agents.find((a) => a.name === 'echo-bot-alpha');
    const beta = agentsData.agents.find((a) => a.name === 'echo-bot-beta');
    if (!alpha || !beta) throw new Error('Could not find echo-bot-alpha/beta in /api/v1/agents');
    RESULT.botIds.alpha = alpha.id;
    RESULT.botIds.beta = beta.id;
    log(`  alpha=${alpha.id}, beta=${beta.id}`);

    // 2. Open the web UI.
    log('Opening web UI...');
    await page.goto(BASE_URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    await page.waitForTimeout(2000);

    // 3. Navigate to Bot Hub.
    log('Opening Bot Hub...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitForText(page, 'Your bots');
    await waitForText(page, BOT_A);
    await waitForText(page, BOT_B);
    RESULT.botHubVisible = true;
    log('  Bot Hub shows Echo Alpha and Echo Beta.');

    // 4. Start single-bot chat with Echo Alpha.
    log('Starting single-bot chat with Echo Alpha...');
    await page.getByText(BOT_A).first().click();
    await waitForText(page, 'Delegate work to Echo Alpha');
    await page.getByRole('button', { name: 'Chat' }).first().click();

    const singleComposer = page.locator('textarea[placeholder*="Type your message"]').first();
    await singleComposer.waitFor({ timeout: 30000 });
    log('  Sending single-bot message...');
    await singleComposer.fill('What is your name?');
    await singleComposer.press('Enter');

    try {
      await waitForChatTranscriptContains(page, 'What is your name?', ['Echo Alpha'], 120000);
    } catch (err) {
      const transcriptText = await page.evaluate(() => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === 'What is your name?'
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? transcript.innerText : 'NO_TRANSCRIPT_FOUND';
      });
      log('  Single-bot transcript debug:', transcriptText.slice(0, 500));
      throw err;
    }
    RESULT.singleBotFlow = true;
    log('  Single-bot identity check passed.');

    // 5. Back to Bot Hub for group chat.
    log('Returning to Bot Hub for group chat...');
    await page.getByText('Agent | Bot Hub').first().click();
    await waitForText(page, 'Your bots');
    await page.getByRole('button', { name: 'New group chat' }).first().click();
    await waitForText(page, 'Start group chat');

    log('  Selecting bots and naming group...');
    await page.locator(`[data-group-bot-id="${RESULT.botIds.alpha}"]`).first().click();
    await page.locator(`[data-group-bot-id="${RESULT.botIds.beta}"]`).first().click();
    // The modal input is the only one with the group-name placeholder.
    await page.locator('input[placeholder="e.g., Research Squad"]').first().fill(GROUP_NAME);
    // Use the modal's Start chat button (enabled after two bots selected).
    await page.locator('.fixed.inset-0').getByRole('button', { name: 'Start chat' }).first().click();

    const groupComposer = page.locator('textarea[aria-label="Message the group"]').first();
    await groupComposer.waitFor({ timeout: 30000 });
    log('  Sending group message...');
    await groupComposer.fill('Say your names.');
    await groupComposer.press('Enter');

    try {
      await waitForChatTranscriptContains(page, 'Say your names.', ['Echo Alpha', 'Echo Beta'], 180000);
    } catch (err) {
      const transcriptText = await page.evaluate(() => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === 'Say your names.'
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? transcript.innerText : 'NO_TRANSCRIPT_FOUND';
      });
      log('  Transcript debug:', transcriptText.slice(0, 500));
      throw err;
    }
    RESULT.groupChatFlow = true;
    log('  Group replies from both bots detected in chat transcript.');

    // Wait for the streaming indicator to disappear so the screenshot shows replies.
    await page.waitForFunction(
      () => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === 'Say your names.'
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? !transcript.innerText.includes('Bots are thinking') : false;
      },
      { timeout: 60000 }
    );

    // 6. Verify rail layout: group chat entry under Bots, name on top, avatars under.
    log('Verifying rail layout...');
    const groupSessionId = await page.evaluate(() => {
      // The group session view or rail item exposes the session id via data attributes.
      const railItem = document.querySelector('[data-rail-item^="group-"]');
      if (railItem) return railItem.getAttribute('data-rail-item');
      return null;
    });
    log(`  Group rail item: ${groupSessionId}`);

    // Ensure the rail shows the Bots section (not Recents) with the group.
    const botsSection = page.locator('text=Bots').first();
    await botsSection.waitFor({ timeout: 10000 });

    // The group chat rail item should show the group name and member count.
    const groupRail = page.locator(`[data-rail-item="${groupSessionId}"]`).first();
    await groupRail.waitFor({ timeout: 10000 });
    const railText = await groupRail.innerText();
    if (!railText.includes(GROUP_NAME)) {
      throw new Error(`Rail item does not show group name: ${railText}`);
    }
    if (!railText.includes('2 members')) {
      throw new Error(`Rail item does not show member count: ${railText}`);
    }

    // Verify the group rail item is inside the Bots/Groups section, not Recents.
    const groupsSectionCheck = await groupRail.evaluate((el) => {
      // Walk up until we find the Groups section container.
      let container = el.parentElement;
      while (container && !container.classList.contains('border-t')) {
        container = container.parentElement;
      }
      if (!container) return { hasGroupsLabel: false };
      const hasGroupsLabel = Array.from(container.querySelectorAll('div')).some(
        (d) => d.textContent?.trim() === 'Groups'
      );
      return { hasGroupsLabel };
    });
    if (!groupsSectionCheck || !groupsSectionCheck.hasGroupsLabel) {
      throw new Error(`Group rail item is not under the Bots > Groups section: ${JSON.stringify(groupsSectionCheck)}`);
    }

    // Verify multiple avatars are rendered in the rail item.
    const avatarCount = await groupRail.locator('.rounded-full.border').count();
    if (avatarCount < 2) {
      throw new Error(`Expected at least 2 avatars in rail item, found ${avatarCount}`);
    }

    RESULT.railLayoutOk = true;
    log('  Rail layout check passed (group under Bots, name + avatars).');

    // 7. Screenshot.
    log(`Taking screenshot: ${SCREENSHOT_PATH}`);
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: false });

    log('ALL VERIFICATION PASSED');
  } catch (err) {
    RESULT.errors.push(err.message);
    log('VERIFICATION FAILED:', err.message);
    try {
      await page.screenshot({ path: '/tmp/allternit-group-chat-failure.png', fullPage: false });
    } catch {}
  } finally {
    await browser.close();
  }

  console.log('\n=== VERIFICATION RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
})();
