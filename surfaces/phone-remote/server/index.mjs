#!/usr/bin/env node
// phone-remote server — view + control this Mac's desktop from an iPhone over
// Tailscale. Zero npm dependencies (node:http + node:net + hand-rolled RFC 6455).
//
// Security model (mandatory, not optional):
//   - Binds ONLY to the tailnet IPv4 (default: `tailscale ip -4`) + 127.0.0.1
//     for local testing. Never 0.0.0.0, never a public interface.
//   - Every HTTP request and the WS upgrade require a random per-boot token
//     (query ?t=… or the prt cookie it sets). 403 otherwise.
//   - Input injection (Quartz CGEvent HID tap) sits behind both of the above.
//
// Module boundaries mirror the future axum port (cmd/allternit-api):
//   lib/ws.mjs  (upgrade+framing) · lib/capture.mjs (frames) · lib/input.mjs (HID)

import { createServer } from 'node:http';
import { execFileSync } from 'node:child_process';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Capture, checkScreenRecordingPermission } from './lib/capture.mjs';
import { CaptureDemand } from './lib/demand.mjs';
import { InputBridge } from './lib/input.mjs';
import { helloBody } from './lib/status.mjs';
import { upgrade } from './lib/ws.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const CLIENT_DIR = join(HERE, '..', 'client');

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.webmanifest': 'application/manifest+json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
};

function parseArgs(argv) {
  const cfg = {
    // PORT env wins as the default so a supervisor (the desktop app) can pin
    // the port without argv surgery; --port still wins over the env.
    port: process.env.PORT ? Number(process.env.PORT) : 8477,
    bind: null, // null → auto tailscale ip -4
    token: null,
    capture: 'sckit', // sckit | screencapture | none
    fps: 10,
    scale: 1,
    quality: 0.8,
    input: true,
    inputDryRun: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    if (a === '--port') cfg.port = Number(next());
    else if (a === '--bind') cfg.bind = next();
    else if (a === '--token') cfg.token = next();
    else if (a === '--capture') cfg.capture = next();
    else if (a === '--fps') cfg.fps = Number(next());
    else if (a === '--scale') cfg.scale = Number(next());
    else if (a === '--quality') cfg.quality = Number(next());
    else if (a === '--no-input') cfg.input = false;
    else if (a === '--input-dry-run') cfg.inputDryRun = true;
    else if (a === '--help') { console.log(USAGE); process.exit(0); }
    else { console.error(`unknown arg: ${a}\n${USAGE}`); process.exit(64); }
  }
  return cfg;
}

const USAGE = `usage: node server/index.mjs [options]
  --port N             listen port (default 8477)
  --bind IP            tailnet bind address (default: \`tailscale ip -4\`)
  --token T            auth token (default: random per boot)
  --capture MODE       sckit | screencapture | none (default sckit)
  --fps N              capture fps target (default 10)
  --scale F            sckit capture scale 0-1 (default 0.75)
  --quality F          JPEG quality 0-1 (default 0.72)
  --no-input           disable input injection entirely
  --input-dry-run      input helper echoes commands, posts no events`;

function tailscaleIPv4() {
  try {
    return execFileSync('tailscale', ['ip', '-4'], { encoding: 'utf8' }).trim().split('\n')[0];
  } catch {
    return null;
  }
}

function screenLocked() {
  if (process.platform !== 'darwin') return false;
  try {
    const out = execFileSync('python3', ['-c',
      'import Quartz\n'
      + 'd=Quartz.CGSessionCopyCurrentDictionary() or {}\n'
      + 'print("1" if d.get("CGSSessionScreenIsLocked") else "0")',
    ], { timeout: 2000, encoding: 'utf8' });
    return out.trim() === '1';
  } catch {
    return false;
  }
}

function isLoopback(req) {
  const a = req.socket?.remoteAddress || '';
  return a === '127.0.0.1' || a === '::1' || a === '::ffff:127.0.0.1';
}

function tokenMatches(cfg, presented) {
  if (!presented) return false;
  const a = Buffer.from(String(presented));
  const b = Buffer.from(cfg.token);
  return a.length === b.length && timingSafeEqual(a, b);
}

