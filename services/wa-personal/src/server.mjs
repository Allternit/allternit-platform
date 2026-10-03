// Local HTTP API: start (QR) / qr / status / send / pairing / logout. Bound to loopback by main.mjs.
import http from 'node:http';
import crypto from 'node:crypto';
import { WARNING } from './policy.mjs';

const MAX_BODY = 64 * 1024;

function same(a, b) {
  const x = Buffer.from(String(a));
  const y = Buffer.from(String(b));
  return x.length === y.length && crypto.timingSafeEqual(x, y);
}

async function readJson(req) {
  let n = 0;
  const chunks = [];
  for await (const c of req) {
    n += c.length;
    if (n > MAX_BODY) throw Object.assign(new Error('too_large'), { status: 413 });
    chunks.push(c);
  }
  if (!chunks.length) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    throw Object.assign(new Error('invalid_json'), { status: 400 });
  }
}

/** @returns an http request listener. `getSession` is lazy so a disabled sidecar never loads Baileys. */
export function createHandler({ env = process.env, getSession }) {
  const enabled = env.ALLTERNIT_WA_PERSONAL === '1';
  const token = env.ALLTERNIT_WA_PERSONAL_TOKEN ?? '';
  return async (req, res) => {
    const out = (status, body) => {
      res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
      res.end(JSON.stringify(body));
    };
    try {
      if (!enabled) return out(503, { error: 'wa_personal_disabled', message: 'Set ALLTERNIT_WA_PERSONAL=1 to turn on the unofficial WhatsApp personal-number bridge.', warning: WARNING });
      if (!token) return out(503, { error: 'wa_personal_not_configured', message: 'ALLTERNIT_WA_PERSONAL_TOKEN is not set.' });
      const given = (req.headers.authorization ?? '').replace(/^Bearer\s+/i, '');
      if (!same(given, token)) return out(401, { error: 'unauthorized' });
      const { pathname } = new URL(req.url, 'http://local');
      const route = `${req.method} ${pathname}`;
      const s = () => getSession();
      switch (route) {
        case 'POST /session/start': {
          const info = await s().start();
          return out(200, { ...info, qr: s().qr });
        }
        case 'GET /session/qr': {
          const sess = s();
          if (!sess.qr) return out(404, { error: 'no_qr', status: sess.status, warning: WARNING });
          return out(200, { qr: sess.qr, status: sess.status, warning: WARNING });
        }
        case 'GET /status':
          return out(200, s().info());
        case 'POST /send': {
          const { to, text } = await readJson(req);
          if (typeof to !== 'string' || !to || typeof text !== 'string' || !text) return out(400, { error: 'to_and_text_required' });
          try {
            return out(200, await s().send(to, text));
          } catch (e) {
            if (e.code === 'not_connected') return out(409, { error: 'not_connected', status: s().status });
            throw e;
          }
        }
        case 'GET /pairing':
          return out(200, { pending: s().pending() });
        case 'POST /pairing/approve': {
          const { number } = await readJson(req);
          return s().approve(number) ? out(200, { ok: true }) : out(400, { error: 'number_required' });
        }
        case 'POST /pairing/deny': {
          const { number } = await readJson(req);
          return out(200, { ok: s().deny(number) });
        }
        case 'POST /session/logout':
          return out(200, await s().logout());
        default:
          return out(404, { error: 'not_found' });
      }
    } catch (e) {
      return out(e.status ?? 500, { error: e.status ? e.message : 'internal_error' });
    }
  };
}

export function createServer(opts) {
  return http.createServer(createHandler(opts));
}
