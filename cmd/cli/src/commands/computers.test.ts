import assert from 'node:assert/strict';
import test from 'node:test';
import { Command } from 'commander';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createComputersCommand, ptyWsUrl } from './computers.js';
import { normalizeApiAlias } from '../argv.js';

function buildProgram(commands: Command[]): Command {
  const program = new Command();
  program
    .exitOverride()
    .option('--api-url <url>', 'Allternit API base URL', 'https://api.example')
    .option('--token <token>', 'Clerk bearer token')
    .option('--json', 'emit machine-readable JSON');
  function enableExitOverride(command: Command): void {
    command.exitOverride();
    for (const sub of command.commands) enableExitOverride(sub);
  }
  for (const cmd of commands) {
    enableExitOverride(cmd);
    program.addCommand(cmd);
  }
  return program;
}

async function withMockedFetch(
  respond: (request: Request) => Response | Promise<Response>,
  fn: () => Promise<void>,
): Promise<Request[]> {
  const originalFetch = globalThis.fetch;
  const seen: Request[] = [];
  globalThis.fetch = (async (input, init) => {
    const request = new Request(input, init);
    seen.push(request);
    return respond(request);
  }) as typeof globalThis.fetch;
  try {
    await fn();
  } finally {
    globalThis.fetch = originalFetch;
  }
  return seen;
}

const jsonOk = (value: unknown) =>
  new Response(JSON.stringify(value), { status: 200, headers: { 'content-type': 'application/json' } });

test('computers create sends POST with snake_case CreateComputerRequest payload', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ id: 'computer-1' }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'create',
        '--kind', 'cloud_desktop',
        '--owner-type', 'user',
        '--cpu-cores', '4',
        '--memory-mb', '8192',
        '--disk-mb', '40960',
        '--resolution', '1920x1080',
        '--name', 'dev-box',
        '--os', 'linux',
        '--template-ref', 'system/ubuntu-dev',
        '--session-id', 'sess-1',
        '--persistence', 'persistent',
        '--provider', 'incus',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'POST');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers');
  assert.deepEqual(await seen[0]?.json(), {
    kind: 'cloud_desktop',
    owner_type: 'user',
    cpu_cores: 4,
    memory_mb: 8192,
    disk_mb: 40960,
    resolution: '1920x1080',
    name: 'dev-box',
    os: 'linux',
    template_ref: 'system/ubuntu-dev',
    session_id: 'sess-1',
    persistence: 'persistent',
    provider: 'incus',
  });
});

test('computers create rejects without --kind', async () => {
  await assert.rejects(() =>
    buildProgram([createComputersCommand()]).parseAsync([
      'node', 'allternit', 'computers', 'create',
    ]),
  );
});

test('computers list threads query params', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ computers: [] }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'list',
        '--bot-id', 'bot-1',
        '--kind', 'cloud_desktop',
        '--group-id', 'grp-1',
        '--include-roles',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'GET');
  const url = new URL(seen[0]?.url ?? '');
  assert.equal(url.pathname, '/api/v1/computers');
  assert.equal(url.searchParams.get('bot_id'), 'bot-1');
  assert.equal(url.searchParams.get('kind'), 'cloud_desktop');
  assert.equal(url.searchParams.get('group_id'), 'grp-1');
  assert.equal(url.searchParams.get('include_roles'), '1');
});

test('computers lifecycle commands hit their paths', async () => {
  for (const [name, method, path] of [
    ['get', 'GET', '/api/v1/computers/c-1'],
    ['start', 'POST', '/api/v1/computers/c-1/start'],
    ['stop', 'POST', '/api/v1/computers/c-1/stop'],
    ['restart', 'POST', '/api/v1/computers/c-1/restart'],
    ['delete', 'POST', '/api/v1/computers/c-1/delete'],
  ] as const) {
    const seen = await withMockedFetch(
      () => jsonOk({ id: 'c-1' }),
      async () => {
        await buildProgram([createComputersCommand()]).parseAsync([
          'node', 'allternit', 'computers', name, 'c-1',
        ]);
      },
    );
    assert.equal(seen.length, 1, name);
    assert.equal(seen[0]?.method, method, name);
    assert.equal(seen[0]?.url, `https://api.example${path}`, name);
  }
});

test('computers lifecycle threads approval_id as a query param', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ id: 'c-1' }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'delete', 'c-1', '--approval-id', 'grant-1',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'POST');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/delete?approval_id=grant-1');
});

