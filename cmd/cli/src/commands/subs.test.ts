import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { Command } from 'commander';
import { createSubsCommand } from './subs.js';

function buildProgram(): Command {
  const program = new Command();
  program.exitOverride().option('--json', 'emit machine-readable JSON');
  const subs = createSubsCommand();
  const enableExitOverride = (command: Command): void => {
    command.exitOverride();
    for (const sub of command.commands) enableExitOverride(sub);
  };
  enableExitOverride(subs);
  program.addCommand(subs);
  return program;
}

// SubsClient speaks HTTP over the gateway's unix socket, so the mock is a real
// UDS server plus SUBS_GATEWAY_STATE_DIR/SUBS_GATEWAY_TOKEN env overrides.
// Routes may be keyed by bare URL or by "METHOD URL" (method wins); when
// `calls` is given, every request is recorded as "METHOD URL".
async function withGateway(
  routes: Record<string, unknown>,
  fn: () => Promise<unknown>,
  calls?: string[],
): Promise<string> {
  const dir = mkdtempSync(join(tmpdir(), 'subs-cmd-test-'));
  const server = http.createServer((req, res) => {
    const key = `${req.method} ${req.url}`;
    calls?.push(key);
    const body = routes[key] ?? routes[req.url ?? ''];
    if (body === undefined) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{"error":"not found"}');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise<void>((resolve) => server.listen(join(dir, 'gateway.sock'), resolve));
  const prevStateDir = process.env.SUBS_GATEWAY_STATE_DIR;
  const prevToken = process.env.SUBS_GATEWAY_TOKEN;
  process.env.SUBS_GATEWAY_STATE_DIR = dir;
  process.env.SUBS_GATEWAY_TOKEN = 'sgw_test';
  let out = '';
  const prevWrite = process.stdout.write.bind(process.stdout);
  // Capture only string writes (the command's output). Under tsx --test the
  // runner's child-process IPC rides the same channel as binary v8-serialized
  // Buffer writes — pass those through untouched or the capture fills with
  // mojibake and the parent runner loses its test:enqueue/dequeue events.
  process.stdout.write = ((chunk: unknown, ...rest: unknown[]) => {
    if (typeof chunk === 'string') {
      out += chunk;
      return true;
    }
    return prevWrite(chunk as never, ...(rest as never[]));
  }) as typeof process.stdout.write;
  try {
    await fn();
  } finally {
    process.stdout.write = prevWrite;
    if (prevStateDir === undefined) delete process.env.SUBS_GATEWAY_STATE_DIR;
    else process.env.SUBS_GATEWAY_STATE_DIR = prevStateDir;
    if (prevToken === undefined) delete process.env.SUBS_GATEWAY_TOKEN;
    else process.env.SUBS_GATEWAY_TOKEN = prevToken;
    await new Promise((resolve) => server.close(resolve));
    rmSync(dir, { recursive: true, force: true });
  }
  return out;
}

const catalog = [
  {
    id: 'subs/prov-a:fast',
    name: 'Work (subscription) · Fast',
    provider: 'prov-a',
    tier: 'fast',
    description: 'Subscription lane — no metered cost',
    supports_effort: false,
    health: 'ready',
    fabric: {
      adapter_id: 'adapter-a',
      account_id: 'acct-1',
      capability: 'chat.create',
      options: { model_class: 'fast' },
      pool_key: 'prov-a:acct-1:chat-msgs',
    },
  },
  {
    id: 'subs/prov-a:reasoning',
    name: 'Work (subscription) · Reasoning',
    provider: 'prov-a',
    tier: 'flagship',
    description: 'Subscription lane — no metered cost',
    supports_effort: false,
    health: 'degraded',
    fabric: {
      adapter_id: 'adapter-a',
      account_id: 'acct-1',
      capability: 'chat.create',
      options: { model_class: 'reasoning' },
      pool_key: 'prov-a:acct-1:chat-msgs',
    },
  },
];

test('subs models prints a compact id/name/health table by default', async () => {
  const out = await withGateway({ '/v1/catalog': catalog }, () =>
    buildProgram().parseAsync(['node', 'allternit', 'subs', 'models']),
  );
  assert.match(out, /ID\s+NAME\s+HEALTH/);
  assert.match(out, /subs\/prov-a:fast\s+Work \(subscription\) · Fast\s+ready/);
  assert.match(out, /subs\/prov-a:reasoning\s+Work \(subscription\) · Reasoning\s+degraded/);
  assert.ok(!out.trimStart().startsWith('['), 'default output is a table, not JSON');
});

test('subs models --json prints the raw catalog entries', async () => {
  const out = await withGateway({ '/v1/catalog': catalog }, () =>
    buildProgram().parseAsync(['node', 'allternit', '--json', 'subs', 'models']),
  );
  assert.deepEqual(JSON.parse(out), catalog);
});

test('subs status keeps the account rows and adds pools + adapter_stats additively', async () => {
  const accounts = [
    {
      account_id: 'acct-1',
      provider: 'prov-a',
      label: 'Work',
      plan: 'plus',
      session_health: 'ready',
      enabled: true,
    },
  ];
  const capabilities = [
    {
      capability: 'chat.create',
      adapter_id: 'adapter-a',
      adapter_version: '1.0.0',
      provider: 'prov-a',
      pool_id: 'chat-msgs',
      plans: ['plus'],
      detachable: false,
      export_formats: [],
      status: 'stable',
      lane: 'subscription',
      entitlements: [
        {
          account_id: 'acct-1',
          pool_key: 'prov-a:acct-1:chat-msgs',
          pool_state: 'available',
          available: true,
        },
      ],
    },
  ];
  const stats = [
    {
      adapter_id: 'adapter-a',
      adapter_version: '1.0.0',
      attempts: 7,
      completed: 6,
      failed: 1,
      needs_user: 0,
      success_rate: 6 / 7,
      latency_ms: { median: 120, p95: 400 },
      failures_by_class: { provider_error: 1 },
      ui_drift: { consecutive_failures: 0, state: 'closed' },
    },
  ];
  const out = await withGateway(
    {
      '/v1/accounts': accounts,
      '/v1/accounts/acct-1/status': { state: 'ready' },
      '/v1/capabilities': capabilities,
      '/v1/stats/adapters': stats,
    },
    () => buildProgram().parseAsync(['node', 'allternit', 'subs', 'status']),
  );
  const rows = JSON.parse(out);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].account_id, 'acct-1');
  assert.deepEqual(rows[0].status, { state: 'ready' });
  assert.deepEqual(rows[0].pools, [
    {
      capability: 'chat.create',
      account_id: 'acct-1',
      pool_key: 'prov-a:acct-1:chat-msgs',
      pool_state: 'available',
      available: true,
    },
  ]);
  assert.deepEqual(rows[0].adapter_stats, [
    { adapter_id: 'adapter-a', adapter_version: '1.0.0', attempts: 7, success_rate: 6 / 7 },
  ]);
});

