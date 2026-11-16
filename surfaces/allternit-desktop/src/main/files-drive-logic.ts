/**
 * Pure parts of the macOS Files drive wiring (plan step 23), kept apart from
 * Electron so they can be tested: which computers get a Finder location, and
 * which locations must be added again after the user turns the File
 * Provider on.
 */

/** `GET /api/v1/runtime-devices` row (runtime_pairing.rs). */
export interface RuntimeDeviceRow {
  id: string;
  name?: string;
  runtimeType?: string;
  hostname?: string;
}

/** `GET /api/v1/computers/paired` row (paired_computers.rs). */
export interface PairedComputerRow {
  id: string;
  name: string;
  mesh_ip?: string | null;
  shared?: boolean;
  cloud_computer?: boolean;
}

/** What the helper stores for the extension (`FilesSharedComputer`). */
export interface FilesComputer {
  id: string;
  name: string;
  runtimeType?: string;
}

export interface MeshOnlyComputer {
  id: string;
  meshIp: string;
}

/** allternit-api's port on the mesh (cloud-api `MESH_RUNTIME_PORT`). */
export const MESH_RUNTIME_PORT = 8013;

/** Runtime devices that are phones, not computers (same as iOS FilesDriveBridge). */
const PHONE_RUNTIME_TYPES = new Set(['ios', 'android']);

/**
 * The computers that get a Finder location:
 * - every runtime device except phones and this Mac itself (its files are
 *   already on this Mac): reached through the relay;
 * - every machine paired by pairing code that is the user's own, on the mesh,
 *   and not also listed as a runtime device: mesh only (SPEC section 2,
 *   "Computer ids"), reached through the app's mesh proxy. Cloud computers
 *   are runtime devices already, and the relay is their only way in.
 */
export function pickComputers(
  runtimes: RuntimeDeviceRow[],
  paired: PairedComputerRow[],
  selfRuntimeId: string | undefined,
): { computers: FilesComputer[]; meshOnly: MeshOnlyComputer[] } {
  const computers: FilesComputer[] = [];
  const names = new Set<string>();
  for (const runtime of runtimes) {
    if (!runtime.id || runtime.id === selfRuntimeId) continue;
    if (PHONE_RUNTIME_TYPES.has(runtime.runtimeType ?? '')) continue;
    const name = (runtime.name || runtime.hostname || 'Unnamed computer').trim();
    computers.push({ id: runtime.id, name, runtimeType: runtime.runtimeType ?? 'desktop' });
    names.add(name.toLowerCase());
  }
  const meshOnly: MeshOnlyComputer[] = [];
  for (const machine of paired) {
    if (machine.cloud_computer || machine.shared || !machine.mesh_ip) continue;
    const name = (machine.name || 'Unnamed computer').trim();
    // The same machine paired both ways shows once, through the relay.
    if (names.has(name.toLowerCase())) continue;
    computers.push({ id: machine.id, name, runtimeType: 'paired' });
    meshOnly.push({ id: machine.id, meshIp: machine.mesh_ip });
    names.add(name.toLowerCase());
  }
  return { computers, meshOnly };
}

export interface DomainStatus {
  computerId: string;
  name: string;
  userEnabled: boolean;
}

/**
 * Spike 6: a domain added while the user had the Allternit File Provider off
 * stays empty after they turn it on, until it is added again. So each
 * location is re-added exactly once after it is first seen on:
 * - off: remember it as off (nothing to do; the checklist asks for the switch);
 * - on, and added in this very sync: fresh, nothing to fix;
 * - on, and not yet known to be on (it was off, or it is the first time this
 *   app sees it): re-add it once.
 *
 * `seen` is what the app remembers between runs (computer id -> was on).
 */
export function planReenable(
  domains: DomainStatus[],
  seen: Record<string, boolean>,
  addedNow: string[] = [],
): { reenable: string[]; seen: Record<string, boolean> } {
  const added = new Set(addedNow);
  const next: Record<string, boolean> = {};
  const reenable: string[] = [];
  for (const domain of domains) {
    if (!domain.userEnabled) {
      next[domain.computerId] = false;
    } else if (added.has(domain.computerId) || seen[domain.computerId] === true) {
      next[domain.computerId] = true;
    } else {
      reenable.push(domain.computerId);
      next[domain.computerId] = true;
    }
  }
  return { reenable, seen: next };
}

/** `dev_mac_<hex>`, the contract's `DeviceId` (`^dev_[A-Za-z0-9_-]{4,64}$`). */
export function makeDeviceId(randomHex: string): string {
  return `dev_mac_${randomHex.replace(/[^0-9a-f]/gi, '').toLowerCase().slice(0, 32)}`;
}
