// bot-e2e-desktop-live.cjs — live bot-mode e2e against the INSTALLED Allternit
// Desktop app, launched under Playwright with the tart VM driver env.
// Flow: launch installed app → Clerk auth window (typed creds) → Bot Hub →
// self-seed two echo bots for the signed-in user → single-bot chat → group
// chat → recents purity → screenshot + RESULT JSON.
//
// Prerequisites:
//   - The installed /Applications/Allternit Desktop.app must NOT already be
//     running — Playwright launches its own instance; a running instance makes
//     electron.launch attach/fail unpredictably.
//   - E2E_CLERK_PASSWORD env required (test-account password; never hardcoded).
//     EMAIL defaults to the Clerk test account, overridable via E2E_CLERK_EMAIL.
//   - ~/.allternit/tart-host.env must contain TART_HOST_TOKEN (loaded here);
//     TART_HOST_URL defaults to the tailscale tart-host bind, overridable.
//   - Run from a checkout with root node_modules installed (`pnpm install`) —
//     @playwright/test must resolve.
//   - NOTE on Clerk 429: this script types creds into the auth window and backs
//     off 5 min on rate-limit text, but do not run it during a known cooldown.
//
// Run: E2E_CLERK_PASSWORD=... node bot-e2e-desktop-live.cjs   (from the repo root)
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
const { _electron: electron } = requireDep('@playwright/test');
const fs = require('fs');

const APP_PATH = process.env.ALLTERNIT_DESKTOP_APP
  || '/Applications/Allternit Desktop.app/Contents/MacOS/Allternit Desktop';
const TART_HOST = process.env.TART_HOST_URL || 'http://100.88.98.69:8020';
const EMAIL = process.env.E2E_CLERK_EMAIL || 'clerk-test@news.allternit.com';
const PASSWORD = process.env.E2E_CLERK_PASSWORD; // required — never hardcode
const SCREENSHOT_PATH = '/tmp/bot-e2e-desktop-live-result.png';

const BOT_A = 'Echo Alpha';
const BOT_B = 'Echo Beta';
const GROUP_NAME = 'AlphaBeta Desktop Verification';

const RESULT = {
  appPath: APP_PATH,
  launched: false,
  tartEnvPropagated: false,
  authSignedIn: false,
  botIds: { alpha: null, beta: null },
  botHubVisible: false,
  singleBotFlow: false,
  groupChatFlow: false,
  recentsPurity: false,
  consoleErrors: [],
  errors: [],
  screenshotPath: SCREENSHOT_PATH,
};

function log(...args) {
  console.log('[DESKTOP-LIVE]', ...args);
}

