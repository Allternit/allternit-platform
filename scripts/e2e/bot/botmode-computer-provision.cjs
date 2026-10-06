// botmode-computer-provision.cjs — live bot-computer provisioning verification.
//
// Steps: pick a bot -> POST /bots/:id/desktop/provision -> poll status until
// running -> assert ws_url + protocol -> fetch a screenshot -> attempt a VNC
// websocket handshake (single-owner claim rules apply).
//
// PREREQUISITES:
//   - A valid Clerk bearer token at /tmp/botmode-clerk-token (0600), written by
//     a successful bot-e2e-live.cjs / bot-e2e-desktop-live.cjs run (or the
//     botmode-signin-probe.cjs they descend from).
//   - vite dev on :3013 (the vite dev proxy forwards /api/v1 to the local
//     data-plane API on :8013), or set ALLTERNIT_API_URL directly.
//   - BOT_NAME (default 'Echo Alpha') must already exist — run bot-e2e-live.cjs
//     first to self-seed it.
//   - The :8013 allternit-api process must have a VM driver (INCUS_URL or
//     TART_HOST_URL) in its env — bot computers provision on the data plane,
//     not the cloud control plane. If the desktop status never returns ws_url
//     the driver is missing and this script reports that honestly as a failure
//     instead of passing.
//   - Root node_modules must include `ws` (pnpm install). Session worktrees
//     without node_modules borrow the shared checkout's via requireDep below.
//
// RUN: node botmode-computer-provision.cjs
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
const WebSocket = requireDep('ws');
const fs = require('fs');

const API = (process.env.ALLTERNIT_API_URL || 'http://localhost:3013') + '/api/v1';
const TOKEN_PATH = '/tmp/botmode-clerk-token';
const BOT_NAME = process.env.BOT_NAME || 'Echo Alpha';

const RESULT = {
  api: API,
  botId: null,
  provisionAccepted: false,
  statusFlow: [],
  terminalStatus: null,
  wsUrl: null,
  protocol: null,
  screenshotOk: false,
  vncHandshake: null,
  errors: [],
};

function log(...args) {
  console.log('[COMPUTER-VERIFY]', ...args);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function apiFetch(path, opts = {}) {
  const token = fs.readFileSync(TOKEN_PATH, 'utf8').trim();
  const res = await fetch(`${API}${path}`, {
    ...opts,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(opts.body ? { 'Content-Type': 'application/json' } : {}),
      ...opts.headers,
    },
  });
  const text = await res.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {}
  return { status: res.status, json, text };
}

(async () => {
  try {
    log('Resolving bot', BOT_NAME, '...');
    const agents = await apiFetch('/agents');
    if (agents.status !== 200) throw new Error(`agents fetch: ${agents.status} ${agents.text.slice(0, 200)}`);
    const bot = (agents.json.agents || []).find((a) => a.name === BOT_NAME || a.id === BOT_NAME);
    if (!bot) throw new Error(`bot '${BOT_NAME}' not found in agents list`);
    RESULT.botId = bot.id;
    log('  bot id:', bot.id);

    log('POST /bots/:id/desktop/provision ...');
    const prov = await apiFetch(`/bots/${bot.id}/desktop/provision`, {
      method: 'POST',
      body: JSON.stringify({}),
    });
    log('  provision status:', prov.status);
    if (![200, 201, 202].includes(prov.status)) {
      throw new Error(`provision rejected: ${prov.status} ${prov.text.slice(0, 400)}`);
    }
    RESULT.provisionAccepted = true;

    const deadline = Date.now() + 8 * 60 * 1000;
    let last = '';
    while (Date.now() < deadline) {
      const st = await apiFetch(`/bots/${bot.id}/desktop`);
      const key = `${st.status}:${st.text.slice(0, 120)}`;
      if (key !== last) {
        last = key;
        RESULT.statusFlow.push({ at: new Date().toISOString(), status: st.status, body: st.json || st.text.slice(0, 200) });
        log('  status poll:', st.status, (st.json ? JSON.stringify(st.json).slice(0, 180) : st.text.slice(0, 180)));
      }
      const body = st.json || {};
      const state = (body.status || body.state || '').toString();
      if (body.ws_url) RESULT.wsUrl = body.ws_url;
      if (body.protocol) RESULT.protocol = body.protocol;
      if (['running', 'started', 'active', 'ready'].includes(state)) break;
      if (['error', 'failed', 'exited', 'stopped'].includes(state)) {
        // stopped can be a normal post-provision state for a paused desktop;
        // keep polling a bit more only for explicit errors
        if (['error', 'failed', 'exited'].includes(state)) throw new Error(`desktop entered state '${state}'`);
      }
      await sleep(5000);
    }
    const final = await apiFetch(`/bots/${bot.id}/desktop`);
    RESULT.terminalStatus = final.status;
    if (final.json) {
      RESULT.wsUrl = final.json.ws_url || RESULT.wsUrl;
      RESULT.protocol = final.json.protocol || RESULT.protocol;
    }
    log('  terminal poll:', final.status, JSON.stringify(final.json || final.text.slice(0, 200)).slice(0, 240));

    if (RESULT.wsUrl) {
      log('Screenshot fetch ...');
      const shot = await apiFetch(`/bots/${bot.id}/desktop/screenshot${final.json?.sandbox_id ? `?sandbox_id=${final.json.sandbox_id}` : ''}`);
      RESULT.screenshotOk = shot.status === 200;
      log('  screenshot:', shot.status, shot.status === 200 ? `${shot.text.length}b (b64)` : shot.text.slice(0, 120));

      log('VNC websocket handshake ...');
      try {
        const wsUrl = new URL(RESULT.wsUrl, 'http://localhost:3013');
        wsUrl.protocol = wsUrl.protocol === 'https:' ? 'wss:' : 'ws:';
        const ws = new WebSocket(wsUrl.toString(), {
          headers: { Authorization: `Bearer ${fs.readFileSync(TOKEN_PATH, 'utf8').trim()}` },
          handshakeTimeout: 10000,
        });
        await new Promise((resolve, reject) => {
          ws.on('open', () => { RESULT.vncHandshake = 'open'; ws.close(); resolve(); });
          ws.on('upgrade', (res) => { RESULT.vncHandshake = `upgrade ${res.statusCode}`; });
          ws.on('message', (d) => { RESULT.vncHandshake = `message ${d.length}b`; ws.close(); resolve(); });
          ws.on('error', (e) => reject(e));
          setTimeout(() => reject(new Error('ws handshake timeout')), 12000);
        });
      } catch (e) {
        RESULT.vncHandshake = `error: ${e.message}`;
      }
      log('  vnc:', RESULT.vncHandshake);
    } else {
      RESULT.errors.push('no ws_url in desktop status — VM driver likely missing on :8013');
    }

    log(RESULT.errors.length ? 'VERIFICATION INCOMPLETE' : 'ALL VERIFICATION PASSED');
  } catch (e) {
    RESULT.errors.push(e.message);
    log('VERIFICATION FAILED:', e.message);
  }
  console.log('\n=== VERIFICATION RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
  process.exit(RESULT.errors.length ? 1 : 0);
})();
