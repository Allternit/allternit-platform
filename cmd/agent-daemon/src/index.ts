import { generateKeyPairSync, sign } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { hostname, homedir, platform, arch } from 'node:os';
import { dirname, join } from 'node:path';
import WebSocket, { type RawData } from 'ws';
import { discoverAgentClis } from './discovery';
import { consumeProvisionedBootstrap, pairWithBootstrap, readProvisionedBootstrap } from './provisioned';

const CLOUD_API_URL = (process.env.ALLTERNIT_CLOUD_API_URL || 'https://api.allternit.com').replace(/\/$/, '');
const RUNTIME_NAME = process.env.ALLTERNIT_RUNTIME_NAME || `${hostname()} VPS`;
const IDENTITY_PATH = process.env.ALLTERNIT_RUNTIME_IDENTITY_PATH
  || join(homedir(), '.config', 'allternit', 'runtime-identity.json');
const HEARTBEAT_INTERVAL_MS = 30_000;
const ROTATION_SKEW_MS = 7 * 24 * 60 * 60 * 1000;
const LOCAL_GATEWAY_URL = (process.env.ALLTERNIT_GATEWAY_URL || 'http://127.0.0.1:8013').replace(/\/$/, '');

type RuntimeIdentity = {
  runtimeId: string;
  userId: string;
  userEmail: string;
  organizationId?: string;
  deviceToken: string;
  expiresAt: string;
  capabilities: string[];
  privateKeyPem: string;
  publicKey: string;
};

type PairingStart = {
  pairingId: string;
  deviceCode: string;
  userCode: string;
  challenge: string;
  verificationUrl: string;
  pollIntervalSeconds: number;
};

let identity: RuntimeIdentity | null = null;
let relay: WebSocket | null = null;
let relayRetryMs = 1_000;
const relayLocalSockets = new Map<string, WebSocket>();

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function loadIdentity(): Promise<RuntimeIdentity | null> {
  try {
    const parsed = JSON.parse(await readFile(IDENTITY_PATH, 'utf8')) as RuntimeIdentity;
    if (!parsed.runtimeId || !parsed.deviceToken || !parsed.privateKeyPem) throw new Error('identity is incomplete');
    return parsed;
  } catch (error: any) {
    if (error?.code === 'ENOENT') return null;
    try {
      await rename(IDENTITY_PATH, `${IDENTITY_PATH}.invalid-${Date.now()}`);
    } catch {
      // A read-only or already-removed invalid file will simply be replaced.
    }
    return null;
  }
}

async function saveIdentity(next: RuntimeIdentity): Promise<void> {
  await mkdir(dirname(IDENTITY_PATH), { recursive: true, mode: 0o700 });
  await writeFile(IDENTITY_PATH, JSON.stringify(next, null, 2), { mode: 0o600 });
  identity = next;
}