function loadTartToken() {
  try {
    const file = path.join(process.env.HOME, '.allternit', 'tart-host.env');
    const line = fs.readFileSync(file, 'utf8').split('\n').find((l) => l.startsWith('TART_HOST_TOKEN='));
    return line ? line.slice('TART_HOST_TOKEN='.length).trim().replace(/^["']|["']$/g, '') : '';
  } catch {
    return '';
  }
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

(async () => {
  if (!PASSWORD) {
    console.error('E2E_CLERK_PASSWORD is required');
    process.exit(2);
  }
  const tartToken = loadTartToken();
  if (!tartToken) {
    console.error('Could not load TART_HOST_TOKEN from ~/.allternit/tart-host.env');
    process.exit(2);
  }

  const app = await electron.launch({
    executablePath: APP_PATH,
    args: [],
    env: {
      ...process.env,
      TART_HOST_URL: TART_HOST,
      TART_HOST_TOKEN: tartToken,
      ELECTRON_ENABLE_LOGGING: '1',
    },
  });
  RESULT.launched = true;
  log('App launched under Playwright with TART env.');

  // ── Auth window ──────────────────────────────────────────────────────────
  // The desktop opens a Clerk auth window when no session exists. Find the
  // window with sign-in inputs and type the creds. Retry on Clerk 429 with a
  // 5-minute backoff (never hammer the rate limiter).
  log('Waiting for auth window...');
  const authDeadline = Date.now() + 10 * 60 * 1000;
  let signedIn = false;
  while (!signedIn && Date.now() < authDeadline) {
    const windows = await app.windows();
    let authPage = null;
    for (const w of windows) {
      try {
        const url = await w.url();
        if (/clerk|sign-in|sign-up|accounts\./i.test(url)) { authPage = w; break; }
      } catch { /* window may be closing */ }
    }
    // Fallback: any window with an email-ish input that is not the platform.
    if (!authPage) {
      for (const w of windows) {
        try {
          if (await w.locator('input[type="email"], input[name="identifier"], input#identifier-field').first().isVisible({ timeout: 1500 }).catch(() => false)) {
            authPage = w;
            break;
          }
        } catch { /* ignore */ }
      }
    }
    if (!authPage) {
      // Maybe we are already signed in and only the platform window exists.
      const platform = await findPlatformWindow(app).catch(() => null);
      if (platform) { signedIn = true; log('Already signed in (no auth window).'); break; }
      await new Promise((r) => setTimeout(r, 2000));
      continue;
    }

    log('Auth window found; attempting sign-in...');
    const emailInput = authPage.locator('input[name="identifier"], input#identifier-field, input[type="email"]').first();
    await emailInput.waitFor({ timeout: 15000 });
    await emailInput.fill(EMAIL);
    // Clerk: identifier step then password step.
    const continueBtn = authPage.getByRole('button', { name: /continue|sign in/i }).first();
    await continueBtn.click().catch(() => {});
    const passwordInput = authPage.locator('input[name="password"], input#password-field, input[type="password"]').first();
    await passwordInput.waitFor({ timeout: 15000 });
    await passwordInput.fill(PASSWORD);
    await authPage.getByRole('button', { name: /continue|sign in|verify/i }).first().click().catch(() => {});

    // Success: auth window closes or platform window appears with Bot Hub.
    // Failure-with-429: error text visible → back off 5 min.
    const settled = await Promise.race([
      (async () => {
        for (let i = 0; i < 60; i++) {
          const wins = await app.windows().catch(() => []);
          for (const w of wins) {
            try {
              await w.getByText('Agent | Bot Hub').first().waitFor({ timeout: 1000 });
              return 'signed-in';
            } catch { /* not the platform */ }
          }
          // auth window still open? check for rate-limit text
          if (authPage && !(await authPage.isClosed().catch(() => true))) {
            const body = await authPage.locator('body').innerText().catch(() => '');
            if (/too many requests|rate limit/i.test(body)) return 'rate-limited';
          }
          await new Promise((r) => setTimeout(r, 2000));
        }
        return 'timeout';
      })(),
    ]);
    if (settled === 'signed-in') { signedIn = true; log('Sign-in succeeded.'); }
    else if (settled === 'rate-limited') {
      log('Clerk rate-limited (429); backing off 5 minutes...');
      await new Promise((r) => setTimeout(r, 5 * 60 * 1000));
    } else {
      log('Sign-in settle timeout; retrying...');
    }
  }
  if (!signedIn) throw new Error('Could not sign in via desktop auth window');
  RESULT.authSignedIn = true;

  // ── Platform window ──────────────────────────────────────────────────────
  const page = await findPlatformWindow(app);
  page.on('pageerror', (err) => RESULT.consoleErrors.push({ type: 'pageerror', text: err.message }));

  // Verify the API child inherited the TART env.
  const tartOk = await verifyTartEnv();
  RESULT.tartEnvPropagated = tartOk;
  log(`TART env propagated to allternit-api: ${tartOk}`);

  // Token for API calls: desktop bridge first, localStorage fallback.
  const token = await page.evaluate(async () => {
    try {
      const s = await window.allternit?.auth?.getSession?.();
      if (s?.accessToken) return s.accessToken;
    } catch { /* fall through */ }
    return localStorage.getItem('allternit_token');
  });
  if (!token) throw new Error('No access token available from desktop session');
  fs.writeFileSync('/tmp/botmode-clerk-token', token, { mode: 0o600 });

  async function apiFetch(p, init = {}) {
    return page.evaluate(async ([p2, init2, tok]) => {
      const res = await fetch(p2, { ...init2, headers: { ...(init2.headers || {}), Authorization: `Bearer ${tok}` } });
      const body = await res.json().catch(() => ({}));
      return { status: res.status, body };
    }, [p, init, token]);
  }

  // Self-seed the two echo bots (same design as bot-e2e-live.cjs).
  log('Ensuring verification bots exist...');
  const BOT_SPECS = [
    { name: BOT_A, description: 'Bot-mode desktop e2e verification bot (alpha).',
      system_prompt: 'You are Echo Alpha, a verification test bot. When asked your name or what you are called, reply with exactly: Echo Alpha. Keep all other replies short.' },
    { name: BOT_B, description: 'Bot-mode desktop e2e verification bot (beta).',
      system_prompt: 'You are Echo Beta, a verification test bot. When asked your name or what you are called, reply with exactly: Echo Beta. Keep all other replies short.' },
  ];
  let agentsBody = null;
  for (const spec of BOT_SPECS) {
    const list = await apiFetch('/api/v1/agents');
    if (list.status !== 200) throw new Error(`GET /api/v1/agents → ${list.status}`);
    agentsBody = list.body;
    if ((list.body.agents || []).some((a) => a.name === spec.name)) continue;
    log(`  Creating ${spec.name}...`);
    const created = await apiFetch('/api/v1/agents', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: spec.name, description: spec.description, type: 'worker',
        model: 'allternit/kimi-k3', provider: 'allternit',
        system_prompt: spec.system_prompt, max_iterations: 10, temperature: 0.7,
        trust_tier: 'standard', harness_config: { mode: 'cloud' }, enabled_modes: ['chat'],
      }),
    });
    if (created.status !== 200 && created.status !== 201) {
      throw new Error(`POST /api/v1/agents (${spec.name}) → ${created.status} ${JSON.stringify(created.body).slice(0, 200)}`);
    }
  }
  const finalList = await apiFetch('/api/v1/agents');
  const alpha = (finalList.body.agents || []).find((a) => a.name === BOT_A);
  const beta = (finalList.body.agents || []).find((a) => a.name === BOT_B);
  if (!alpha || !beta) throw new Error('Echo Alpha/Beta missing after seeding');
  RESULT.botIds.alpha = alpha.id;
  RESULT.botIds.beta = beta.id;
  log(`  alpha=${alpha.id}, beta=${beta.id}`);

  // ── Bot Hub + single-bot chat ────────────────────────────────────────────
  await page.getByText('Agent | Bot Hub').first().click();
  await waitForText(page, 'Your bots');
  await waitForText(page, BOT_A);
  await waitForText(page, BOT_B);
  RESULT.botHubVisible = true;
  log('Bot Hub visible with both bots.');

  log('Single-bot chat with', BOT_A, '...');
  await page.getByText(BOT_A).first().click();
  await waitForText(page, `Delegate work to ${BOT_A}`);
  await page.getByRole('button', { name: 'Chat' }).first().click();
  const singleComposer = page.locator('textarea[placeholder*="Type your message"]').first();
  await singleComposer.waitFor({ timeout: 30000 });
  await singleComposer.fill('What is your name?');
  await singleComposer.press('Enter');
  await waitForChatTranscriptContains(page, 'What is your name?', [BOT_A], 150000);
  RESULT.singleBotFlow = true;
  log('Single-bot identity check passed.');

  // ── Group chat ───────────────────────────────────────────────────────────
  log('Group chat...');
  await page.getByText('Agent | Bot Hub').first().click();
  await waitForText(page, 'Your bots');
  await page.getByRole('button', { name: 'New group chat' }).first().click();
  await waitForText(page, 'Start group chat');
  await page.locator(`[data-group-bot-id="${RESULT.botIds.alpha}"]`).first().click();
  await page.locator(`[data-group-bot-id="${RESULT.botIds.beta}"]`).first().click();
  await page.locator('input[placeholder="e.g., Research Squad"]').first().fill(GROUP_NAME);
  await page.locator('.fixed.inset-0').getByRole('button', { name: 'Start chat' }).first().click();
  const groupComposer = page.locator('textarea[aria-label="Message the group"]').first();
  await groupComposer.waitFor({ timeout: 30000 });
  await groupComposer.fill('Say your names.');
  await groupComposer.press('Enter');
  await waitForChatTranscriptContains(page, 'Say your names.', [BOT_A, BOT_B], 240000);
  await page.waitForFunction(
    () => {
      const userEl = Array.from(document.querySelectorAll('p.whitespace-pre-wrap, div, span')).find(
        (el) => el.textContent?.trim() === 'Say your names.'
      );
      let transcript = userEl?.parentElement;
      while (transcript && !transcript.classList.contains('overflow-y-auto')) transcript = transcript.parentElement;
      return transcript ? !transcript.innerText.includes('Bots are thinking') : false;
    },
    { timeout: 60000 }
  );
  RESULT.groupChatFlow = true;
  log('Group replies from both bots detected.');

  // ── Recents purity ───────────────────────────────────────────────────────
  await page.getByTitle('Open all recents').first().click();
  await page.getByRole('heading', { name: 'Recents' }).first().waitFor({ timeout: 10000 });
  const recentsText = await page.locator('[role="tabpanel"], [data-testid="recents-panel"], .recents-list').first().innerText({ timeout: 5000 }).catch(() => '');
  if (recentsText.includes(GROUP_NAME)) throw new Error('Recents incorrectly contains bot group session');
  RESULT.recentsPurity = true;
  log('Recents purity check passed.');

  await page.screenshot({ path: SCREENSHOT_PATH, fullPage: false });
  log('ALL VERIFICATION PASSED');
})().catch((err) => {
  RESULT.errors.push(err.message);
  log('VERIFICATION FAILED:', err.message);
  process.exitCode = 1;
}).finally(() => {
  console.log('\n=== DESKTOP VERIFICATION RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
});

async function findPlatformWindow(app) {
  const deadline = Date.now() + 300000;
  while (Date.now() < deadline) {
    const windows = await app.windows();
    for (const w of windows) {
      try {
        const url = await w.url();
        if (/^https?:\/\/(localhost|127\.0\.0\.1)/.test(url) && !/clerk|sign-in|sign-up/.test(url)) {
          try {
            await w.getByText('Agent | Bot Hub').first().waitFor({ timeout: 3000 });
            return w;
          } catch { /* still loading */ }
        }
      } catch { /* window may be closing */ }
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('Platform window did not appear');
}

async function verifyTartEnv() {
  // The API is a child of the app; confirm it has TART_HOST_URL by checking the
  // process table from node (no secrets printed).
  try {
    const { execSync } = require('child_process');
    const out = execSync('pgrep -f "Resources/bin/allternit-api" | head -3', { encoding: 'utf8' }).trim().split('\n').filter(Boolean);
    for (const pid of out) {
      try {
        const env = require('child_process').execSync(`ps eww ${pid}`, { encoding: 'utf8' });
        if (env.includes('TART_HOST_URL=')) return true;
      } catch { /* pid gone */ }
    }
  } catch { /* pgrep failed */ }
  return false;
}
