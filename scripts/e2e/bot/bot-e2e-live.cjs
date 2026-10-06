// bot-e2e-live.cjs — bot-e2e-verification.cjs adapted for the LIVE cloud API.
// Differences vs the original:
//   - BASE_URL defaults to :3013 (vite dev), overridable via env.
//   - Clerk seed auto-login happens in-app (dev only); we wait for the token
//     in localStorage and attach it to API fetches (page.request has no session).
//   - Agent list is fetched inside the page context with the bearer header.
//
// PREREQUISITES:
//   - Vite dev server running on :3013 (pnpm --dir surfaces/ai.allternit.com dev)
//     with real Clerk keys and VITE_CLERK_SEED_EMAIL / VITE_CLERK_SEED_PASSWORD
//     set in .env.local (gitignored) so the app can seed auto-login in dev.
//   - Installed Google Chrome (chromium launches with channel:'chrome').
//   - Session reuse: if /tmp/botmode-e2e-state.json exists it is used as
//     Playwright storageState — Clerk rate-limits sign-in attempts per
//     account/IP, so do not delete it between runs. On success the Clerk JWT
//     is also saved (0600) to /tmp/botmode-clerk-token for paired local
//     tooling (e.g. a headless gizzi CLI chat turn against the live gateway)
//     and the storageState is refreshed.
//
// RUN: node bot-e2e-live.cjs   (from the repo root; any cwd works because all
// paths are absolute — see requireDep for module resolution in worktrees
// without node_modules.)
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

const BASE_URL = process.env.PLATFORM_URL || 'http://localhost:3013';
const STATE_PATH = process.env.E2E_STATE_PATH || '/tmp/botmode-e2e-state.json';
const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
// Self-seeded by this harness when missing — the echo bots that originally
// backed bot-e2e-verification.cjs live under the owner's account, so a fresh
// test account has nothing to drive. Names double as identity-check answers.
const BOT_SPECS = [
  {
    name: BOT_A,
    description: 'Bot-mode live e2e verification bot (alpha).',
    system_prompt:
      'You are Echo Alpha, a verification test bot. When asked your name or what you are called, reply with exactly: Echo Alpha. Keep all other replies short.',
  },
  {
    name: BOT_B,
    description: 'Bot-mode live e2e verification bot (beta).',
    system_prompt:
      'You are Echo Beta, a verification test bot. When asked your name or what you are called, reply with exactly: Echo Beta. Keep all other replies short.',
  },
];
const GROUP_NAME = 'AlphaBeta Verification';
const RUN_ID = `BOTMODE-${Date.now().toString(36)}`;
const SINGLE_PROMPT = `What is your name? ${RUN_ID}`;
const GROUP_PROMPT = `Say your names. ${RUN_ID}`;
const SCREENSHOT_PATH = '/tmp/bot-e2e-live-result.png';

const RESULT = {
  baseUrl: BASE_URL,
  seedSignIn: false,
  botIds: { alpha: null, beta: null },
  botHubVisible: false,
  singleBotFlow: false,
  groupChatFlow: false,
  railLayoutOk: false,
  consoleErrors: [],
  errors: [],
  screenshotPath: SCREENSHOT_PATH,
};

function log(...args) {
  console.log('[LIVE-VERIFY]', ...args);
}

async function waitForText(page, text, opts = {}) {
  const locator = page.getByText(text).first();
  await locator.waitFor({ timeout: 30000, ...opts });
  return locator;
}