async function beginPairing(): Promise<RuntimeIdentity> {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const publicDer = publicKey.export({ type: 'spki', format: 'der' });
  const publicKeyRaw = publicDer.subarray(publicDer.length - 32).toString('base64url');
  const privateKeyPem = privateKey.export({ type: 'pkcs8', format: 'pem' }).toString();

  // An Allternit cloud computer pairs itself from the bootstrap its
  // provisioning left behind (same contract as the Desktop app).
  const bootstrap = await readProvisionedBootstrap();
  if (bootstrap) try {
    console.log('[AgentDaemon] Provisioned computer: pairing with the first-boot bootstrap…');
    const payload = await pairWithBootstrap(bootstrap, {
      hostname: hostname(),
      platform: `${platform()}-${arch()}`,
      version: process.env.npm_package_version || '0.1.0',
      publicKey: publicKeyRaw,
      capabilities: [
        'runtime:connect',
        'runtime:execute',
        'runtime:files',
        'runtime:terminal',
        'runtime:remote_control',
        'providers:connect',
        'providers:use',
      ],
    }, {
      fetch,
      sign: (message) => sign(null, Buffer.from(message), privateKeyPem).toString('base64url'),
      sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    });
    const next: RuntimeIdentity = {
      runtimeId: payload.runtimeId as string,
      userId: payload.userId as string,
      userEmail: payload.userEmail as string,
      organizationId: payload.organizationId as string | undefined,
      deviceToken: payload.deviceToken as string,
      expiresAt: payload.expiresAt as string,
      capabilities: (payload.capabilities as string[]) || [],
      privateKeyPem,
      publicKey: publicKeyRaw,
    };
    await saveIdentity(next);
    await consumeProvisionedBootstrap(bootstrap);
    console.log(`[AgentDaemon] Cloud computer paired as ${next.userEmail} (${next.runtimeId}).`);
    return next;
  } catch (error) {
    // Expired or rejected token: fall back to the other pairing modes.
    console.warn('[AgentDaemon] Bootstrap pairing failed; falling back:', (error as Error).message);
  }

  if (process.env.ALLTERNIT_PAIRING_MODE === 'hosted_auto') {
    return beginHostedPairing(publicKeyRaw, privateKeyPem);
  }

  const clis = discoverAgentClis();
  const response = await fetch(`${CLOUD_API_URL}/api/v1/runtime-pairings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: RUNTIME_NAME,
      runtimeType: 'vps',
      hostname: hostname(),
      platform: `${platform()}-${arch()}`,
      version: process.env.npm_package_version || '0.1.0',
      publicKey: publicKeyRaw,
      capabilities: [
        'runtime:connect',
        'runtime:execute',
        'runtime:files',
        'runtime:terminal',
        'runtime:remote_control',
        'providers:connect',
        'providers:use',
      ],
      metadata: { agentClis: clis.map((cli) => cli.alias) },
    }),
  });
  if (!response.ok) throw new Error(`Unable to begin runtime pairing (${response.status})`);
  const pairing = await response.json() as PairingStart;

  console.log('');
  console.log('Pair this VPS with Allternit:');
  console.log(`  ${pairing.verificationUrl}`);
  console.log(`  Code: ${pairing.userCode}`);
  console.log('');
  console.log('Waiting for approval…');

  const message = `allternit-runtime-pairing:${pairing.pairingId}:${pairing.challenge}`;
  const signature = sign(null, Buffer.from(message), privateKeyPem).toString('base64url');
  const pollInterval = Math.max(pairing.pollIntervalSeconds || 2, 2) * 1000;

  for (;;) {
    const exchange = await fetch(`${CLOUD_API_URL}/api/v1/runtime-pairings/exchange`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        pairingId: pairing.pairingId,
        deviceCode: pairing.deviceCode,
        signature,
      }),
    });
    if (exchange.status === 428) {
      await wait(pollInterval);
      continue;
    }
    const payload = await exchange.json().catch(() => ({})) as any;
    if (!exchange.ok) throw new Error(payload.message || payload.error || `Pairing failed (${exchange.status})`);
    const next: RuntimeIdentity = {
      runtimeId: payload.runtimeId,
      userId: payload.userId,
      userEmail: payload.userEmail,
      organizationId: payload.organizationId,
      deviceToken: payload.deviceToken,
      expiresAt: payload.expiresAt,
      capabilities: payload.capabilities || [],
      privateKeyPem,
      publicKey: publicKeyRaw,
    };
    await saveIdentity(next);
    console.log(`Paired as ${next.userEmail} (${next.runtimeId}).`);
    return next;
  }
}

async function beginHostedPairing(
  publicKeyRaw: string,
  privateKeyPem: string,
): Promise<RuntimeIdentity> {
  const instanceId = process.env.ALLTERNIT_HOSTED_INSTANCE_ID;
  const bootstrapToken = process.env.ALLTERNIT_HOSTED_BOOTSTRAP_TOKEN;
  if (!instanceId || !bootstrapToken) {
    throw new Error('Hosted pairing requires ALLTERNIT_HOSTED_INSTANCE_ID and ALLTERNIT_HOSTED_BOOTSTRAP_TOKEN');
  }

  console.log('[AgentDaemon] Starting hosted auto-pairing…');
  const response = await fetch(`${CLOUD_API_URL}/api/v1/runtime-pairings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: RUNTIME_NAME,
      runtimeType: 'hosted',
      hostname: hostname(),
      platform: `${platform()}-${arch()}`,
      version: process.env.npm_package_version || '0.1.0',
      publicKey: publicKeyRaw,
      capabilities: [
        'runtime:connect',
        'runtime:execute',
        'runtime:files',
        'runtime:terminal',
        'runtime:remote_control',
        'providers:connect',
        'providers:use',
      ],
      hostedInstanceId: instanceId,
      hostedBootstrapToken: bootstrapToken,
    }),
  });
  if (!response.ok) throw new Error(`Unable to begin hosted pairing (${response.status})`);
  const pairing = await response.json() as PairingStart;

  const message = `allternit-runtime-pairing:${pairing.pairingId}:${pairing.challenge}`;
  const signature = sign(null, Buffer.from(message), privateKeyPem).toString('base64url');

  const exchange = await fetch(`${CLOUD_API_URL}/api/v1/runtime-pairings/exchange`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      pairingId: pairing.pairingId,
      deviceCode: pairing.deviceCode,
      signature,
    }),
  });
  const payload = await exchange.json().catch(() => ({})) as any;
  if (!exchange.ok) throw new Error(payload.message || payload.error || `Hosted pairing exchange failed (${exchange.status})`);
  const next: RuntimeIdentity = {
    runtimeId: payload.runtimeId,
    userId: payload.userId,
    userEmail: payload.userEmail,
    organizationId: payload.organizationId,
    deviceToken: payload.deviceToken,
    expiresAt: payload.expiresAt,
    capabilities: payload.capabilities || [],
    privateKeyPem,
    publicKey: publicKeyRaw,
  };
  await saveIdentity(next);
  console.log(`[AgentDaemon] Hosted runtime paired as ${next.userEmail} (${next.runtimeId}).`);
  return next;
}

