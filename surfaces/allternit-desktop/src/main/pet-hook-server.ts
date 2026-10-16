import http from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { chmodSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { isPetState, type PetState } from './pet-activity-hub.js';

/**
 * Loopback port that lets terminal agents (gizzi, Claude Code, Codex, any
 * script) drive the desktop pet. Only processes that can read the token file
 * (mode 0600 in the app's data folder) can talk to it; browsers are refused
 * by Origin and Host checks, so a web page can't poke it either.
 *
 *   POST /state   { "state": "<signal or pet state>", "client"?: "claude", "ttlMs"?: 120000 }
 *   POST /bubble  { "text": "Tests pass", "client"?: "claude", "ttlMs"?: 5000 }
 *   GET  /health
 */
export const PET_HOOK_TOKEN_FILE = 'desktop-pet-hook.json';
export const PET_HOOK_SCRIPT_FILE = 'desktop-pet-hook.sh';
const MAX_BODY = 4096;
const MAX_TTL = 10 * 60_000;
const ONGOING_TTL = 2 * 60_000;

/** Hook-friendly signal names (same vocabulary as the app's pet-activity signal) → what the pet plays and for how long. */
export const PET_HOOK_SIGNALS: Record<string, { state: PetState; ttlMs: number }> = {
  thinking: { state: 'review', ttlMs: ONGOING_TTL },
  tool: { state: 'running', ttlMs: ONGOING_TTL },
  'tool-done': { state: 'review', ttlMs: ONGOING_TTL },
  waiting: { state: 'waiting', ttlMs: MAX_TTL },
  'turn-done': { state: 'waving', ttlMs: 2400 },
  'plan-done': { state: 'jumping', ttlMs: 2400 },
  failed: { state: 'failed', ttlMs: 4000 },
  idle: { state: 'idle', ttlMs: 0 },
};
const ONE_SHOT: Partial<Record<PetState, number>> = { waving: 2400, jumping: 2400, failed: 4000, special: 2400 };

/** What a POST /state body asks for, or null. Terminal sources always expire: a closed terminal can't leave the pet stuck. */
export function resolveHookState(body: unknown): { client: string; state: PetState; ttlMs: number } | null {
  if (!body || typeof body !== 'object') return null;
  const b = body as Record<string, unknown>;
  const client = hookClient(b.client);
  if (typeof b.state !== 'string') return null;
  const signal = PET_HOOK_SIGNALS[b.state];
  const state = signal?.state ?? (isPetState(b.state) ? b.state : null);
  if (!state) return null;
  if (state === 'idle') return { client, state, ttlMs: 0 };
  const fallback = signal?.ttlMs ?? ONE_SHOT[state] ?? ONGOING_TTL;
  const asked = typeof b.ttlMs === 'number' && Number.isFinite(b.ttlMs) ? b.ttlMs : fallback;
  return { client, state, ttlMs: Math.round(Math.min(MAX_TTL, Math.max(1000, asked))) };
}

/** Bubble text: one line, no control characters, at most 140 characters. */
export function resolveHookBubble(body: unknown): { client: string; text: string; ttlMs: number } | null {
  if (!body || typeof body !== 'object') return null;
  const b = body as Record<string, unknown>;
  if (typeof b.text !== 'string') return null;
  const text = b.text.replace(/[\u0000-\u001f\u007f-\u009f]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 140);
  if (!text) return null;
  const asked = typeof b.ttlMs === 'number' && Number.isFinite(b.ttlMs) ? b.ttlMs : 5000;
  return { client: hookClient(b.client), text, ttlMs: Math.round(Math.min(30_000, Math.max(1500, asked))) };
}

function hookClient(value: unknown): string {
  return typeof value === 'string' && /^[a-z0-9][a-z0-9_-]{0,31}$/i.test(value) ? value.toLowerCase() : 'cli';
}

/** Keeps the token across restarts so configured hooks keep working; writes a fresh one if the file is missing or bad. */
function loadOrCreateToken(file: string): string {
  try {
    const saved = JSON.parse(readFileSync(file, 'utf8')) as { token?: unknown };
    if (typeof saved.token === 'string' && /^[a-f0-9]{64}$/.test(saved.token)) return saved.token;
  } catch { /* first run */ }
  return randomBytes(32).toString('hex');
}

function writePrivate(file: string, contents: string, mode: number) {
  mkdirSync(path.dirname(file), { recursive: true });
  const temp = `${file}.${process.pid}.tmp`;
  writeFileSync(temp, contents, { mode });
  chmodSync(temp, mode);
  renameSync(temp, file);
}

/**
 * The hook script written beside the token: `desktop-pet-hook.sh <signal> [client]`
 * or `desktop-pet-hook.sh bubble "<text>" [client]`. It never blocks the agent
 * (1 s cap) and exits 0 when Allternit isn't running.
 */
export const PET_HOOK_SCRIPT = `#!/bin/sh
# Allternit desktop pet hook. Written by Allternit Desktop; safe to call from any agent hook.
#   desktop-pet-hook.sh <thinking|tool|tool-done|waiting|turn-done|plan-done|failed|idle> [client]
#   desktop-pet-hook.sh bubble "<text>" [client]
f="$(dirname "$0")/${PET_HOOK_TOKEN_FILE}"
[ -r "$f" ] || exit 0
command -v curl >/dev/null 2>&1 || exit 0
url=$(sed -n 's/.*"url":"\\([^"]*\\)".*/\\1/p' "$f")
token=$(sed -n 's/.*"token":"\\([^"]*\\)".*/\\1/p' "$f")
[ -n "$url" ] && [ -n "$token" ] || exit 0
esc() { printf '%s' "$1" | tr '\\n\\r\\t' '   ' | sed 's/\\\\/\\\\\\\\/g; s/"/\\\\"/g'; }
if [ "$1" = bubble ]; then
  path=bubble; body="{\\"text\\":\\"$(esc "$2")\\",\\"client\\":\\"$(esc "\${3:-cli}")\\"}"
else
  path=state; body="{\\"state\\":\\"$(esc "$1")\\",\\"client\\":\\"$(esc "\${2:-cli}")\\"}"
fi
curl -s -m 1 -o /dev/null -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data "$body" "$url/$path" >/dev/null 2>&1
exit 0
`;

export interface PetHookServer { port: number; url: string; close(): Promise<void>; }

export async function startPetHookServer(options: {
  port: number;
  /** Folder for the token file and hook script (the app's userData). */
  dir: string;
  report: (client: string, state: PetState, ttlMs: number) => void;
  bubble: (client: string, text: string, ttlMs: number) => void;
}): Promise<PetHookServer> {
  const tokenFile = path.join(options.dir, PET_HOOK_TOKEN_FILE);
  const token = loadOrCreateToken(tokenFile);
  const expected = Buffer.from(`Bearer ${token}`);
  let port = options.port;

  const server = http.createServer((req, res) => {
    const reply = (status: number, body: unknown) => {
      res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end(JSON.stringify(body));
    };
    // Browsers always send Origin on cross-site requests; terminal tools don't. Host blocks DNS rebinding.
    if (req.headers.origin !== undefined) return reply(403, { error: 'forbidden' });
    if (req.headers.host !== `127.0.0.1:${port}` && req.headers.host !== `localhost:${port}`) return reply(403, { error: 'forbidden' });
    const auth = Buffer.from(req.headers.authorization ?? '');
    if (auth.length !== expected.length || !timingSafeEqual(auth, expected)) return reply(401, { error: 'unauthorized' });
    const route = `${req.method} ${(req.url ?? '').split('?')[0]}`;
    if (route === 'GET /health') return reply(200, { ok: true });
    if (route !== 'POST /state' && route !== 'POST /bubble') return reply(404, { error: 'not found' });
    const chunks: Buffer[] = [];
    let size = 0;
    let tooBig = false;
    req.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_BODY) { tooBig = true; reply(413, { error: 'too large' }); req.destroy(); return; }
      chunks.push(chunk);
    });
    req.on('end', () => {
      if (tooBig) return;
      let body: unknown;
      try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { return reply(400, { error: 'bad json' }); }
      if (route === 'POST /state') {
        const s = resolveHookState(body);
        if (!s) return reply(400, { error: 'unknown state' });
        options.report(s.client, s.state, s.ttlMs);
        return reply(200, { ok: true, state: s.state });
      }
      const b = resolveHookBubble(body);
      if (!b) return reply(400, { error: 'empty text' });
      options.bubble(b.client, b.text, b.ttlMs);
      return reply(200, { ok: true });
    });
  });
  server.headersTimeout = 5000;
  server.requestTimeout = 5000;

  const listen = (p: number) => new Promise<number>((resolve, reject) => {
    const onError = (error: Error) => { server.off('listening', onListening); reject(error); };
    const onListening = () => { server.off('error', onError); resolve((server.address() as { port: number }).port); };
    server.once('error', onError);
    server.once('listening', onListening);
    server.listen(p, '127.0.0.1');
  });
  try {
    port = await listen(options.port);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EADDRINUSE') throw error;
    // Something else holds the usual port: take any free one. Hooks read the port from the token file.
    port = await listen(0);
  }
  server.unref();
  const url = `http://127.0.0.1:${port}`;
  writePrivate(tokenFile, JSON.stringify({ version: 1, url, port, token }), 0o600);
  writePrivate(path.join(options.dir, PET_HOOK_SCRIPT_FILE), PET_HOOK_SCRIPT, 0o700);
  return { port, url, close: () => new Promise<void>(resolve => server.close(() => resolve())) };
}
