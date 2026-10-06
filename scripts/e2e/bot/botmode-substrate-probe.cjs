// botmode-substrate-probe.cjs — Clerk-independent bot-computer substrate
// verification against the isolated :18013 API (ALLTERNIT_LOCAL_DEV_BYPASS=1,
// own data dir, TART env). Creates a probe bot, provisions a desktop (real
// tart VM via the tart host), polls to running, screenshots, VNC handshake,
// then deprovisions + deletes the bot.
const API = 'http://127.0.0.1:18013/api/v1';
const BOT_NAME = 'Botmode Substrate Probe';

const RESULT = { botId: null, provision: null, statusFlow: [], wsUrl: null, protocol: null, screenshot: null, vnc: null, cleanup: null, errors: [] };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log('[SUBSTRATE]', ...a);

async function api(path, opts = {}) {
  const res = await fetch(`${API}${path}`, {
    ...opts,
    headers: { 'Content-Type': 'application/json', ...(opts.headers || {}) },
  });
  const text = await res.text();
  let json = null;
  try { json = JSON.parse(text); } catch {}
  return { status: res.status, json, text };
}

(async () => {
  try {
    log('Creating probe bot...');
    const create = await api('/agents', {
      method: 'POST',
      body: JSON.stringify({
        name: BOT_NAME,
        description: 'Clerk-independent substrate verification probe.',
        type: 'worker', model: 'allternit/kimi-k3', provider: 'allternit',
        system_prompt: 'Probe bot.', max_iterations: 1, temperature: 0.7,
        trust_tier: 'standard', harness_config: { mode: 'cloud' }, enabled_modes: ['chat'],
      }),
    });
    if (create.status !== 200 && create.status !== 201) throw new Error(`create agent: ${create.status} ${create.text.slice(0, 300)}`);
    RESULT.botId = (create.json.agent || create.json).id;
    log('  bot id:', RESULT.botId);

    log('POST /bots/:id/desktop/provision ...');
    const prov = await api(`/bots/${RESULT.botId}/desktop/provision?os=macos&provider=tart&resolution=${process.env.PROBE_RESOLUTION || '1280x720'}`, { method: 'POST', body: JSON.stringify({}) });
    RESULT.provision = { status: prov.status, body: (prov.json || prov.text.slice(0, 300)) };
    log('  ', prov.status, JSON.stringify(prov.json || prov.text.slice(0, 200)).slice(0, 250));
    if (![200, 201, 202].includes(prov.status)) throw new Error('provision not accepted');
    const sandboxId = prov.json?.sandbox_id;
    const statusPath = `/bots/${RESULT.botId}/desktop${sandboxId ? `?sandbox_id=${sandboxId}` : ''}`;

    const deadline = Date.now() + 10 * 60 * 1000;
    let last = '';
    while (Date.now() < deadline) {
      const st = await api(statusPath);
      const key = `${st.status}:${st.text.slice(0, 100)}`;
      if (key !== last) {
        last = key;
        RESULT.statusFlow.push({ at: new Date().toISOString(), status: st.status, body: st.json || st.text.slice(0, 200) });
        log('  poll:', st.status, (st.json ? JSON.stringify(st.json) : st.text).slice(0, 220));
      }
      const body = st.json || {};
      if (body.ws_url) RESULT.wsUrl = body.ws_url;
      if (body.protocol) RESULT.protocol = body.protocol;
      const state = (body.status || body.state || '').toString();
      if (['running', 'started', 'active', 'ready'].includes(state)) break;
      if (['error', 'failed', 'exited'].includes(state)) throw new Error(`desktop state '${state}'`);
      await sleep(6000);
    }

    if (RESULT.wsUrl) {
      log('Screenshot ...');
      const shot = await api(`/bots/${RESULT.botId}/desktop/screenshot${sandboxId ? `?sandbox_id=${sandboxId}` : ''}`);
      RESULT.screenshot = { status: shot.status, bytes: shot.text.length };
      log('  screenshot:', shot.status, `${shot.text.length}b`);

      log('VNC ws handshake ...');
      try {
        const u = new URL(RESULT.wsUrl, 'http://127.0.0.1:18013');
        u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:';
        const ws = new (require('ws'))(u.toString(), { handshakeTimeout: 10000 });
        await new Promise((resolve, reject) => {
          ws.on('open', () => { RESULT.vnc = 'open'; ws.close(); resolve(); });
          ws.on('upgrade', (res) => { RESULT.vnc = `upgrade ${res.statusCode}`; });
          ws.on('error', reject);
          setTimeout(() => reject(new Error('handshake timeout')), 12000);
        });
      } catch (e) { RESULT.vnc = `error: ${e.message}`; }
      log('  vnc:', RESULT.vnc);
    } else {
      RESULT.errors.push('no ws_url after polling');
    }

    log('Cleanup: deprovision ...');
    const dep = await api(`/bots/${RESULT.botId}/desktop/deprovision`, { method: 'POST', body: JSON.stringify({}) });
    const del = await api(`/agents/${RESULT.botId}`, { method: 'DELETE' });
    RESULT.cleanup = { deprovision: dep.status, deleteBot: del.status };
    log('  deprovision:', dep.status, 'delete bot:', del.status);
    log('PROBE COMPLETE');
  } catch (e) {
    RESULT.errors.push(e.message);
    log('PROBE FAILED:', e.message);
    if (RESULT.botId) {
      await api(`/bots/${RESULT.botId}/desktop/deprovision`, { method: 'POST', body: JSON.stringify({}) }).catch(() => {});
      await api(`/agents/${RESULT.botId}`, { method: 'DELETE' }).catch(() => {});
      log('  (best-effort cleanup done)');
    }
  }
  console.log('\n=== SUBSTRATE RESULT ===');
  console.log(JSON.stringify(RESULT, null, 2));
  process.exit(RESULT.errors.length ? 1 : 0);
})();