async function heartbeat(): Promise<void> {
  if (!identity) return;
  const response = await fetch(
    `${CLOUD_API_URL}/api/v1/runtime-devices/${encodeURIComponent(identity.runtimeId)}/heartbeat`,
    { method: 'POST', headers: { Authorization: `Bearer ${identity.deviceToken}` } },
  );
  if (response.status === 401 || response.status === 403) {
    throw new Error('This VPS runtime was revoked. Remove the identity file and pair it again.');
  }
  if (!response.ok) throw new Error(`Heartbeat failed (${response.status})`);
}

async function rotateIfNeeded(): Promise<void> {
  if (!identity || new Date(identity.expiresAt).getTime() - Date.now() > ROTATION_SKEW_MS) return;
  const response = await fetch(
    `${CLOUD_API_URL}/api/v1/runtime-devices/${encodeURIComponent(identity.runtimeId)}/rotate`,
    { method: 'POST', headers: { Authorization: `Bearer ${identity.deviceToken}` } },
  );
  if (!response.ok) throw new Error(`Credential rotation failed (${response.status})`);
  const payload = await response.json() as { deviceToken: string; expiresAt: string };
  await saveIdentity({ ...identity, deviceToken: payload.deviceToken, expiresAt: payload.expiresAt });
  relay?.close();
}

