import { describe, expect, it, afterEach } from 'vitest';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { PET_HOOK_SCRIPT_FILE, PET_HOOK_TOKEN_FILE, resolveHookBubble, resolveHookState, startPetHookServer, type PetHookServer } from './pet-hook-server.js';

const dirs: string[] = [];
const servers: PetHookServer[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map(s => s.close()));
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

async function start() {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'pet-hook-'));
  dirs.push(dir);
  const states: Array<[string, string, number]> = [];
  const bubbles: Array<[string, string, number]> = [];
  const server = await startPetHookServer({ port: 0, dir, report: (...a) => states.push(a), bubble: (...a) => bubbles.push(a) });
  servers.push(server);
  const { token } = JSON.parse(readFileSync(path.join(dir, PET_HOOK_TOKEN_FILE), 'utf8')) as { token: string };
  return { dir, server, token, states, bubbles };
}

const post = (url: string, body: unknown, headers: Record<string, string> = {}) =>
  fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: typeof body === 'string' ? body : JSON.stringify(body) });

describe('hook payloads', () => {
  it('maps signals and pet states, always with an expiry', () => {
    expect(resolveHookState({ state: 'tool', client: 'Claude' })).toEqual({ client: 'claude', state: 'running', ttlMs: 120_000 });
    expect(resolveHookState({ state: 'turn-done' })).toEqual({ client: 'cli', state: 'waving', ttlMs: 2400 });
    expect(resolveHookState({ state: 'review', ttlMs: 10 })).toMatchObject({ state: 'review', ttlMs: 1000 });
    expect(resolveHookState({ state: 'running', ttlMs: 1e9 })).toMatchObject({ ttlMs: 600_000 });
    expect(resolveHookState({ state: 'idle' })).toMatchObject({ state: 'idle', ttlMs: 0 });
    expect(resolveHookState({ state: 'dance' })).toBeNull();
    expect(resolveHookState({ state: 'tool', client: '../etc' })).toMatchObject({ client: 'cli' });
  });

  it('cleans bubble text', () => {
    expect(resolveHookBubble({ text: '  Tests\npass\u0007 ' })).toMatchObject({ text: 'Tests pass', ttlMs: 5000 });
    expect(resolveHookBubble({ text: 'x'.repeat(500) })!.text).toHaveLength(140);
    expect(resolveHookBubble({ text: '   ' })).toBeNull();
  });
});

describe('pet hook server', () => {
  it('writes a private token file and hook script', async () => {
    const { dir, server } = await start();
    const file = path.join(dir, PET_HOOK_TOKEN_FILE);
    expect(statSync(file).mode & 0o777).toBe(0o600);
    expect(statSync(path.join(dir, PET_HOOK_SCRIPT_FILE)).mode & 0o777).toBe(0o700);
    expect(JSON.parse(readFileSync(file, 'utf8'))).toMatchObject({ version: 1, url: server.url, port: server.port });
  });

  it('accepts authorized state and bubble posts', async () => {
    const { server, token, states, bubbles } = await start();
    const auth = { Authorization: `Bearer ${token}` };
    expect((await post(`${server.url}/state`, { state: 'tool', client: 'claude' }, auth)).status).toBe(200);
    expect(states).toEqual([['claude', 'running', 120_000]]);
    expect((await post(`${server.url}/bubble`, { text: 'Tests pass' }, auth)).status).toBe(200);
    expect(bubbles).toEqual([['cli', 'Tests pass', 5000]]);
    expect((await fetch(`${server.url}/health`, { headers: auth })).status).toBe(200);
    expect((await post(`${server.url}/state`, { state: 'dance' }, auth)).status).toBe(400);
    expect((await post(`${server.url}/state`, '{oops', auth)).status).toBe(400);
  });

  it('refuses missing tokens, browsers, foreign hosts and big bodies', async () => {
    const { server, token, states } = await start();
    const auth = { Authorization: `Bearer ${token}` };
    expect((await post(`${server.url}/state`, { state: 'tool' })).status).toBe(401);
    expect((await post(`${server.url}/state`, { state: 'tool' }, { Authorization: 'Bearer nope' })).status).toBe(401);
    expect((await post(`${server.url}/state`, { state: 'tool' }, { ...auth, Origin: 'https://evil.example' })).status).toBe(403);
    // fetch can't override Host; send a raw request for the rebinding case.
    const raw = await new Promise<string>((resolve) => {
      const socket = net.connect(server.port, '127.0.0.1', () => socket.end(`GET /health HTTP/1.1\r\nHost: evil.example:${server.port}\r\nAuthorization: Bearer ${token}\r\nConnection: close\r\n\r\n`));
      let data = ''; socket.on('data', c => { data += c; }); socket.on('close', () => resolve(data));
    });
    expect(raw).toMatch(/^HTTP\/1.1 403/);
    const big = await post(`${server.url}/bubble`, { text: 'x'.repeat(5000) }, auth).then(r => r.status, () => 413);
    expect(big).toBe(413);
    expect(states).toEqual([]);
  });

  it('keeps the token across restarts and falls back to a free port when busy', async () => {
    const first = await start();
    const again = await startPetHookServer({ port: first.server.port, dir: first.dir, report: () => {}, bubble: () => {} });
    servers.push(again);
    expect(again.port).not.toBe(first.server.port);
    const saved = JSON.parse(readFileSync(path.join(first.dir, PET_HOOK_TOKEN_FILE), 'utf8'));
    expect(saved.token).toBe(first.token);
    expect(saved.port).toBe(again.port);
  });

  it.skipIf(process.platform === 'win32')('the hook script posts states and bubbles, and is silent when the app is gone', async () => {
    const { dir, states, bubbles } = await start();
    const script = path.join(dir, PET_HOOK_SCRIPT_FILE);
    execFileSync(script, ['tool', 'claude']);
    execFileSync(script, ['bubble', 'Say "hi" \\ now', 'codex']);
    await new Promise(r => setTimeout(r, 50));
    expect(states).toEqual([['claude', 'running', 120_000]]);
    expect(bubbles).toEqual([['codex', 'Say "hi" \\ now', 5000]]);
    await servers.splice(0)[0].close();
    expect(() => execFileSync(script, ['tool'])).not.toThrow();
  });
});