test('subs status degrades to empty pools/stats when the gateway predates those routes', async () => {
  const accounts = [
    {
      account_id: 'acct-1',
      provider: 'prov-a',
      label: 'Work',
      plan: null,
      session_health: 'ready',
      enabled: true,
    },
  ];
  const out = await withGateway(
    { '/v1/accounts': accounts, '/v1/accounts/acct-1/status': { state: 'ready' } },
    () => buildProgram().parseAsync(['node', 'allternit', 'subs', 'status']),
  );
  const rows = JSON.parse(out);
  assert.equal(rows.length, 1);
  assert.deepEqual(rows[0].pools, []);
  assert.deepEqual(rows[0].adapter_stats, []);
});

test('subs connect creates the account then drives the connect endpoint', async () => {
  const created = {
    account_id: 'acct-9',
    provider: 'prov-a',
    label: 'prov-a',
    plan: null,
    session_health: 'auth_required',
    enabled: true,
  };
  const calls: string[] = [];
  const out = await withGateway(
    {
      'GET /v1/accounts': [],
      'POST /v1/accounts': created,
      'POST /v1/accounts/acct-9/connect': created,
    },
    () => buildProgram().parseAsync(['node', 'allternit', 'subs', 'connect', 'prov-a']),
    calls,
  );
  assert.deepEqual(calls, [
    'GET /v1/accounts',
    'POST /v1/accounts',
    'POST /v1/accounts/acct-9/connect',
  ]);
  assert.equal(JSON.parse(out).account_id, 'acct-9');
});

test('subs connect reuses an existing enabled account instead of creating a duplicate', async () => {
  const existing = {
    account_id: 'acct-1',
    provider: 'prov-a',
    label: 'Work',
    plan: 'plus',
    session_health: 'ready',
    enabled: true,
  };
  const calls: string[] = [];
  const out = await withGateway(
    {
      'GET /v1/accounts': [existing],
      'POST /v1/accounts/acct-1/connect': existing,
    },
    () => buildProgram().parseAsync(['node', 'allternit', 'subs', 'connect', 'prov-a']),
    calls,
  );
  assert.deepEqual(calls, ['GET /v1/accounts', 'POST /v1/accounts/acct-1/connect']);
  assert.equal(JSON.parse(out).account_id, 'acct-1');
});

test('subs login reuses the account and opens the Firefox login window', async () => {
  const existing = {
    account_id: 'acct-1',
    provider: 'prov-a',
    label: 'Work',
    plan: null,
    session_health: 'auth_required',
    enabled: true,
  };
  const calls: string[] = [];
  const out = await withGateway(
    {
      'GET /v1/accounts': [existing],
      'POST /v1/accounts/acct-1/login': { account_id: 'acct-1', status: 'login_window_open', browser: 'firefox' },
    },
    () => buildProgram().parseAsync(['node', 'allternit', 'subs', 'login', 'prov-a']),
    calls,
  );
  assert.deepEqual(calls, ['GET /v1/accounts', 'POST /v1/accounts/acct-1/login']);
  assert.equal(JSON.parse(out).status, 'login_window_open');
});
