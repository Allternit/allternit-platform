/**
 * Finding a linked phone after its Wi-Fi address changed. Measured 2026-10-17:
 * the iPhone moved 10.0.0.90 → 10.0.0.252 between sessions, and the moto
 * moved 10.0.0.238 → 10.0.0.11 after a reboot (the app answered at the new
 * address while dozing and locked). Both drivers store the last LAN host;
 * that goes stale.
 *
 * The Phone Link app answers plain GET /whoami with its stable device id, so
 * Desktop can ask the subnet who has the phone it paired with. Matching is on
 * device_id only — a host that answers with another id is never trusted.
 */

import * as os from 'node:os';

export const DISCOVERY_PORT = 8200;
const MAX_PREFIX = 22; // scan at most a /22 (1022 hosts); skip larger subnets
const MAX_HOSTS = 1022;
const CONCURRENCY = 64;
const TIMEOUT_MS = 700;

export interface DiscoveryDeps {
  /** GET http://host/whoami (unauthenticated). */
  fetchWhoami(host: string, timeoutMs: number): Promise<{ device_id?: string } | null>;
  /** IPv4, not internal (os.networkInterfaces()). */
  lanInterfaces(): { address: string; netmask: string }[];
}

/** Discovery over the real network: fetch + os.networkInterfaces(). */
export const nodeDiscoveryDeps: DiscoveryDeps = {
  fetchWhoami: async (host, timeoutMs) => {
    try {
      const res = await fetch(`http://${host}/whoami`, { signal: AbortSignal.timeout(timeoutMs) });
      if (res.status !== 200) return null;
      return (await res.json()) as { device_id?: string };
    } catch {
      return null;
    }
  },
  lanInterfaces: () =>
    Object.values(os.networkInterfaces())
      .flat()
      .filter((i): i is os.NetworkInterfaceInfo => !!i && i.family === 'IPv4' && !i.internal)
      .map((i) => ({ address: i.address, netmask: i.netmask })),
};

export function netmaskToPrefix(netmask: string): number | null {
  const parts = netmask.split('.').map(Number);
  if (parts.length !== 4 || parts.some((p) => !Number.isInteger(p) || p < 0 || p > 255)) return null;
  const bits = parts.map((p) => p.toString(2).padStart(8, '0')).join('');
  if (/01/.test(bits)) return null; // non-contiguous mask
  return bits.replace(/0/g, '').length;
}

/** Every usable host address of a subnet, [] for networks larger than a /22. */
export function subnetHosts(address: string, netmask: string): string[] {
  const prefix = netmaskToPrefix(netmask);
  if (prefix === null || prefix < MAX_PREFIX || prefix > 30) return [];
  const parts = address.split('.').map(Number);
  if (parts.length !== 4 || parts.some((p) => !Number.isInteger(p) || p < 0 || p > 255)) return [];
  const ipNum = parts.reduce((acc, p) => acc * 256 + p, 0);
  const size = 2 ** (32 - prefix);
  const network = ipNum - (ipNum % size);
  const first = prefix >= 31 ? network : network + 1;
  const last = prefix >= 31 ? network + size - 1 : network + size - 2;
  const hosts: string[] = [];
  for (let h = first; h <= last && hosts.length < MAX_HOSTS; h++) {
    hosts.push(`${Math.floor(h / 2 ** 24) & 255}.${Math.floor(h / 2 ** 16) & 255}.${Math.floor(h / 2 ** 8) & 255}.${h & 255}`);
  }
  return hosts;
}

const bareHost = (hostAndPort: string): string => hostAndPort.replace(/:\d+$/, '');

/**
 * Where is the phone with this device id? The last known host is asked first,
 * then every host of each LAN subnet (skipping this computer's own
 * addresses). Returns the first `host:port` whose whoami reports `deviceId`.
 */
export async function findPhoneHost(
  deviceId: string,
  lastHost: string | undefined,
  deps: DiscoveryDeps,
  opts: { port?: number; concurrency?: number; timeoutMs?: number; signal?: AbortSignal } = {},
): Promise<string | null> {
  const port = opts.port ?? DISCOVERY_PORT;
  const timeoutMs = opts.timeoutMs ?? TIMEOUT_MS;
  const withPort = (host: string) => (/:\d+$/.test(host) ? host : `${host}:${port}`);

  const probe = async (host: string): Promise<string | null> => {
    const who = await deps.fetchWhoami(host, timeoutMs).catch(() => null);
    return who?.device_id === deviceId ? host : null;
  };

  if (lastHost) {
    const found = await probe(withPort(lastHost));
    if (found) return found;
  }

  const own = new Set(deps.lanInterfaces().map((i) => i.address));
  const seen = new Set(lastHost ? [withPort(lastHost), bareHost(withPort(lastHost))] : []);
  const targets: string[] = [];
  for (const iface of deps.lanInterfaces()) {
    for (const host of subnetHosts(iface.address, iface.netmask)) {
      if (own.has(host) || seen.has(host)) continue;
      seen.add(host);
      targets.push(withPort(host));
    }
  }

  let next = 0;
  let found: string | null = null;
  const workers = Array.from({ length: Math.max(1, Math.min(opts.concurrency ?? CONCURRENCY, targets.length)) }, async () => {
    while (next < targets.length && !found) {
      if (opts.signal?.aborted) return;
      const hit = await probe(targets[next++]);
      if (hit) found = hit;
    }
  });
  await Promise.all(workers);
  return found;
}

/**
 * One pass over every LAN subnet, recording which device id answers where.
 * Several linked phones can be looked up from one scan instead of one each.
 */
export async function scanLan(
  deps: DiscoveryDeps,
  opts: { port?: number; concurrency?: number; timeoutMs?: number } = {},
): Promise<Map<string, string>> {
  const port = opts.port ?? DISCOVERY_PORT;
  const timeoutMs = opts.timeoutMs ?? TIMEOUT_MS;
  const own = new Set(deps.lanInterfaces().map((i) => i.address));
  const targets: string[] = [];
  const seen = new Set<string>();
  for (const iface of deps.lanInterfaces()) {
    for (const host of subnetHosts(iface.address, iface.netmask)) {
      if (own.has(host) || seen.has(host)) continue;
      seen.add(host);
      targets.push(`${host}:${port}`);
    }
  }
  const found = new Map<string, string>();
  let next = 0;
  const workers = Array.from({ length: Math.max(1, Math.min(opts.concurrency ?? CONCURRENCY, targets.length)) }, async () => {
    while (next < targets.length) {
      const host = targets[next++];
      const who = await deps.fetchWhoami(host, timeoutMs).catch(() => null);
      if (typeof who?.device_id === 'string' && !found.has(who.device_id)) found.set(who.device_id, host);
    }
  });
  await Promise.all(workers);
  return found;
}