function connectRelay(): void {
  if (!identity || relay) return;
  const relayUrl = new URL(`${CLOUD_API_URL}/api/v1/runtime-relay/connect/${encodeURIComponent(identity.runtimeId)}`);
  relayUrl.protocol = relayUrl.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(relayUrl);
  relay = socket;
  socket.on('open', () => {
    if (!identity) return socket.close();
    socket.send(JSON.stringify({
      type: 'authenticate',
      runtime_id: identity.runtimeId,
      device_token: identity.deviceToken,
    }));
  });
  socket.on('message', (data) => {
    let message: any;
    try { message = JSON.parse(data.toString()); } catch { return; }
    if (message.type === 'authenticated') {
      relayRetryMs = 1_000;
      console.log('[AgentDaemon] Secure runtime relay connected.');
    } else if (message.type === 'request') {
      void handleRelayRequest(socket, message);
    } else if (message.type === 'socket_open') {
      handleRelaySocketOpen(socket, message);
    } else if (message.type === 'socket_data') {
      handleRelaySocketData(message);
    } else if (message.type === 'socket_close') {
      handleRelaySocketClose(message);
    } else if (message.type === 'ping') {
      socket.send(JSON.stringify({ type: 'pong' }));
    }
  });
  socket.on('close', () => {
    if (relay === socket) relay = null;
    for (const local of relayLocalSockets.values()) local.close(1012, 'Relay disconnected');
    relayLocalSockets.clear();
    const delay = relayRetryMs;
    relayRetryMs = Math.min(relayRetryMs * 2, 30_000);
    setTimeout(connectRelay, delay);
  });
  socket.on('error', (error) => console.error('[AgentDaemon] Relay:', error.message));
}

async function handleRelayRequest(socket: WebSocket, message: any): Promise<void> {
  const requestId = typeof message.request_id === 'string' ? message.request_id : '';
  const method = typeof message.method === 'string' ? message.method.toUpperCase() : 'GET';
  const requestPath = typeof message.path === 'string' ? message.path : '';
  if (!identity || !requestId || !requestPath.startsWith('/') || requestPath.includes('..') || requestPath.includes('://')) return;
  const allowedPrefixes = [
    '/api/', '/viz', '/sandbox', '/vm-session', '/rails', '/stream',
    '/terminal', '/mcp', '/platform', '/metrics', '/alabs', '/cowork',
    '/webhooks', '/status', '/health',
    '/ws', '/panes',
    // gizzi-code mirrors its entire route surface under /v1/* (pty, sessions,
    // events, instance, …) — BYO boxes are unreachable without this.
    '/v1/',
  ];
  if (!allowedPrefixes.some((prefix) => requestPath.startsWith(prefix))) return;
  try {
    const headers = new Headers(message.headers || {});
    // Prefer the caller's own Clerk JWT when the envelope carries one so the
    // local gateway authenticates as the end user; fall back to the device
    // token (and its identity headers) when it doesn't.
    if (!headers.has('Authorization')) {
      headers.set('Authorization', `Bearer ${identity.deviceToken}`);
    }
    headers.set('X-Allternit-Desktop-Access-Token', identity.deviceToken);
    headers.set('X-Allternit-User-Id', identity.userId);
    headers.set('X-Allternit-User-Email', identity.userEmail);
    if (identity.organizationId) headers.set('X-Allternit-Tenant-Id', identity.organizationId);
    const body = method === 'GET' || method === 'HEAD' || !message.body
      ? undefined
      : message.body_encoding === 'base64'
        ? Buffer.from(message.body, 'base64')
        : Buffer.from(message.body, 'utf8');
    const response = await fetch(`${LOCAL_GATEWAY_URL}${requestPath}`, { method, headers, body });
    const responseHeaders: Record<string, string> = {};
    for (const name of ['content-type', 'cache-control', 'content-disposition', 'etag', 'last-modified', 'x-request-id']) {
      const value = response.headers.get(name);
      if (value) responseHeaders[name] = value;
    }
    socket.send(JSON.stringify({
      type: 'response_start',
      request_id: requestId,
      status: response.status,
      headers: responseHeaders,
    }));
    if (response.body) {
      const reader = response.body.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (value?.byteLength) {
          socket.send(JSON.stringify({
            type: 'response_chunk',
            request_id: requestId,
            body: Buffer.from(value).toString('base64'),
            body_encoding: 'base64',
          }));
        }
      }
    }
    socket.send(JSON.stringify({ type: 'response_end', request_id: requestId }));
  } catch (error) {
    socket.send(JSON.stringify({
      type: 'response_start',
      request_id: requestId,
      status: 502,
      headers: { 'content-type': 'application/json' },
    }));
    socket.send(JSON.stringify({
      type: 'response_chunk',
      request_id: requestId,
      body: Buffer.from(JSON.stringify({ error: 'runtime_proxy_error', message: String(error) })).toString('base64'),
      body_encoding: 'base64',
    }));
    socket.send(JSON.stringify({ type: 'response_end', request_id: requestId }));
  }
}

