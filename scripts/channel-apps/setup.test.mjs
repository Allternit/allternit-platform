// node --test scripts/channel-apps/setup.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { run } from './setup.mjs';

const reply = (status, body) => ({ status, json: async () => body });

test('validates with fake vendors, never prints secrets, writes env 0600', async () => {
  const store = { 'telegram.bot_token': 'TGSECRET', 'telegram.webhook_secret': 'WHSECRET', 'teams.app_id': 'aid', 'teams.app_password': 'TEAMSSECRET' };
  const calls = [];
  const fetchFn = async (url, init = {}) => {
    calls.push([init.method ?? 'GET', String(url).replace(/bot[^/]+\//, 'bot<t>/')]);
    if (url.includes('getMe')) return reply(200, { ok: true, result: { username: 'allternit_bot' } });
    if (url.includes('setWebhook')) return reply(200, { ok: true });
    if (url.includes('login.microsoftonline.com')) return reply(200, { access_token: 'x' });
    throw new Error('unexpected ' + url);
  };
  const lines = []; let written;
  const results = await run({
    only: ['telegram', 'teams', 'slack'], apply: false, base: 'https://api.allternit.com', out: '/tmp/x.env',
    read: (c, f) => store[`${c}.${f}`] ?? null, fetchFn, write: (p, d, o) => { written = { p, d, o }; }, log: (l) => lines.push(l),
  });
  const printed = lines.join('\n');
  assert.ok(!/TGSECRET|WHSECRET|TEAMSSECRET/.test(printed), 'secrets must not be printed');
  assert.ok(!calls.some(([, u]) => u.includes('setWebhook')), 'dry run must not call setWebhook');
  assert.deepEqual(results.map((r) => [r.channel, r.ok, !!r.skipped]), [['telegram', true, false], ['slack', false, true], ['teams', true, false]]);
  assert.match(written.d, /ALLTERNIT_TEAMS_APP_PASSWORD=TEAMSSECRET/);
  assert.equal(written.o.mode, 0o600);
});

test('--apply registers the telegram webhook and a failure is reported', async () => {
  const fetchFn = async (url) => (url.includes('getMe') ? reply(200, { ok: true, result: { username: 'b' } }) : reply(400, { ok: false, description: 'bad url' }));
  const lines = [];
  const results = await run({ only: ['telegram'], apply: true, base: 'https://x', out: '/tmp/y.env', read: () => 's', fetchFn, write() {}, log: (l) => lines.push(l) });
  assert.equal(results[0].ok, false);
  assert.match(lines.join('\n'), /setWebhook failed \(bad url\)/);
});
