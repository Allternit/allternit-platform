/**
 * Phone Link × the Allternit mesh. On by default since 2026-10-17, when the
 * production Headscale policy went to per-customer isolation
 * (autogroup:self; verified live: another account's node can't reach a
 * customer's phone). ALLTERNIT_PHONE_LINK_MESH=0 turns it off.
 *
 * Cable setup enrolls the phone on the user's mesh, and the tool lane falls
 * back to it when the phone isn't on the computer's LAN, so the phone works
 * on other Wi-Fi and on cellular.
 */

/**
 * True when Phone Link may put phones on the mesh. Frozen at module load;
 * tests that need the on-path set ALLTERNIT_PHONE_LINK_MESH=1 via
 * `vi.hoisted` before importing the drivers.
 */
export const PHONE_LINK_MESH_ENABLED = process.env.ALLTERNIT_PHONE_LINK_MESH !== '0';

/**
 * Mesh credentials source handed to the setup drivers (built from
 * MeshManager in phone-link-ipc.ts). `enroll` rejects when Desktop is not
 * signed in to an Allternit account or minting the key fails — drivers treat
 * that as non-fatal.
 */
export interface PhoneLinkMeshDep {
  /** Mint a single-use mesh pre-auth key for this phone's owner. */
  enroll(): Promise<{ controlUrl: string; authKey: string }>;
  /**
   * A loopback `127.0.0.1:<port>` reaching a raw TCP mesh address
   * (MeshManager.loopbackFor). Used to reach a cloud phone's published
   * Phone Link port during setup. Rejects when the mesh is unavailable.
   */
  loopbackFor?(target: string): Promise<string>;
}

/** The one plain sentence setup shows when the mesh join didn't finish. */
export const MESH_PENDING_DETAIL =
  'Connected on this Wi-Fi. To use it away from home too, plug it in and run setup again.';

/** How long setup waits for the phone to report mesh_state 'running' (join measured < 10 s live). */
export const MESH_JOIN_TIMEOUT_MS = 60_000;
const MESH_JOIN_POLL_MS = 2000;

/** The mesh fields of the helpers' unauthenticated GET /whoami (Android 0.1.0 live-verified 2026-10-17). */
export interface PhoneMeshWhoami {
  device_id?: string;
  mesh_state?: string;
  /** `<100.x>:8200` while joined, else null (absent on runners without mesh support). */
  mesh_host?: string | null;
}

/**
 * The mesh address from a /whoami body, accepted only once the node actually
 * reports `mesh_state: 'running'` with a non-empty `mesh_host`. The helper
 * states are `stopped|starting|running|needs_login|error:<reason>`.
 */
export function meshHostOf(who: PhoneMeshWhoami | null | undefined): string | null {
  return who?.mesh_state === 'running' && typeof who.mesh_host === 'string' && who.mesh_host.length > 0
    ? who.mesh_host
    : null;
}

/**
 * Poll GET /whoami at `host` until the phone reports its mesh address or the
 * 60 s join budget runs out (resolves null — never throws). `check` lets the
 * drivers honour cancellation between polls.
 */
export async function pollMeshHost(
  fetchWhoami: (host: string, timeoutMs: number) => Promise<PhoneMeshWhoami | null>,
  host: string,
  opts: { check?: () => void; sleep: (ms: number) => Promise<void>; now?: () => number },
): Promise<string | null> {
  const now = opts.now ?? Date.now;
  const deadline = now() + MESH_JOIN_TIMEOUT_MS;
  while (now() < deadline) {
    opts.check?.();
    const who = await fetchWhoami(host, 2000).catch(() => null);
    const meshHost = meshHostOf(who);
    if (meshHost) return meshHost;
    await opts.sleep(MESH_JOIN_POLL_MS);
  }
  return null;
}

/**
 * Feature-detect mesh support on an iCTRL runner from GET /schema: true only
 * when the `config` method lists a param named `mesh_control_url`. The schema
 * document shape is the iCTRL catalogue the helpers serve —
 * `{ methods: [{ name, description, params: [{ name, type, required, description }], returns }] }`
 * (Schema.kt in the Phone Link Android app, which ports iCTRLKit's server).
 * A runner without mesh support simply has no such param; anything that isn't
 * that shape is treated as "not supported", never as an error.
 */
export function schemaHasConfigParam(schema: unknown, methodName: string, paramName: string): boolean {
  if (!schema || typeof schema !== 'object') return false;
  const methods = (schema as { methods?: unknown }).methods;
  if (!Array.isArray(methods)) return false;
  for (const method of methods) {
    if (!method || typeof method !== 'object') continue;
    const candidate = method as { name?: unknown; params?: unknown };
    if (candidate.name !== methodName) continue;
    if (!Array.isArray(candidate.params)) return false;
    return candidate.params.some(
      (param) => !!param && typeof param === 'object' && (param as { name?: unknown }).name === paramName,
    );
  }
  return false;
}

/** True when the runner's /schema says `config` accepts `mesh_control_url`. */
export function schemaSupportsMeshConfig(schema: unknown): boolean {
  return schemaHasConfigParam(schema, 'config', 'mesh_control_url');
}

/**
 * Quote a value for the phone shell inside an `adb shell am broadcast --es`
 * argument (the device shell strips them; spaces and metachars stay one
 * token). Single quotes inside the value are escaped the way the app's own
 * provision.sh does it: `'\''`.
 */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}