function handleRelaySocketOpen(relaySocket: WebSocket, message: any): void {
  const socketId = typeof message.socket_id === 'string' ? message.socket_id : '';
  const requestPath = typeof message.path === 'string' ? message.path : '';
  const allowedPrefixes = [
    '/api/', '/viz', '/sandbox', '/vm-session', '/rails', '/stream',
    '/terminal', '/mcp', '/platform', '/metrics', '/alabs', '/cowork',
    '/webhooks', '/ws', '/panes', '/status', '/health',
  ];
  if (!identity || !socketId || !requestPath.startsWith('/') || requestPath.includes('..')
    || requestPath.includes('://') || !allowedPrefixes.some((prefix) => requestPath.startsWith(prefix))) return;
  relayLocalSockets.get(socketId)?.close();
  const local = new WebSocket(`${LOCAL_GATEWAY_URL.replace(/^http/, 'ws')}${requestPath}`, {
    headers: {
      Authorization: `Bearer ${identity.deviceToken}`,
      'X-Allternit-Desktop-Access-Token': identity.deviceToken,
      'X-Allternit-User-Id': identity.userId,
      'X-Allternit-User-Email': identity.userEmail,
      ...(identity.organizationId ? { 'X-Allternit-Tenant-Id': identity.organizationId } : {}),
    },
  });
  relayLocalSockets.set(socketId, local);
  local.on('open', () => {
    if (relaySocket.readyState === WebSocket.OPEN) {
      relaySocket.send(JSON.stringify({ type: 'socket_ready', socket_id: socketId }));
    }
  });
  local.on('message', (data: RawData, isBinary) => {
    if (relaySocket.readyState !== WebSocket.OPEN) return;
    const body = isBinary
      ? Buffer.from(data as Buffer).toString('base64')
      : (data as Buffer).toString();
    relaySocket.send(JSON.stringify({
      type: 'socket_data',
      socket_id: socketId,
      body,
      body_encoding: isBinary ? 'base64' : 'utf8',
    }));
  });
  local.on('close', (code, reason) => {
    relayLocalSockets.delete(socketId);
    if (relaySocket.readyState === WebSocket.OPEN) {
      relaySocket.send(JSON.stringify({ type: 'socket_close', socket_id: socketId, code, reason: reason.toString() }));
    }
  });
  local.on('error', (error) => {
    console.error('[AgentDaemon] Local runtime socket:', error.message);
    local.close(1011, 'Local runtime socket failed');
  });
}

function handleRelaySocketData(message: any): void {
  const local = relayLocalSockets.get(message.socket_id);
  if (!local || local.readyState !== WebSocket.OPEN) return;
  local.send(message.body_encoding === 'base64'
    ? Buffer.from(String(message.body || ''), 'base64')
    : String(message.body || ''));
}

function handleRelaySocketClose(message: any): void {
  const local = relayLocalSockets.get(message.socket_id);
  if (!local) return;
  relayLocalSockets.delete(message.socket_id);
  const code = Number(message.code) >= 1000 && Number(message.code) <= 4999 ? Number(message.code) : 1000;
  local.close(code, String(message.reason || ''));
}

async function main() {
  console.log('[AgentDaemon] Starting paired Allternit runtime…');
  console.log('[AgentDaemon] Cloud API:', CLOUD_API_URL);
  identity = await loadIdentity();
  if (!identity) identity = await beginPairing();

  await rotateIfNeeded();
  await heartbeat();
  console.log(`[AgentDaemon] ${RUNTIME_NAME} is online. Runtime ID: ${identity.runtimeId}`);
  connectRelay();

  setInterval(() => {
    void rotateIfNeeded()
      .then(() => heartbeat())
      .catch((error) => console.error('[AgentDaemon]', error instanceof Error ? error.message : error));
  }, HEARTBEAT_INTERVAL_MS);
}

main().catch((error) => {
  console.error('[AgentDaemon] Fatal:', error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
