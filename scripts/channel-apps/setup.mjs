#!/usr/bin/env node
// Validate Allternit's own channel-app credentials and produce the env lines for
// cloud-api. No dependencies. Dry-run by default.
//
//   node scripts/channel-apps/setup.mjs                  validate only (read-only vendor calls)
//   node scripts/channel-apps/setup.mjs --apply          also register webhooks / endpoints
//   node scripts/channel-apps/setup.mjs --only slack,teams
//   node scripts/channel-apps/setup.mjs --out ./channel-apps.env
//   node scripts/channel-apps/setup.mjs --base https://api.allternit.com
//
// Credentials come from the macOS keychain, one generic password per field:
//   security add-generic-password -s com.allternit.<channel>.<field> -a allternit -w
// Secret values are never printed. The env snippet (mode 0600) is the only
// place they are written; scp it to /opt/allternit-cloud-api/ and merge into .env.
import { execFileSync } from 'node:child_process';
import { writeFileSync, chmodSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// channel -> field -> { env: canonical cloud-api env name, required }
export const CHANNELS = {
  telegram: {
    bot_token: { env: 'ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN', required: true },
    webhook_secret: { env: 'ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET', required: true },
  },
  discord: {
    app_id: { env: 'ALLTERNIT_DISCORD_APP_ID', required: true },
    public_key: { env: 'ALLTERNIT_DISCORD_PUBLIC_KEY', required: true },
    bot_token: { env: 'ALLTERNIT_DISCORD_BOT_TOKEN', required: true },
    client_secret: { env: 'ALLTERNIT_DISCORD_CLIENT_SECRET', required: true },
  },
  slack: {
    client_id: { env: 'ALLTERNIT_SLACK_CLIENT_ID', required: true },
    client_secret: { env: 'ALLTERNIT_SLACK_CLIENT_SECRET', required: true },
    signing_secret: { env: 'ALLTERNIT_SLACK_SIGNING_SECRET', required: true },
    bot_token: { env: null, required: false }, // validation only, from a test install; never an env line
  },
  teams: {
    app_id: { env: 'ALLTERNIT_TEAMS_APP_ID', required: true },
    app_password: { env: 'ALLTERNIT_TEAMS_APP_PASSWORD', required: true },
    tenant_id: { env: 'ALLTERNIT_TEAMS_TENANT_ID', required: false },
  },
  meta: {
    app_id: { env: 'ALLTERNIT_META_APP_ID', required: true },
    app_secret: { env: 'ALLTERNIT_META_APP_SECRET', required: true },
    es_config_id: { env: 'ALLTERNIT_META_ES_CONFIG_ID', required: true },
    system_token: { env: 'ALLTERNIT_META_SYSTEM_TOKEN', required: false },
  },
};

export const service = (channel, field) => `com.allternit.${channel}.${field}`;

export function keychainRead(channel, field) {
  try {
    const out = execFileSync('security', ['find-generic-password', '-s', service(channel, field), '-w'], {
      stdio: ['ignore', 'pipe', 'ignore'],
    });
    return out.toString().trim() || null;
  } catch {
    return null;
  }
}

async function json(fetchFn, url, init) {
  const res = await fetchFn(url, { ...init, signal: AbortSignal.timeout(15000) });
  let body = null;
  try { body = await res.json(); } catch { /* non-JSON body */ }
  return { status: res.status, body };
}

// Each validator returns { ok, detail, actions: [string] }. detail never contains secrets.
export const VALIDATORS = {
  async telegram(c, { fetchFn, apply, base }) {
    const api = (m) => `https://api.telegram.org/bot${c.bot_token}/${m}`;
    const me = await json(fetchFn, api('getMe'));
    if (!me.body?.ok) return { ok: false, detail: `getMe failed (${me.body?.description ?? me.status})` };
    const url = `${base}/channels/telegram-manager/${c.webhook_secret}`;
    const actions = [];
    if (apply) {
      const set = await json(fetchFn, api('setWebhook'), {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ url, secret_token: c.webhook_secret }),
      });
      if (!set.body?.ok) return { ok: false, detail: `setWebhook failed (${set.body?.description ?? set.status})` };
      actions.push('setWebhook done (webhook path embeds the secret, not shown)');
    } else {
      actions.push('would setWebhook to <cloud>/channels/telegram-manager/<secret> (use --apply)');
    }
    const warn = me.body.result?.can_manage_bots === false ? ' (Managed Bots not enabled on this bot: enable it in @BotFather)' : '';
    return { ok: true, detail: `bot @${me.body.result?.username}${warn}`, actions };
  },

  async discord(c, { fetchFn, apply, base }) {
    const headers = { authorization: `Bot ${c.bot_token}`, 'content-type': 'application/json' };
    const me = await json(fetchFn, 'https://discord.com/api/v10/applications/@me', { headers });
    if (me.status !== 200) return { ok: false, detail: `GET /applications/@me failed (${me.status})` };
    if (String(me.body.id) !== c.app_id) return { ok: false, detail: 'bot token belongs to a different application than app_id' };
    if (me.body.verify_key && me.body.verify_key !== c.public_key) return { ok: false, detail: 'public_key does not match the application verify_key' };
    const target = `${base}/channels/discord/interactions`;
    const actions = [];
    if (me.body.interactions_endpoint_url === target) {
      actions.push('interactions endpoint already set');
    } else if (apply) {
      const patch = await json(fetchFn, 'https://discord.com/api/v10/applications/@me', {
        method: 'PATCH', headers, body: JSON.stringify({ interactions_endpoint_url: target }),
      });
      if (patch.status !== 200) return { ok: false, detail: `setting interactions_endpoint_url failed (${patch.status}); cloud-api must already have the Discord env and be deployed so Discord's PING verifies` };
      actions.push('interactions_endpoint_url set');
    } else {
      actions.push(`would set interactions_endpoint_url to ${target} (use --apply; cloud-api needs the env first)`);
    }
    return { ok: true, detail: `application ${me.body.name ?? me.body.id}`, actions };
  },

  async slack(c, { fetchFn }) {
    if (!c.bot_token) {
      return { ok: true, detail: 'no bot_token in keychain; client_id/secret/signing_secret cannot be checked without a test install', actions: ['install the app to a test workspace, then store its xoxb- token as com.allternit.slack.bot_token to run auth.test'] };
    }
    const r = await json(fetchFn, 'https://slack.com/api/auth.test', { method: 'POST', headers: { authorization: `Bearer ${c.bot_token}` } });
    if (!r.body?.ok) return { ok: false, detail: `auth.test failed (${r.body?.error ?? r.status})` };
    return { ok: true, detail: `workspace ${r.body.team}, bot ${r.body.user}` };
  },

  async teams(c, { fetchFn }) {
    const tenant = c.tenant_id || 'botframework.com';
    const form = new URLSearchParams({
      grant_type: 'client_credentials', client_id: c.app_id, client_secret: c.app_password,
      scope: 'https://api.botframework.com/.default',
    });
    const r = await json(fetchFn, `https://login.microsoftonline.com/${tenant}/oauth2/v2.0/token`, {
      method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: form,
    });
    if (!r.body?.access_token) return { ok: false, detail: `token fetch failed (${r.body?.error ?? r.status}${r.body?.error_description ? ': ' + String(r.body.error_description).split('\r')[0] : ''})` };
    return { ok: true, detail: `token issued via ${c.tenant_id ? 'single-tenant ' + c.tenant_id : 'botframework.com (multi-tenant)'}` };
  },

  async meta(c, { fetchFn }) {
    const appToken = `${c.app_id}|${c.app_secret}`;
    const q = (input) => `https://graph.facebook.com/debug_token?input_token=${encodeURIComponent(input)}&access_token=${encodeURIComponent(appToken)}`;
    const app = await json(fetchFn, q(appToken));
    if (!app.body?.data?.is_valid) return { ok: false, detail: `app id/secret rejected (${app.body?.error?.message ?? app.status})` };
    let detail = `app ${app.body.data.application ?? c.app_id} valid`;
    if (c.system_token) {
      const sys = await json(fetchFn, q(c.system_token));
      if (!sys.body?.data?.is_valid) return { ok: false, detail: `system_token invalid (${sys.body?.error?.message ?? sys.body?.data?.error?.message ?? sys.status})` };
      detail += '; system token valid';
    }
    return { ok: true, detail };
  },
};