async function waitForChatTranscriptContains(page, userMessage, opts, timeout = 180000) {
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

async function fetchAgentsAuthed(page) {
  return page.evaluate(async () => {
    const token = localStorage.getItem('allternit_token');
    if (!token) return { error: 'no allternit_token in localStorage' };
    const res = await fetch('/api/v1/agents', {
      headers: { Authorization: `Bearer ${token}` },
    });
    const body = await res.json().catch(() => ({}));
    return { status: res.status, ...body };
  });
}

(async () => {
  const fs = require('fs');
  // The repo-pinned Playwright version lacks the cached headless shell; use the
  // machine's installed Chrome instead of downloading another browser.
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  // Reuse a signed-in session across runs — Clerk rate-limits sign-in attempts
  // per account/IP, and every fresh context forces a new sign-in.
  const context = await browser.newContext({
    viewport: { width: 1280, height: 900 },
    ...(fs.existsSync(STATE_PATH) ? { storageState: STATE_PATH } : {}),
  });
  const page = await context.newPage();

  page.on('console', (msg) => {
    const type = msg.type();
    if (type === 'error' || type === 'warning') {
      RESULT.consoleErrors.push({ type, text: msg.text() });
    }
  });
  page.on('pageerror', (err) => {
    RESULT.consoleErrors.push({ type: 'pageerror', text: err.message });
  });

  try {
    // 1. Open the app and wait for the Clerk seed auto-login to produce a
    //    token the API accepts (setActive can lag token presence by a beat).
    log('Opening app, waiting for seed sign-in token...', BASE_URL);
    await page.goto(BASE_URL, { waitUntil: 'domcontentloaded', timeout: 60000 });
    let agentsData = null;
    const deadline = Date.now() + 120000;
    while (Date.now() < deadline) {
      const token = await page.evaluate(() => localStorage.getItem('allternit_token'));
      if (token) {
        const attempt = await fetchAgentsAuthed(page);
        if (attempt && attempt.status === 200 && attempt.agents) {
          agentsData = attempt;
          break;
        }
      }
      await page.waitForTimeout(2000);
    }
    if (!agentsData) {
      const token = await page.evaluate(() => !!localStorage.getItem('allternit_token'));
      throw new Error(`seed sign-in did not yield an API-valid token (token present: ${token})`);
    }
    RESULT.seedSignIn = true;
    log('  Seed sign-in token accepted by API.');
    // Save the token (0600) so paired local tooling (e.g. a headless gizzi
    // CLI chat turn against the live gateway) can reuse this session.
    const token = await page.evaluate(() => localStorage.getItem('allternit_token'));
    if (token) {
      fs.writeFileSync('/tmp/botmode-clerk-token', token, { mode: 0o600 });
    }
    await context.storageState({ path: STATE_PATH });

    // 2. Ensure the two echo bots exist for THIS user, then resolve their ids.
    //    The original harness assumed pre-seeded echo-bot-alpha/beta rows owned
    //    by whoever was signed in — true only for the owner's account.
    log('Ensuring verification bots exist for this user...');
    const findBot = (agents, name) => (agents || []).find((a) => a.name === name);
    const botPrimitives = (s) => ({
      is_bot: true,
      bot_profile: {
        displayName: s.name,
        handle: s.name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''),
        tagline: s.description,
        groupChatEnabled: true,
        botCategory: 'custom',
        lifecycle: 'active',
      },
    });
    let anyFlagged = false;
    for (const spec of BOT_SPECS) {
      const existing = findBot(agentsData.agents, spec.name);
      if (existing) {
        // Rows created before is_bot existed (or by other tooling) are
        // invisible to Bot Hub, which filters on isBot + botProfile.
        if (existing.is_bot !== true) {
          log(`  Flagging ${spec.name} as a bot via PUT /api/v1/agents/${existing.id}...`);
          anyFlagged = true;
          const patched = await page.evaluate(async ({ id, p }) => {
            const token = localStorage.getItem('allternit_token');
            const res = await fetch(`/api/v1/agents/${id}`, {
              method: 'PUT',
              headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
              body: JSON.stringify(p),
            });
            const body = await res.json().catch(() => ({}));
            return { status: res.status, body };
          }, { id: existing.id, p: botPrimitives(spec) });
          if (patched.status !== 200) {
            throw new Error(`Failed to flag ${spec.name} as bot: HTTP ${patched.status} ${JSON.stringify(patched.body).slice(0, 200)}`);
          }
        }
        continue;
      }
      log(`  Creating ${spec.name} via POST /api/v1/agents...`);
      const created = await page.evaluate(async (s) => {
        const token = localStorage.getItem('allternit_token');
        const p = {
          is_bot: true,
          bot_profile: {
            displayName: s.name,
            handle: s.name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''),
            tagline: s.description,
            groupChatEnabled: true,
            botCategory: 'custom',
            lifecycle: 'active',
          },
        };
        const res = await fetch('/api/v1/agents', {
          method: 'POST',
          headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
          body: JSON.stringify({
            name: s.name,
            description: s.description,
            type: 'worker',
            model: 'allternit/kimi-k3',
            provider: 'allternit',
            system_prompt: s.system_prompt,
            max_iterations: 10,
            temperature: 0.7,
            trust_tier: 'standard',
            harness_config: { mode: 'cloud' },
            enabled_modes: ['chat'],
            ...p,
          }),
        });
        const body = await res.json().catch(() => ({}));
        return { status: res.status, body };
      }, spec);
      if (created.status !== 200 && created.status !== 201) {
        throw new Error(`Failed to create ${spec.name}: HTTP ${created.status} ${JSON.stringify(created.body).slice(0, 200)}`);
      }
    }
    const refreshed = await fetchAgentsAuthed(page);
    if (!refreshed || refreshed.status !== 200 || !refreshed.agents) {
      throw new Error(`Could not re-fetch /api/v1/agents after seeding: ${JSON.stringify(refreshed).slice(0, 200)}`);
    }
    agentsData = refreshed;
    const alpha = findBot(agentsData.agents, BOT_A);
    const beta = findBot(agentsData.agents, BOT_B);
    if (!alpha || !beta) throw new Error(`Could not find ${BOT_A}/${BOT_B} in /api/v1/agents after seeding`);
    RESULT.botIds.alpha = alpha.id;
    RESULT.botIds.beta = beta.id;
    log(`  alpha=${alpha.id}, beta=${beta.id}`);

    if (anyFlagged) {
      // The app's agent store fetched before the PUTs above; a reload is the
      // only reliable way to make it re-list with the new bot flags.
      log('Reloading app so the agent store picks up the bot flags...');
      await page.reload({ waitUntil: 'domcontentloaded', timeout: 60000 });
      const reloadDeadline = Date.now() + 120000;
      while (Date.now() < reloadDeadline) {
        const token = await page.evaluate(() => localStorage.getItem('allternit_token'));
        if (token) {
          const attempt = await fetchAgentsAuthed(page);
          if (attempt && attempt.status === 200 && attempt.agents) break;
        }
        await page.waitForTimeout(2000);
      }
      await page.waitForTimeout(3000);
    }

    // 3. Navigate to Bot Hub. Prefer the rail nav button (present on every
    //    landing view); fall back to the Products discovery tile for older
    //    builds that land on the discovery grid.
    log('Opening Bot Hub...');
    const botHubNav = page.getByRole('button', { name: 'Bot Hub' }).first();
    try {
      await botHubNav.waitFor({ timeout: 20000 });
      await botHubNav.click();
    } catch {
      await page.getByText('Agent | Bot Hub').first().click();
    }
    await waitForText(page, 'Your bots');
    await waitForText(page, BOT_A);
    await waitForText(page, BOT_B);
    RESULT.botHubVisible = true;
    log('  Bot Hub shows Echo Alpha and Echo Beta.');

    // 4. Single-bot chat with Echo Alpha.
    log('Starting single-bot chat with Echo Alpha...');
    await page.getByText(BOT_A).first().click();
    await waitForText(page, 'Delegate work to Echo Alpha');
    // Playwright's role-based click can land on a no-op match (icon-only
    // rail buttons share the accessible name); click the visible text-exact
    // button in the DOM instead — verified to dispatch open-view.
    await page.evaluate(() => {
      const btns = Array.from(document.querySelectorAll('button')).filter(
        (b) => (b.textContent || '').trim() === 'Chat' && b.offsetParent !== null && !b.disabled,
      );
      if (!btns.length) throw new Error('no visible Chat button on bot home');
      btns[0].click();
    });

    const singleComposer = page.locator('textarea[placeholder^="Message "], textarea[placeholder*="Type your message"]').first();
    await singleComposer.waitFor({ timeout: 30000 });
    log('  Sending single-bot message...');
    await singleComposer.fill(SINGLE_PROMPT);
    await singleComposer.press('Enter');

    try {
      await waitForChatTranscriptContains(page, SINGLE_PROMPT, ['Echo Alpha'], 150000);
    } catch (err) {
      const transcriptText = await page.evaluate((prompt) => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === prompt
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? transcript.innerText : 'NO_TRANSCRIPT_FOUND';
      }, SINGLE_PROMPT);
      log('  Single-bot transcript debug:', transcriptText.slice(0, 500));
      throw err;
    }
    RESULT.singleBotFlow = true;
    log('  Single-bot identity check passed.');

    // 5. Group chat.
    log('Returning to Bot Hub for group chat...');
    const botHubNav2 = page.getByRole('button', { name: 'Bot Hub' }).first();
    try {
      await botHubNav2.waitFor({ timeout: 20000 });
      await botHubNav2.click();
    } catch {
      await page.getByText('Agent | Bot Hub').first().click();
    }
    await waitForText(page, 'Your bots');
    await page.getByRole('button', { name: 'New group chat' }).first().click();
    await waitForText(page, 'Start group chat');

    log('  Selecting bots and naming group...');
    await page.locator(`[data-group-bot-id="${RESULT.botIds.alpha}"]`).first().click();
    await page.locator(`[data-group-bot-id="${RESULT.botIds.beta}"]`).first().click();
    await page.locator('input[placeholder="e.g., Research Squad"]').first().fill(GROUP_NAME);
    await page.locator('.fixed.inset-0').getByRole('button', { name: 'Start chat' }).first().click();

    const groupComposer = page.locator('textarea[aria-label="Message the group"], textarea[placeholder^="Message #"]').first();
    await groupComposer.waitFor({ timeout: 30000 });
    log('  Sending group message...');
    await groupComposer.fill(GROUP_PROMPT);
    await groupComposer.press('Enter');

    try {
      await waitForChatTranscriptContains(page, GROUP_PROMPT, ['Echo Alpha', 'Echo Beta'], 240000);
    } catch (err) {
      const transcriptText = await page.evaluate((prompt) => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === prompt
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? transcript.innerText : 'NO_TRANSCRIPT_FOUND';
      }, GROUP_PROMPT);
      log('  Transcript debug:', transcriptText.slice(0, 500));
      throw err;
    }
    RESULT.groupChatFlow = true;
    log('  Group replies from both bots detected in chat transcript.');

    await page.waitForFunction(
      (prompt) => {
        const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
          (el) => el.textContent?.trim() === prompt
        );
        let transcript = userEl?.parentElement;
        while (transcript && !transcript.classList.contains('overflow-y-auto')) {
          transcript = transcript.parentElement;
        }
        return transcript ? !transcript.innerText.includes('Bots are thinking') : false;
      },
      GROUP_PROMPT,
      { timeout: 60000 }
    );

    // 6. Rail layout — Bots list + Group Chats row for this room.
    log('Verifying rail layout...');
    await page.getByText('Bots', { exact: true }).first().waitFor({ timeout: 10000 });
    await page.getByText('Group Chats').first().waitFor({ timeout: 10000 });

    const groupRail = page.locator('[data-rail-item^="group-"]').filter({ hasText: GROUP_NAME }).first();
    await groupRail.waitFor({ timeout: 10000 });
    const groupSessionId = await groupRail.getAttribute('data-rail-item');
    log(`  Group rail item: ${groupSessionId}`);
    const railText = await groupRail.innerText();
    if (!railText.includes(GROUP_NAME)) throw new Error(`Rail item missing group name: ${railText}`);

    RESULT.railLayoutOk = true;
    log('  Rail layout check passed.');

    log(`Taking screenshot: ${SCREENSHOT_PATH}`);
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: false });

    log('ALL VERIFICATION PASSED');
  } catch (err) {
    RESULT.errors.push(err.message);
    log('VERIFICATION FAILED:', err.message);
    try {
      await page.screenshot({ path: '/tmp/bot-e2e-live-failure.png', fullPage: false });
    } catch {}
  } finally {
    await browser.close();
  }

  console.log('\n=== VERIFICATION RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
  process.exit(RESULT.errors.length ? 1 : 0);
})();
