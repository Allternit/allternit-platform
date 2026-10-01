/**
 * First-boot sign-in for an Allternit cloud computer (provisioned lane).
 *
 * When cloud-api provisions a paying user's computer it writes a one-time
 * bootstrap file through cloud-init (contract:
 * infrastructure/provisioned-instance/README.md):
 *
 *   /etc/allternit/bootstrap.json
 *   { "api": "https://api.allternit.com", "token": "...",
 *     "instance_id": "...", "user_id": "..." }
 *
 * The Desktop app redeems it with the normal runtime pairing (same keys, same
 * exchange), sending the token so cloud-api approves the pairing for that
 * user without a browser. The file is deleted only after the exchange
 * succeeds, so a crash in between can retry while the token is still valid.
 */
import { existsSync, readFileSync, unlinkSync } from 'node:fs';

export const DEFAULT_BOOTSTRAP_PATH = '/etc/allternit/bootstrap.json';

export interface ProvisionedBootstrap {
  api: string;
  token: string;
  instanceId: string;
  userId: string;
  path: string;
}

/** Where the bootstrap file lives (overridable for tests and odd images). */
export function bootstrapPath(env: NodeJS.ProcessEnv = process.env): string {
  return env.ALLTERNIT_BOOTSTRAP_FILE?.trim() || DEFAULT_BOOTSTRAP_PATH;
}

/**
 * The bootstrap this computer was provisioned with, or null when there is
 * none or it's malformed (a malformed file is never fatal: the app falls back
 * to the normal sign-in).
 */
export function readProvisionedBootstrap(path = bootstrapPath()): ProvisionedBootstrap | null {
  if (!existsSync(path)) return null;
  try {
    const raw = JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>;
    const api = typeof raw.api === 'string' ? raw.api.trim().replace(/\/+$/, '') : '';
    const token = typeof raw.token === 'string' ? raw.token.trim() : '';
    const instanceId = typeof raw.instance_id === 'string' ? raw.instance_id.trim() : '';
    const userId = typeof raw.user_id === 'string' ? raw.user_id.trim() : '';
    if (!token || !instanceId || !/^https:\/\//.test(api)) return null;
    return { api, token, instanceId, userId, path };
  } catch {
    return null;
  }
}

/**
 * True on an Allternit cloud computer. The image sets ALLTERNIT_PROVISIONED=1
 * for the Desktop session, which outlives the single-use bootstrap file.
 */
export function isProvisionedMode(env: NodeJS.ProcessEnv = process.env, path = bootstrapPath(env)): boolean {
  return env.ALLTERNIT_PROVISIONED === '1' || readProvisionedBootstrap(path) !== null;
}

/** True on an Allternit cloud computer that still has its first-boot sign-in to do. */
export function isProvisionedComputer(path = bootstrapPath()): boolean {
  return readProvisionedBootstrap(path) !== null;
}

/** Remove the bootstrap after a successful exchange; the token is single use. */
export function consumeProvisionedBootstrap(bootstrap: ProvisionedBootstrap): void {
  try {
    unlinkSync(bootstrap.path);
  } catch {
    // Already gone, or read-only: cloud-api has consumed the token either way.
  }
}

/** The pairing request a provisioned computer sends (runtimeType is decided server-side too). */
export function provisionedPairingBody(base: Record<string, unknown>, bootstrap: ProvisionedBootstrap): Record<string, unknown> {
  return {
    ...base,
    name: 'Allternit cloud computer',
    runtimeType: 'provisioned',
    bootstrapToken: bootstrap.token,
    instanceId: bootstrap.instanceId,
  };
}