test('computers resize sends PATCH with resize body', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ id: 'c-1', cpu_cores: 8, memory_mb: 16384, disk_mb: 40960 }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'resize', 'c-1',
        '--cpu-cores', '8',
        '--memory-mb', '16384',
        '--disk-mb', '40960',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'PATCH');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/resize');
  assert.deepEqual(await seen[0]?.json(), { cpu_cores: 8, memory_mb: 16384, disk_mb: 40960 });
});

test('computers clone sends POST with optional name', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ id: 'computer-copy' }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'clone', 'c-1', '--name', 'backup',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'POST');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/clone');
  assert.deepEqual(await seen[0]?.json(), { name: 'backup' });
});

test('computers drive shell posts command array and sets exit code', async () => {
  let exitCode: number | string | undefined;
  const originalWrite = process.stdout.write.bind(process.stdout);
  process.stdout.write = (() => true) as typeof process.stdout.write;
  try {
    const seen = await withMockedFetch(
      () => jsonOk({ exit_code: 5, stdout: 'hello\n', stderr: 'warn\n', duration_ms: 12 }),
      async () => {
        await buildProgram([createComputersCommand()]).parseAsync([
          'node', 'allternit', 'computers', 'drive', 'shell', 'c-1', '--', 'ls', '-la',
        ]);
        exitCode = process.exitCode;
      },
    );
    assert.equal(seen.length, 1);
    assert.equal(seen[0]?.method, 'POST');
    assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/shell');
    assert.deepEqual(await seen[0]?.json(), { command: ['ls', '-la'] });
  } finally {
    process.stdout.write = originalWrite;
    process.exitCode = undefined;
  }
  assert.equal(exitCode, 5);
});

test('computers drive upload streams raw octet-stream bytes', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'allternit-cli-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const localPath = join(dir, 'payload.bin');
  await writeFile(localPath, Buffer.from([1, 2, 3, 250]));
  let receivedBody: Buffer | undefined;
  const seen = await withMockedFetch(
    async (request) => {
      receivedBody = Buffer.from(await request.arrayBuffer());
      return jsonOk({ success: true });
    },
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'drive', 'upload', 'c-1', localPath, '/tmp/payload.bin',
        '--approval-id', 'grant-2',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'POST');
  assert.equal(
    seen[0]?.url,
    'https://api.example/api/v1/computers/c-1/files/upload?path=%2Ftmp%2Fpayload.bin&approval_id=grant-2',
  );
  assert.equal(seen[0]?.headers.get('content-type'), 'application/octet-stream');
  assert.deepEqual(receivedBody, Buffer.from([1, 2, 3, 250]));
});

test('computers drive download writes guest bytes to the local path', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'allternit-cli-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const localPath = join(dir, 'downloaded.bin');
  const seen = await withMockedFetch(
    () => new Response(Buffer.from('guest-bytes'), { status: 200 }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'drive', 'download', 'c-1', '/etc/motd', localPath,
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'GET');
  assert.equal(
    seen[0]?.url,
    'https://api.example/api/v1/computers/c-1/files/download?path=%2Fetc%2Fmotd',
  );
  assert.equal(await readFile(localPath, 'utf8'), 'guest-bytes');
});

test('computers screenshot saves PNG bytes', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'allternit-cli-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const localPath = join(dir, 'screen.png');
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47]);
  const seen = await withMockedFetch(
    () => new Response(png, { status: 200, headers: { 'content-type': 'image/png' } }),
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'screenshot', 'c-1', localPath,
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.method, 'GET');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/screenshot');
  assert.deepEqual(await readFile(localPath), png);
});

test('computers drive screenshot runs click/type one-shots after capture', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'allternit-cli-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const localPath = join(dir, 'screen.png');
  const png = Buffer.from([0x89, 0x50]);
  const seen = await withMockedFetch((request) => {
    if (request.url.endsWith('/screenshot')) {
      return new Response(png, { status: 200, headers: { 'content-type': 'image/png' } });
    }
    return jsonOk({ success: true });
  }, async () => {
    await buildProgram([createComputersCommand()]).parseAsync([
      'node', 'allternit', 'computers', 'drive', 'screenshot', 'c-1', localPath,
      '--click', '10,20',
      '--double-click', '30,40',
      '--right-click', '50,60',
      '--type', 'hello',
      '--key', 'Return',
      '--approval-id', 'grant-9',
    ]);
  });
  assert.equal(seen.length, 6);
  assert.equal(seen[0]?.method, 'GET');
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/screenshot');
  const expected = [
    ['mouse', { action: 'click', x: 10, y: 20 }],
    ['mouse', { action: 'doubleclick', x: 30, y: 40 }],
    ['mouse', { action: 'rightclick', x: 50, y: 60 }],
    ['keyboard', { action: 'type', text: 'hello' }],
    ['keyboard', { action: 'key', key: 'Return' }],
  ] as const;
  for (let i = 0; i < expected.length; i += 1) {
    const [plane, body] = expected[i];
    assert.equal(seen[i + 1]?.method, 'POST', plane);
    assert.equal(seen[i + 1]?.url, `https://api.example/api/v1/computers/c-1/${plane}?approval_id=grant-9`, plane);
    assert.deepEqual(await seen[i + 1]?.json(), body, plane);
  }
  assert.deepEqual(await readFile(localPath), png);
});

