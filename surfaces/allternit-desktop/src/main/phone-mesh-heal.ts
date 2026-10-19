/**
 * Self-heal for a phone's embedded mesh node (tsnet inside the Android Phone
 * Link app's :mesh process, or inside the iPhone runner).
 *
 * Seen live 2026-10-18: after a Headscale outage the phone's node still said
 * `running`, but every connection Desktop's mesh-node proxied to
 * `<phone 100.x>:8200` was reset until the app was force-stopped. When a
 * phone's mesh route fails like that but the phone still answers off the
 * mesh — over the LAN, or over the cable (adb forward / usbmux) — Desktop
 * asks it to restart its mesh node (`mesh_restart`, same node key and state
 * dir) and the caller retries the mesh route once.
 *
 * At most one attempt per phone per [MESH_HEAL_COOLDOWN_MS] (failed attempts
 * count, so a phone that can't be healed is not hammered); concurrent
 * callers for one phone share the attempt. One log line per attempt.
 */

import type { LinkedPhone } from './phone-link-health.js';
import type { AdbCall } from './phone-tools.js';

/** The helper's iCTRL port (ictrl-client ICTRL_PORT); a saved LAN host's port wins. */
const DEFAULT_HELPER_PORT = 8200;

export const MESH_HEAL_COOLDOWN_MS = 120_000;

/** A non-mesh address where the phone may answer, plus how to release it (a cable forward). */
export interface OffMeshRoute {
  host: string;
  via: 'lan' | 'cable';
  close?: () => Promise<void> | void;
}

/** What `mesh_restart` reported back. */
export interface MeshHealResult {
  meshState: string;
  /** `<100.x>:8200` once the rejoined node reports it, else null. */
  meshHost: string | null;
}

export type MeshHealFn = (phone: LinkedPhone, why: string) => Promise<MeshHealResult | null>;

export interface MeshHealDeps {
  /**
   * Off-mesh routes to try in order (each opened lazily; null = not
   * available now). Production: the saved LAN host, then the cable.
   */
  routes: (phone: LinkedPhone) => Array<() => Promise<OffMeshRoute | null>>;
  fetchWhoami: (host: string, timeoutMs: number) => Promise<{ device_id?: string } | null>;
  /** A sealed iCTRL call on the phone at `host`. */
  call: (phone: LinkedPhone, host: string, method: string, params: Record<string, unknown>, timeoutMs: number) => Promise<unknown>;
  log: (line: string) => void;
  /** A rejoined node reported a different mesh address: save it to the link record. */
  rememberMeshHost?: (phone: LinkedPhone, meshHost: string) => void;
  now?: () => number;
  cooldownMs?: number;
}

/** Seconds the phone waits for its rejoin before answering (Schema: default 20, max 45). */
const RESTART_WAIT_S = 20;