export async function run({ only, apply, base, out, read = keychainRead, fetchFn = fetch, write = writeFileSync, log = console.log }) {
  const results = [];
  const envLines = [];
  for (const [channel, fields] of Object.entries(CHANNELS)) {
    if (only && !only.includes(channel)) continue;
    const creds = {};
    const missing = [];
    for (const [field, spec] of Object.entries(fields)) {
      const value = read(channel, field);
      if (value) creds[field] = value;
      else if (spec.required) missing.push(service(channel, field));
    }
    if (missing.length) {
      log(`- ${channel}: SKIPPED, not in keychain: ${missing.join(', ')}`);
      results.push({ channel, ok: false, skipped: true });
      continue;
    }
    let res;
    try { res = await VALIDATORS[channel](creds, { fetchFn, apply, base }); }
    catch (error) { res = { ok: false, detail: `error: ${error.message}` }; }
    log(`- ${channel}: ${res.ok ? 'OK' : 'FAILED'}, ${res.detail}`);
    for (const action of res.actions ?? []) log(`    ${action}`);
    results.push({ channel, ok: res.ok });
    if (res.ok) {
      const names = [];
      for (const [field, spec] of Object.entries(fields)) {
        if (spec.env && creds[field]) { envLines.push(`${spec.env}=${creds[field]}`); names.push(spec.env); }
      }
      if (names.length) log(`    env to add on the server: ${names.join(', ')}`);
    }
  }
  if (envLines.length) {
    write(out, envLines.join('\n') + '\n', { mode: 0o600 });
    try { chmodSync(out, 0o600); } catch { /* injected writer in tests */ }
    log(`\nWrote ${envLines.length} env lines to ${out} (0600). Contains secrets: scp it to /opt/allternit-cloud-api/, merge into .env, restart cloud-api, then delete it.`);
  } else {
    log('\nNo channel passed validation; no env snippet written.');
  }
  if (!apply) log('Dry run: no vendor settings were changed. Re-run with --apply to register webhooks / endpoints.');
  return results;
}

function parseArgs(argv) {
  const opts = { apply: false, base: 'https://api.allternit.com', out: './channel-apps.env', only: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--apply') opts.apply = true;
    else if (a === '--only') opts.only = (argv[++i] ?? '').split(',').filter(Boolean);
    else if (a === '--out') opts.out = argv[++i];
    else if (a === '--base') opts.base = (argv[++i] ?? '').replace(/\/+$/, '');
    else { console.error(`unknown argument ${a}`); process.exit(2); }
  }
  return opts;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const results = await run(parseArgs(process.argv.slice(2)));
  process.exit(results.some((r) => !r.ok && !r.skipped) ? 1 : 0);
}