test('computers screenshot rejects malformed click coordinates', async () => {
  const originalWrite = process.stderr.write.bind(process.stderr);
  let stderr = '';
  process.stderr.write = ((chunk: unknown) => { stderr += String(chunk); return true; }) as typeof process.stderr.write;
  try {
    await withMockedFetch(
      () => new Response(Buffer.from([0x89]), { status: 200 }),
      async () => {
        await buildProgram([createComputersCommand()]).parseAsync([
          'node', 'allternit', 'computers', 'screenshot', 'c-1', '--click', 'not-a-coordinate',
        ]);
      },
    );
  } finally {
    process.stderr.write = originalWrite;
    process.exitCode = undefined;
  }
  assert.match(stderr, /invalid coordinate/);
});

test('global --api flag is an alias for --api-url', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ computers: [] }),
    async () => {
      const program = new Command();
      program
        .exitOverride()
        .option('--api-url <url>', 'Allternit API base URL', 'https://default.example')
        .addCommand(createComputersCommand());
      await program.parseAsync(normalizeApiAlias(
        ['node', 'allternit', '--api', 'https://api-alias.example', 'computers', 'list'],
      ));
    },
  );
  assert.equal(seen[0]?.url, 'https://api-alias.example/api/v1/computers');
});

test('global --api=value form is an alias for --api-url', async () => {
  const seen = await withMockedFetch(
    () => jsonOk({ computers: [] }),
    async () => {
      const program = new Command();
      program
        .exitOverride()
        .option('--api-url <url>', 'Allternit API base URL', 'https://default.example')
        .addCommand(createComputersCommand());
      await program.parseAsync(normalizeApiAlias(
        ['node', 'allternit', '--api=https://api-eq.example', 'computers', 'list'],
      ));
    },
  );
  assert.equal(seen[0]?.url, 'https://api-eq.example/api/v1/computers');
});

// ── ssh ──────────────────────────────────────────────────────────────────────
//
// The interactive tty passthrough itself is intentionally not automated (it
// needs a real TTY); what is unit-tested here is the honest seam: the ws-token
// mint request and the pty WebSocket URL construction, with global WebSocket
// mocked so the session lifecycle (open -> close) completes.

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  readonly url: string;
  binaryType = 'blob';
  private readonly listeners = new Map<string, Array<(event: unknown) => void>>();

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
    setImmediate(() => {
      this.emit('open', {});
      this.emit('close', {});
    });
  }

  addEventListener(type: string, listener: (event: unknown) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  send(): void {}

  private emit(type: string, event: unknown): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

test('ptyWsUrl maps http(s) API URLs to ws(s) and scopes the token query', () => {
  assert.equal(
    ptyWsUrl('http://127.0.0.1:8013', 'c-1', 'tok x'),
    'ws://127.0.0.1:8013/ws/computers/c-1/pty?token=tok%20x',
  );
  assert.equal(
    ptyWsUrl('https://api.example/', 'a b', 'tok'),
    'wss://api.example/ws/computers/a%20b/pty?token=tok',
  );
});

test('computers ssh mints a pty ws-token then opens the pty socket', async (t) => {
  FakeWebSocket.instances = [];
  const originalWebSocket = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket;
  t.after(() => {
    globalThis.WebSocket = originalWebSocket;
  });
  const seen = await withMockedFetch(
    (request) => {
      assert.equal(request.method, 'POST');
      return jsonOk({ token: 'tok-1', expires_in: 300 });
    },
    async () => {
      await buildProgram([createComputersCommand()]).parseAsync([
        'node', 'allternit', 'computers', 'ssh', 'c-1',
      ]);
    },
  );
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.url, 'https://api.example/api/v1/computers/c-1/ws-token');
  assert.deepEqual(await seen[0]?.json(), { purpose: 'pty' });
  // Global WebSocket was constructed with the pty URL carrying the minted token.
  assert.equal(FakeWebSocket.instances.length, 1);
  assert.equal(
    FakeWebSocket.instances[0]?.url,
    'wss://api.example/ws/computers/c-1/pty?token=tok-1',
  );
});