function presentedToken(req, url) {
  const q = url.searchParams.get('t');
  if (q) return q;
  const cookie = req.headers.cookie || '';
  const m = cookie.match(/(?:^|;\s*)prt=([^;]+)/);
  return m ? decodeURIComponent(m[1]) : null;
}

async function serveStatic(res, relPath, extraHeaders = {}) {
  const safe = normalize(relPath).replace(/^(\.\.[/\\])+/, '');
  const file = join(CLIENT_DIR, safe);
  if (!file.startsWith(CLIENT_DIR)) return false;
  try {
    const st = await stat(file);
    if (!st.isFile()) return false;
    const body = await readFile(file);
    res.writeHead(200, {
      'content-type': MIME[extname(file)] || 'application/octet-stream',
      'content-length': body.length,
      'cache-control': 'no-store',
      ...extraHeaders,
    });
    res.end(body);
    return true;
  } catch {
    return false;
  }
}

function persistableTokenPath() {
  if (process.env.PHONE_REMOTE_TOKEN_FILE) return process.env.PHONE_REMOTE_TOKEN_FILE;
  if (process.env.ELECTRON_RUN_AS_NODE === '1') {
    return join(homedir(), 'Library/Application Support/@allternit/desktop/phone-remote-token');
  }
  return null;
}

function loadOrCreateToken(explicit) {
  if (explicit) return explicit;
  const file = persistableTokenPath();
  if (file) {
    try {
      const existing = readFileSync(file, 'utf8').trim();
      if (existing.length >= 16) return existing;
    } catch {
      /* first boot — create below */
    }
  }
  const token = randomBytes(24).toString('base64url');
  if (file) {
    try {
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, token, { encoding: 'utf8', mode: 0o600 });
    } catch {
      /* clipboard/log still carry the in-memory token */
    }
  }
  return token;
}

