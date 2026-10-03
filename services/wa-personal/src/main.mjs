#!/usr/bin/env node
// Entrypoint. Does nothing (exit 0) unless ALLTERNIT_WA_PERSONAL=1.
import path from 'node:path';
import os from 'node:os';
import { createServer } from './server.mjs';
import { Session } from './session.mjs';
import { loadKey } from './crypto.mjs';
import { WARNING } from './policy.mjs';

const env = process.env;
if (env.ALLTERNIT_WA_PERSONAL !== '1') {
  console.log('wa-personal: disabled (set ALLTERNIT_WA_PERSONAL=1 to enable).');
  process.exit(0);
}
if (!env.ALLTERNIT_WA_PERSONAL_TOKEN) {
  console.error('wa-personal: ALLTERNIT_WA_PERSONAL_TOKEN is required.');
  process.exit(1);
}
console.warn(`wa-personal WARNING: ${WARNING}`);

const dataDir = env.ALLTERNIT_WA_PERSONAL_DIR ?? path.join(env.ALLTERNIT_DATA_DIR ?? path.join(os.homedir(), '.allternit'), 'wa-personal');
const key = loadKey({ env, keyFile: env.ALLTERNIT_WA_PERSONAL_KEY_FILE ?? path.join(path.dirname(dataDir), 'wa-personal.key') });
const apiUrl = (env.ALLTERNIT_API_URL ?? 'http://127.0.0.1:8013').replace(/\/$/, '');

async function forward(event) {
  const r = await fetch(`${apiUrl}/webhooks/channels/whatsapp-personal`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-allternit-sidecar-token': env.ALLTERNIT_WA_PERSONAL_TOKEN },
    body: JSON.stringify(event),
    signal: AbortSignal.timeout(15_000),
  });
  if (!r.ok) throw new Error(`allternit-api answered ${r.status}`);
}

let session;
const getSession = () =>
  (session ??= new Session({
    loadBaileys: () => import('@whiskeysockets/baileys'),
    dataDir,
    key,
    forward,
    owners: (env.ALLTERNIT_WA_PERSONAL_OWNERS ?? '').split(',').map((s) => s.trim()).filter(Boolean),
    log: (...a) => console.warn(...a),
  }));

const port = Number(env.ALLTERNIT_WA_PERSONAL_PORT ?? 8791);
createServer({ env, getSession }).listen(port, '127.0.0.1', () => console.log(`wa-personal listening on 127.0.0.1:${port}`));
