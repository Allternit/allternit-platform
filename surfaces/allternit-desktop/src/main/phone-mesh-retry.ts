/**
 * A linked phone whose mesh join didn't finish at setup (`meshPending`)
 * stayed home-only until someone ran Phone Link setup again. When its helper
 * answers on the LAN, Desktop now finishes the join itself: the same steps
 * setup takes (GET /schema lists `mesh_control_url`, mint a single-use key,
 * one sealed `config` call, poll /whoami up to 60 s), then saves the mesh
 * address and clears the flag.
 *
 * At most one attempt per phone per [MESH_RETRY_EVERY_MS], one at a time,
 * one log line per attempt. The auth key goes straight to the phone and is
 * never logged.
 */

import { MESH_JOIN_TIMEOUT_MS, meshHostOf, schemaSupportsMeshConfig, type PhoneMeshWhoami } from './phone-link-mesh.js';

export const MESH_RETRY_EVERY_MS = 10 * 60_000;
const POLL_MS = 2_000;

export interface MeshPendingPhone {
  deviceId: string;
  platform: 'ios' | 'android';
  name?: string;
  /** Where the helper answers now (its saved LAN host). */
  host: string;
}

export interface MeshRetryDeps {
  /** Linked phones with `meshPending` and no mesh address yet. */
  pending: () => MeshPendingPhone[];
  fetchSchema: (host: string) => Promise<unknown | null>;
  whoami: (host: string) => Promise<PhoneMeshWhoami | null>;
  enroll: () => Promise<{ controlUrl: string; authKey: string }>;
  /** A sealed iCTRL call on the phone at `host`. */
  call: (phone: MeshPendingPhone, method: string, params: Record<string, unknown>) => Promise<unknown>;
  /** Save the mesh address and clear meshPending on the link record. */
  joined: (phone: MeshPendingPhone, meshHost: string) => void;
  log: (line: string) => void;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  everyMs?: number;
}

export function createMeshPendingRetry(deps: MeshRetryDeps): () => Promise<void> {
  const now = deps.now ?? Date.now;
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const every = deps.everyMs ?? MESH_RETRY_EVERY_MS;
  const last = new Map<string, number>();
  let running: Promise<void> | null = null;

  const attempt = async (phone: MeshPendingPhone): Promise<void> => {
    const who = phone.name || phone.deviceId.slice(0, 8);
    const said = (outcome: string) => deps.log(`[PhoneLink] mesh join retry ${who}: ${outcome}`);
    const first = await deps.whoami(phone.host).catch(() => null);
    if (!first || (first as { device_id?: string }).device_id !== phone.deviceId) return; // not answering here now: try again later, quietly
    const already = meshHostOf(first);
    if (already) {
      deps.joined(phone, already);
      return said(`already running at ${already}`);
    }
    if (!schemaSupportsMeshConfig(await deps.fetchSchema(phone.host).catch(() => null))) return said('this helper has no mesh support');
    let enrollment: { controlUrl: string; authKey: string };
    try {
      enrollment = await deps.enroll();
    } catch (error) {
      return said(`no mesh key (${error instanceof Error ? error.message : String(error)})`);
    }
    try {
      await deps.call(phone, 'config', { mesh_control_url: enrollment.controlUrl, mesh_auth_key: enrollment.authKey });
    } catch (error) {
      return said(`config failed (${error instanceof Error ? error.message : String(error)})`);
    }
    const deadline = now() + MESH_JOIN_TIMEOUT_MS;
    while (now() < deadline) {
      const meshHost = meshHostOf(await deps.whoami(phone.host).catch(() => null));
      if (meshHost) {
        deps.joined(phone, meshHost);
        return said(`joined at ${meshHost}`);
      }
      await sleep(POLL_MS);
    }
    said('did not report running in time');
  };

  const pass = async (): Promise<void> => {
    for (const phone of deps.pending()) {
      const at = last.get(phone.deviceId);
      if (at !== undefined && now() - at < every) continue;
      last.set(phone.deviceId, now());
      await attempt(phone).catch(() => {});
    }
  };

  return () => {
    running ??= pass().finally(() => {
      running = null;
    });
    return running;
  };
}
