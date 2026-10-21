/**
 * Allternit cloud phones (redroid on a fleet host, cloud-api
 * `GET /api/v1/cloud-phones`) brought into Phone Link.
 *
 * A running cloud phone publishes adb (and its Phone Link app port) on the
 * host's mesh address. Desktop is not on the mesh as a system VPN, so each
 * address goes through a MeshManager loopback proxy, then
 * PhoneDeviceManager.addByAddress saves it like any other phone. Phones the
 * cloud no longer lists are forgotten; nothing is forgotten when the list
 * could not be read (signed out, offline, or cloud phones off on the server).
 *
 * The server answers 503 until an operator turns cloud phones on
 * (CLOUD_PHONES_ENABLED), so for every account today this is a quiet no-op.
 */

import type { AddByAddressOptions, AddByAddressResult } from './phone-device-manager.js';

export interface CloudPhone {
  id: string;
  label: string | null;
  status: 'provisioning' | 'running' | 'stopped' | 'error' | 'deleted';
  plan: string;
  adbAddress: string | null;
  linkAddress: string | null;
  meshIp: string | null;
  adbPort: number | null;
  lastError: string | null;
  createdAt: string;
}

export type CloudPhonesSnapshot =
  | { available: true; phones: CloudPhone[] }
  | { available: false; reason: 'signed_out' | 'not_available' | 'error'; message?: string; phones: [] };

export interface CloudPhonesDeps {
  /** cloud-api base URL, no trailing slash. */
  baseUrl: string;
  /** Desktop's device access token; null when signed out. */
  accessToken: () => Promise<string | null>;
  fetch?: typeof fetch;
}

/** Read the signed-in account's cloud phones. Never throws. */
export async function fetchCloudPhones(deps: CloudPhonesDeps): Promise<CloudPhonesSnapshot> {
  const token = await deps.accessToken().catch(() => null);
  if (!token) return { available: false, reason: 'signed_out', phones: [] };
  try {
    const res = await (deps.fetch ?? fetch)(`${deps.baseUrl}/api/v1/cloud-phones`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(15_000),
    });
    const body = (await res.json().catch(() => ({}))) as { phones?: CloudPhone[]; message?: string; error?: string };
    if (res.status === 503) return { available: false, reason: 'not_available', message: body.message, phones: [] };
    if (!res.ok) return { available: false, reason: 'error', message: body.message ?? body.error ?? `HTTP ${res.status}`, phones: [] };
    return { available: true, phones: Array.isArray(body.phones) ? body.phones : [] };
  } catch (error) {
    return { available: false, reason: 'error', message: error instanceof Error ? error.message : String(error), phones: [] };
  }
}

/** The slice of PhoneDeviceManager the sync needs (kept narrow for tests). */
export interface CloudPhoneTarget {
  addByAddress(host: string, port: number, label: string | undefined, options: AddByAddressOptions): Promise<AddByAddressResult>;
  cloudPhoneIds(): string[];
  forgetCloudPhone(cloudPhoneId: string): void;
  list(): { id: string; state: string; serial: string }[];
}

export interface CloudPhoneSyncDeps extends CloudPhonesDeps {
  /** Loopback `127.0.0.1:<port>` for a mesh `100.x:port` (MeshManager.loopbackFor). */
  loopbackFor: (meshTarget: string) => Promise<string>;
  logger?: (message: string) => void;
}

function splitHostPort(value: string): { host: string; port: number } | null {
  const match = /^(.+):(\d{1,5})$/.exec(value);
  return match ? { host: match[1], port: Number(match[2]) } : null;
}

/**
 * Add every running cloud phone by address (skipping ones already online) and
 * forget saved cloud phones the account no longer has. Returns the snapshot.
 */
export async function syncCloudPhones(manager: CloudPhoneTarget, deps: CloudPhoneSyncDeps): Promise<CloudPhonesSnapshot> {
  const snapshot = await fetchCloudPhones(deps);
  if (!snapshot.available) return snapshot;

  const live = new Set(snapshot.phones.map((p) => p.id));
  for (const id of manager.cloudPhoneIds()) {
    if (!live.has(id)) manager.forgetCloudPhone(id);
  }

  const online = new Set(manager.list().filter((p) => p.state === 'online').map((p) => p.id));
  for (const phone of snapshot.phones) {
    if (phone.status !== 'running' || !phone.adbAddress) continue;
    if (online.has(`cloud-${phone.id}`)) continue;
    try {
      const loopback = splitHostPort(await deps.loopbackFor(phone.adbAddress));
      if (!loopback) continue;
      const result = await manager.addByAddress(loopback.host, loopback.port, phone.label ?? 'Cloud phone', {
        kind: 'cloud',
        cloudPhoneId: phone.id,
        meshTarget: phone.adbAddress,
      });
      if (!result.ok) deps.logger?.(`[cloud-phones] ${phone.id}: ${result.error}`);
    } catch (error) {
      deps.logger?.(`[cloud-phones] ${phone.id}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return snapshot;
}