async function captureOutput(fn: () => Promise<void>): Promise<{ out: string; err: string; exitCode: number | string | undefined }> {
  const outWrite = process.stdout.write.bind(process.stdout);
  const errWrite = process.stderr.write.bind(process.stderr);
  let out = '';
  let err = '';
  process.stdout.write = ((chunk: string | Uint8Array) => { out += String(chunk); return true; }) as typeof process.stdout.write;
  process.stderr.write = ((chunk: string | Uint8Array) => { err += String(chunk); return true; }) as typeof process.stderr.write;
  process.exitCode = undefined;
  try {
    await fn();
  } finally {
    process.stdout.write = outWrite;
    process.stderr.write = errWrite;
  }
  const exitCode = process.exitCode;
  process.exitCode = undefined;
  return { out, err, exitCode };
}

test('computers files-roots shows the state and each root', async () => {
  let result: Awaited<ReturnType<typeof captureOutput>> | undefined;
  const seen = await withMockedFetch(
    () => jsonOk({ id: 'c1', wholeDisk: { enabled: true, available: true, roots: [{ id: 'disk:/', path: '/', label: 'Whole disk' }] } }),
    async () => {
      result = await captureOutput(() => buildProgram([createComputersCommand()]).parseAsync(['node', 'allternit', 'computers', 'files-roots', 'c1']) as Promise<unknown> as Promise<void>);
    },
  );
  assert.equal(seen[0]?.method, 'GET');
  assert.equal(new URL(seen[0]?.url ?? '').pathname, '/api/v1/computers/c1/file-roots');
  assert.match(result?.out ?? '', /Whole-disk browsing: on\./);
  assert.match(result?.out ?? '', /Whole disk {2}\//);
});

test('computers files-roots --on without --yes refuses and sends nothing', async () => {
  let result: Awaited<ReturnType<typeof captureOutput>> | undefined;
  const seen = await withMockedFetch(
    () => jsonOk({}),
    async () => {
      result = await captureOutput(() => buildProgram([createComputersCommand()]).parseAsync(['node', 'allternit', 'computers', 'files-roots', 'c1', '--on']) as Promise<unknown> as Promise<void>);
    },
  );
  assert.equal(seen.length, 0);
  assert.equal(result?.exitCode, 1);
  assert.match(result?.err ?? '', /Anyone using this computer from Allternit can open, change or delete any file on it/);
  assert.match(result?.err ?? '', /--yes/);
});

test('computers files-roots --on --yes sends wholeDisk and confirm; --off sends wholeDisk false', async () => {
  const bodies: unknown[] = [];
  await withMockedFetch(
    async (request) => { bodies.push(await request.json()); return jsonOk({ id: 'c1', wholeDisk: { enabled: false, available: true, roots: [] } }); },
    async () => {
      await captureOutput(() => buildProgram([createComputersCommand()]).parseAsync(['node', 'allternit', 'computers', 'files-roots', 'c1', '--on', '--yes']) as Promise<unknown> as Promise<void>);
      await captureOutput(() => buildProgram([createComputersCommand()]).parseAsync(['node', 'allternit', 'computers', 'files-roots', 'c1', '--off']) as Promise<unknown> as Promise<void>);
    },
  );
  assert.deepEqual(bodies, [{ wholeDisk: true, confirm: true }, { wholeDisk: false }]);
});

test('computers files-roots turns a server refusal into a plain sentence', async () => {
  let result: Awaited<ReturnType<typeof captureOutput>> | undefined;
  await withMockedFetch(
    () => new Response(JSON.stringify({ error: 'not_owner' }), { status: 403, headers: { 'content-type': 'application/json' } }),
    async () => {
      result = await captureOutput(() => buildProgram([createComputersCommand()]).parseAsync(['node', 'allternit', 'computers', 'files-roots', 'c1', '--off']) as Promise<unknown> as Promise<void>);
    },
  );
  assert.equal(result?.exitCode, 1);
  assert.match(result?.err ?? '', /Only the owner of this computer can turn this on\./);
});
