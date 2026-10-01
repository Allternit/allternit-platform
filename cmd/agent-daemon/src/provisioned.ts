/**
 * First-boot pairing for an Allternit cloud computer running headless (the
 * free, runtime-only profile). Same contract as the Desktop app
 * (surfaces/allternit-desktop/src/main/provisioned-bootstrap.ts) and
 * cloud-api's bootstrapToken path on /api/v1/runtime-pairings:
 *
 *   /etc/allternit/bootstrap.json  (written by provisioning, mode 0600)
 *   { "api": "https://api.allternit.com", "token": "...",
 *     "instance_id": "...", "user_id": "...", "expires_at": "..." }
 *
 * The token pre-approves the pairing for the instance's owner; the exchange
 * may answer 428 until that approval commits, so it is retried. The file is
 * removed only after a successful exchange (the token is single use).
 */
import { readFile, unlink } from 'node:fs/promises';

export const DEFAULT_BOOTSTRAP_PATH = '/etc/allternit/bootstrap.json';

export interface ProvisionedBootstrap {
  api: string;
  token: string;
  instanceId: string;
  userId: string;
  path: string;
}

export function bootstrapPath(env: NodeJS.ProcessEnv = process.env): string {
  return env.ALLTERNIT_BOOTSTRAP_FILE?.trim() || DEFAULT_BOOTSTRAP_PATH;
}

export async function readProvisionedBootstrap(path = bootstrapPath()): Promise<ProvisionedBootstrap | null> {
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(await readFile(path, 'utf8')) as Record<string, unknown>;
  } catch {
    return null;
  }
  const api = typeof raw.api === 'string' ? raw.api.trim().replace(/\/+$/, '') : '';
  const token = typeof raw.token === 'string' ? raw.token.trim() : '';
  const instanceId = typeof raw.instance_id === 'string' ? raw.instance_id.trim() : '';
  const userId = typeof raw.user_id === 'string' ? raw.user_id.trim() : '';
  if (!token || !instanceId || !/^https:\/\//.test(api)) return null;
  return { api, token, instanceId, userId, path };
}

export async function consumeProvisionedBootstrap(bootstrap: ProvisionedBootstrap): Promise<void> {
  await unlink(bootstrap.path).catch(() => {});
}

export interface PairingStart {
  pairingId: string;
  deviceCode: string;
  challenge: string;
  pollIntervalSeconds?: number;
}

export interface ProvisionedPairingDeps {
  fetch: typeof fetch;
  sign: (message: string) => string;
  sleep: (ms: number) => Promise<void>;
  maxAttempts?: number;
}

/**
 * Start a pre-approved pairing with the bootstrap token and exchange it for
 * the device credential. Returns the exchange payload.
 */
export async function pairWithBootstrap(
  bootstrap: ProvisionedBootstrap,
  body: Record<string, unknown>,
  deps: ProvisionedPairingDeps,
): Promise<Record<string, unknown>> {
  const start = await deps.fetch(`${bootstrap.api}/api/v1/runtime-pairings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Allternit-Bootstrap-Token': bootstrap.token },
    body: JSON.stringify({
      ...body,
      name: 'Allternit cloud computer',
      runtimeType: 'provisioned',
      bootstrapToken: bootstrap.token,
      instanceId: bootstrap.instanceId,
    }),
  });
  if (!start.ok) throw new Error(`Bootstrap pairing was refused (${start.status})`);
  const pairing = await start.json() as PairingStart;
  const signature = deps.sign(`allternit-runtime-pairing:${pairing.pairingId}:${pairing.challenge}`);
  const interval = Math.max(pairing.pollIntervalSeconds || 2, 1) * 1000;
  const attempts = deps.maxAttempts ?? 60;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    const exchange = await deps.fetch(`${bootstrap.api}/api/v1/runtime-pairings/exchange`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pairingId: pairing.pairingId, deviceCode: pairing.deviceCode, signature }),
    });
    if (exchange.status === 428 || exchange.status >= 500) {
      await deps.sleep(interval);
      continue;
    }
    const payload = await exchange.json().catch(() => ({})) as Record<string, unknown>;
    if (!exchange.ok) {
      throw new Error(String(payload.message || payload.error || `Bootstrap exchange failed (${exchange.status})`));
    }
    return payload;
  }
  throw new Error('Bootstrap pairing was not approved in time');
}