export function createMeshHealer(deps: MeshHealDeps): MeshHealFn {
  const now = deps.now ?? Date.now;
  const cooldown = deps.cooldownMs ?? MESH_HEAL_COOLDOWN_MS;
  const lastAttempt = new Map<string, number>();
  const inflight = new Map<string, Promise<MeshHealResult | null>>();
  const label = (phone: LinkedPhone) => phone.name || phone.deviceId.slice(0, 8);

  const attempt = async (phone: LinkedPhone, why: string): Promise<MeshHealResult | null> => {
    for (const open of deps.routes(phone)) {
      const route = await open().catch(() => null);
      if (!route) continue;
      try {
        const who = await deps.fetchWhoami(route.host, 2_000).catch(() => null);
        if (who?.device_id !== phone.deviceId) continue;
        try {
          const res = (await deps.call(phone, route.host, 'mesh_restart', { wait: RESTART_WAIT_S }, (RESTART_WAIT_S + 10) * 1000)) as {
            mesh_state?: unknown;
            mesh_host?: unknown;
          };
          const result: MeshHealResult = {
            meshState: typeof res?.mesh_state === 'string' ? res.mesh_state : 'unknown',
            meshHost: typeof res?.mesh_host === 'string' && res.mesh_host ? res.mesh_host : null,
          };
          deps.log(
            `[PhoneLink] mesh heal ${label(phone)}: ${why}; mesh_restart over ${route.via} -> ${result.meshState}` +
              (result.meshHost ? ` ${result.meshHost}` : ''),
          );
          if (result.meshHost && result.meshHost !== phone.meshHost) deps.rememberMeshHost?.(phone, result.meshHost);
          return result;
        } catch (error) {
          // An older helper answers unknown_method; a stopped mesh answers mesh_unavailable.
          deps.log(`[PhoneLink] mesh heal ${label(phone)}: ${why}; mesh_restart over ${route.via} failed: ${error instanceof Error ? error.message : String(error)}`);
          return null;
        }
      } finally {
        await Promise.resolve(route.close?.()).catch(() => {});
      }
    }
    deps.log(`[PhoneLink] mesh heal ${label(phone)}: ${why}; no LAN or cable route answered, can't restart its mesh`);
    return null;
  };

  return (phone, why) => {
    const running = inflight.get(phone.deviceId);
    if (running) return running;
    const last = lastAttempt.get(phone.deviceId);
    if (last !== undefined && now() - last < cooldown) return Promise.resolve(null);
    lastAttempt.set(phone.deviceId, now());
    const p = attempt(phone, why)
      .catch(() => null)
      .finally(() => inflight.delete(phone.deviceId));
    inflight.set(phone.deviceId, p);
    return p;
  };
}

/**
 * The production off-mesh routes for a linked phone, in order: its saved LAN
 * host, then the cable — an adb forward for an Android phone (only when adb
 * is already running: never downloaded just to heal), a usbmux loopback
 * forward for an iPhone. Each cable forward is removed after the attempt.
 */
export function phoneOffMeshRoutes(deps: {
  /** The adb serial recorded at cable setup, or null. */
  androidSerial: (deviceId: string) => string | null;
  /** The iPhone's UDID, or null. */
  iosUdid: (deviceId: string) => string | null;
  /** adb when it is already running, else null. */
  adb: () => Promise<AdbCall | null>;
  /** usbmux 127.0.0.1 forward to a device port (phone-usbmux startLoopbackForward). */
  usbForward?: (udid: string, port: number) => Promise<{ port: number; close(): Promise<void> }>;
}): (phone: LinkedPhone) => Array<() => Promise<OffMeshRoute | null>> {
  return (phone) => {
    const port = Number(phone.lanHost?.split(':').pop()) || DEFAULT_HELPER_PORT;
    const routes: Array<() => Promise<OffMeshRoute | null>> = [];
    if (phone.lanHost) {
      const host = phone.lanHost;
      routes.push(async () => ({ host, via: 'lan' }));
    }
    if (phone.platform === 'android') {
      routes.push(async () => {
        const serial = deps.androidSerial(phone.deviceId);
        const adb = serial ? await deps.adb() : null;
        if (!serial || !adb) return null;
        const fwd = await adb(['forward', 'tcp:0', `tcp:${port}`], { serial, timeoutMs: 5_000 });
        const local = Number(fwd.stdout.trim());
        if (fwd.code !== 0 || !Number.isInteger(local) || local <= 0) return null;
        return {
          host: `127.0.0.1:${local}`,
          via: 'cable',
          close: async () => {
            await adb(['forward', '--remove', `tcp:${local}`], { serial, timeoutMs: 5_000 }).catch(() => undefined);
          },
        };
      });
    } else if (deps.usbForward) {
      const usbForward = deps.usbForward;
      routes.push(async () => {
        const udid = deps.iosUdid(phone.deviceId);
        if (!udid) return null;
        const fwd = await usbForward(udid, port);
        return { host: `127.0.0.1:${fwd.port}`, via: 'cable', close: () => fwd.close() };
      });
    }
    return routes;
  };
}