async function main() {
  const cfg = parseArgs(process.argv.slice(2));
  if (!cfg.bind) cfg.bind = tailscaleIPv4() || '100.88.98.69';
  cfg.token = loadOrCreateToken(cfg.token);

  const capture = cfg.capture === 'none' ? null : new Capture({
    mode: cfg.capture, fps: cfg.fps, scale: cfg.scale, quality: cfg.quality,
  });
  const input = cfg.input ? new InputBridge({ dryRun: cfg.inputDryRun }) : null;

  let viewer = null; // single-viewer: the one WSConnection holding the session

  // Capture runs only while someone wants frames (a viewer, or Fabric polling /frame).
  const demand = new CaptureDemand({ capture });

  function sendJSON(conn, obj) { conn.sendText(JSON.stringify(obj)); }

  function onViewerMessage(conn, text) {
    let msg;
    try { msg = JSON.parse(text); } catch { return sendJSON(conn, { type: 'error', error: 'bad json' }); }
    if (msg.type === 'ping') return sendJSON(conn, { type: 'pong', t: Date.now() });
    if (msg.type === 'view' && input) {
      // Client reports the stream-image pixel size it renders (authority for
      // coordinate mapping); mapping to screen points is server-side.
      input.setImageSize(Number(msg.imgW) || 0, Number(msg.imgH) || 0);
      return;
    }
    if (msg.type === 'input' && msg.ev && input) {
      input.handleClientEvent(msg.ev).then((r) => {
        if (!r.ok) sendJSON(conn, { type: 'input-error', error: r.error, ev: msg.ev.type });
      });
      return;
    }
    sendJSON(conn, { type: 'error', error: `unknown message type: ${msg.type}` });
  }

  function attachViewer(conn) {
    if (viewer) {
      sendJSON(conn, { type: 'error', error: 'another viewer is connected (single-viewer build)' });
      conn.close(1013, 'single viewer');
      return;
    }
    viewer = conn;
    console.error('[server] viewer connected');
    sendJSON(conn, {
      type: 'hello',
      capture: { mode: capture?.actualMode ?? cfg.capture, fps: cfg.fps },
      input: input ? { enabled: true, dryRun: cfg.inputDryRun, accessibilityTrusted: input.ready?.accessibilityTrusted ?? null } : { enabled: false },
      display: input?.display ?? null,
    });
    demand.viewerAttached();

    conn.on('text', (t) => onViewerMessage(conn, t));
    conn.on('close', () => {
      console.error('[server] viewer disconnected');
      if (viewer === conn) {
        viewer = null;
        demand.viewerDetached();
        cfg.token = randomBytes(24).toString('base64url');
        const next = `http://${cfg.bind}:${cfg.port}/?t=${cfg.token}`;
        console.log(`  token rotated after viewer disconnect\n    ${next}\n`);
      }
    });
    conn.on('error', () => {});
  }

  capture?.on('frame', (jpeg) => {
    if (!viewer) return;
    // Drop frames under backpressure rather than growing the send queue —
    // the phone always wants the newest frame, not the backlog.
    if (viewer.buffered > 512 * 1024) return;
    viewer.sendBinary(jpeg);
  });
  capture?.on('info', ({ width, height }) => {
    if (viewer && width > 0) sendJSON(viewer, { type: 'screen', width, height });
  });
  capture?.on('captureError', (e) => {
    console.error(`[capture] error: ${e.error}`);
    if (viewer) sendJSON(viewer, { type: 'error', error: `capture: ${e.error}` });
  });
  capture?.on('exit', (code) => console.error(`[capture] exited (${code})`));
  capture?.on('fatal', (reason) => {
    // In-process recovery is exhausted (watchdog tripped twice or the helper
    // is gone). Exit non-zero so a supervisor — the desktop app supervises
    // its own child — restarts the whole server instead of serving a stale
    // frame forever.
    console.error(`[server] capture unrecoverable: ${reason} — exiting non-zero for supervisor restart`);
    process.exit(1);
  });

  if (input) {
    input.on('ready', ({ display }) => {
      if (viewer) sendJSON(viewer, { type: 'display', display });
    });
    input.start();
  }

  const httpHandler = async (req, res) => {
    const url = new URL(req.url, 'http://x');
    if (url.pathname === '/healthz') { res.writeHead(200, { 'content-type': 'text/plain' }); return res.end('ok'); }

    // Loopback is the Fabric shim on this machine. Token still required off-loopback.
    const trustedLocal = isLoopback(req);
    if (url.pathname === '/hello' && (req.method === 'GET' || req.method === 'HEAD')) {
      if (!trustedLocal && !tokenMatches(cfg, presentedToken(req, url))) {
        res.writeHead(403, { 'content-type': 'text/plain' }); return res.end('403\n');
      }
      demand.demand(); // a viewer says hello before it polls frames
      const body = Buffer.from(JSON.stringify(helloBody({
        capture, captureMode: cfg.capture, fps: cfg.fps,
        input, inputDryRun: cfg.inputDryRun, display: input?.display ?? null,
      })));
      res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store', 'content-length': body.length });
      return res.end(body);
    }
    if (url.pathname === '/frame' && req.method === 'GET') {
      if (!trustedLocal && !tokenMatches(cfg, presentedToken(req, url))) {
        res.writeHead(403, { 'content-type': 'text/plain' }); return res.end('403\n');
      }
      const jpeg = capture ? await demand.frame() : null;
      if (!jpeg) { res.writeHead(503, { 'content-type': 'text/plain' }); return res.end('no frame\n'); }
      res.writeHead(200, { 'content-type': 'image/jpeg', 'cache-control': 'no-store', 'content-length': jpeg.length });
      return res.end(jpeg);
    }
    if (url.pathname === '/start' && req.method === 'POST') {
      if (!trustedLocal && !tokenMatches(cfg, presentedToken(req, url))) {
        res.writeHead(403, { 'content-type': 'text/plain' }); return res.end('403\n');
      }
      const already = Boolean(capture?.running);
      demand.demand();
      const body = Buffer.from(JSON.stringify({ ok: true, already, pid: process.pid }));
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': body.length });
      return res.end(body);
    }
    if (url.pathname === '/input' && req.method === 'POST') {
      if (!trustedLocal && !tokenMatches(cfg, presentedToken(req, url))) {
        res.writeHead(403, { 'content-type': 'text/plain' }); return res.end('403\n');
      }
      const chunks = [];
      for await (const c of req) chunks.push(c);
      let ev;
      try { ev = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch {
        res.writeHead(400, { 'content-type': 'text/plain' }); return res.end('bad json\n');
      }
      if (!input) { res.writeHead(409, { 'content-type': 'text/plain' }); return res.end('input disabled\n'); }
      if (ev.type === 'view') {
        input.setImageSize(Number(ev.imgW) || 0, Number(ev.imgH) || 0);
        res.writeHead(204); return res.end();
      }
      const r = await input.handleClientEvent(ev);
      res.writeHead(r.ok ? 204 : 400, { 'content-type': 'application/json' });
      return res.end(r.ok ? '' : JSON.stringify(r));
    }

    if (req.method !== 'GET') { res.writeHead(405); return res.end(); }
    if (!tokenMatches(cfg, presentedToken(req, url))) {
      res.writeHead(403, { 'content-type': 'text/plain; charset=utf-8', 'cache-control': 'no-store' });
      return res.end('403 — missing or invalid token. Use the full URL printed by the server at startup.\n');
    }
    const headers = {};
    if (url.searchParams.has('t')) {
      headers['set-cookie'] = `prt=${encodeURIComponent(cfg.token)}; Path=/; SameSite=Strict; HttpOnly`;
    }
    const rel = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
    if (rel === 'manifest.webmanifest') {
      // Bake the token into start_url so iOS Add to Home Screen relaunches
      // with auth (static start_url "./" would 403 after the query is dropped).
      const raw = await readFile(join(CLIENT_DIR, 'manifest.webmanifest'), 'utf8');
      const manifest = JSON.parse(raw);
      manifest.start_url = `./?t=${encodeURIComponent(cfg.token)}`;
      const body = Buffer.from(JSON.stringify(manifest));
      res.writeHead(200, {
        'content-type': 'application/manifest+json',
        'content-length': body.length,
        'cache-control': 'no-store',
        ...headers,
      });
      return res.end(body);
    }
    const ok = await serveStatic(res, rel, headers);
    if (!ok) { res.writeHead(404, { 'content-type': 'text/plain' }); res.end('not found\n'); }
  };

  const onUpgrade = (req, socket, head) => {
    const url = new URL(req.url, 'http://x');
    if (url.pathname !== '/ws' || !tokenMatches(cfg, presentedToken(req, url))) {
      socket.write('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n');
      socket.destroy();
      return;
    }
    const conn = upgrade(req, socket, head);
    if (conn) attachViewer(conn);
  };

  const servers = [];
  const binds = [...new Set([cfg.bind, '127.0.0.1'])].map((host) => [host, cfg.port]);
  for (const [host, port] of binds) {
    const srv = createServer(httpHandler);
    srv.on('upgrade', onUpgrade);
    await new Promise((resolve, reject) => {
      srv.once('error', reject);
      srv.listen(port, host, () => {
        console.error(`[server] listening on http://${host}:${srv.address().port}`);
        resolve();
      });
    }).catch((err) => {
      console.error(`[server] FAILED to bind ${host}:${port} — ${err.message}`);
      if (host === cfg.bind) console.error('[server] (tailnet bind failed; is Tailscale up? loopback may still be bound)');
    });
    servers.push(srv);
  }

  // TCC preflight: say it plainly at startup, never fake success later.
  try {
    const granted = await checkScreenRecordingPermission();
    console.error(`[server] Screen Recording permission: ${granted ? 'granted' : 'DENIED — capture will fail; see README TCC runbook'}`);
  } catch (err) {
    console.error(`[server] Screen Recording preflight inconclusive: ${err.message}`);
  }

  const url = `http://${cfg.bind}:${cfg.port}/?t=${cfg.token}`;
  console.log(`\n  phone-remote ready. Open on the iPhone (Tailscale on):\n\n    ${url}\n`);
  console.log(`  local test: http://127.0.0.1:${cfg.port}/?t=${cfg.token}\n`);
  // Supervised Desktop restarts used to pbcopy a new URL on every capture
  // crash, which spammed Messages / Universal Clipboard. Only copy when a
  // human launched the server themselves.
  if (process.env.ELECTRON_RUN_AS_NODE !== '1') {
    try {
      execFileSync('pbcopy', { input: url, encoding: 'utf8' });
      console.log('  (URL copied to the Mac clipboard — AirDrop / Messages it to the phone)\n');
    } catch {
      /* pbcopy absent or not a Mac clipboard session; URL is still printed */
    }
  }

  const shutdown = () => {
    console.error('\n[server] shutting down');
    capture?.stop();
    input?.stop();
    for (const s of servers) s.close();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

main().catch((err) => { console.error(err); process.exit(1); });
