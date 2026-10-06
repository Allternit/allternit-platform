// Dual-era MCP connect (protocol 2026-07-28 + the 2025-era `initialize` handshake).
//
// Every gizzi MCP client connects through `McpEra.connect`. The official SDK v2
// (`@modelcontextprotocol/client`) probes a server with `server/discover`: a modern
// server is then spoken to statelessly (per-request `_meta` envelope, no session),
// a legacy server falls back to `initialize`. The verdict is cached per server so a
// connect after the first one skips the probe:
//
// - modern: the cached `DiscoverResult` is adopted without a probe, then confirmed with one
//   `server/discover` on the live connection (stdio: no extra sibling spawn); if that fails
//   (the server went back to the 2025 era), the verdict is dropped and the server re-probed.
// - legacy: the plain `initialize` handshake runs directly. A legacy verdict cannot
//   fail when the server upgrades (upgraded servers still answer `initialize`), so it
//   expires after LEGACY_TTL_MS and the server is re-probed.
//
// The cache is keyed by server name + target (URL or command line), so changing a
// server's URL/command, or re-authenticating it (`forget`), starts over.

import path from "path"
import fs from "fs/promises"
import z from "zod/v4"
import type { Client, ClientOptions, DiscoverResult, PriorDiscovery, Transport } from "@modelcontextprotocol/client"
import { UnauthorizedError } from "@modelcontextprotocol/client"
import { Global } from "@/runtime/context/global"
import { Log } from "@/shared/util/log"
import { withTimeout } from "@/shared/util/timeout"

export namespace McpEra {
  const log = Log.create({ service: "mcp.era" })

  /** The protocol revision gizzi pins when a server is configured as `protocol: "modern"`. */
  export const MODERN_PROTOCOL_VERSION = "2026-07-28"
  /** Legacy verdicts are re-probed after a day, so an upgraded server is picked up. */
  export const LEGACY_TTL_MS = 24 * 60 * 60 * 1000
  /** Modern verdicts carry a DiscoverResult; refresh it daily so capabilities stay current. */
  export const MODERN_TTL_MS = 24 * 60 * 60 * 1000
  /**
   * stdio probe timeout. A local legacy server that ignores the unknown pre-`initialize`
   * request is a legacy server, not an outage, so don't make the user wait the full
   * connect timeout for it (once: the verdict is cached).
   */
  export const STDIO_PROBE_TIMEOUT_MS = 5_000

  /** Per-server override: `auto` (default), `legacy` (initialize only), `modern` (pin 2026-07-28). */
  export const Mode = z.enum(["auto", "legacy", "modern"])
  export type Mode = z.infer<typeof Mode>

  export type Era = "modern" | "legacy"

  const Verdict = z.object({
    era: z.enum(["modern", "legacy"]),
    target: z.string(),
    at: z.number(),
    discover: z.unknown().optional(),
  })
  type Verdict = z.infer<typeof Verdict>

  let cache: Record<string, Verdict> | undefined
  let writeQueue: Promise<void> = Promise.resolve()
  let filepathOverride: string | undefined

  function filepath() {
    return filepathOverride ?? path.join(Global.Path.data, "mcp-era.json")
  }

  /** Tests: point the cache at a scratch file (and drop the in-memory copy). */
  export function setCacheFileForTesting(file: string | undefined) {
    filepathOverride = file
    cache = undefined
  }

  async function load(): Promise<Record<string, Verdict>> {
    if (cache) return cache
    const raw = await fs.readFile(filepath(), "utf8").catch(() => undefined)
    const parsed: Record<string, Verdict> = {}
    if (raw) {
      try {
        const json = JSON.parse(raw) as Record<string, unknown>
        for (const [key, value] of Object.entries(json)) {
          const v = Verdict.safeParse(value)
          if (v.success) parsed[key] = v.data
        }
      } catch {
        // corrupt cache: start over, it is only a probe-skipping hint
      }
    }
    cache = parsed
    return parsed
  }

  function persist() {
    const snapshot = JSON.stringify(cache ?? {}, null, 2)
    writeQueue = writeQueue
      .then(async () => {
        await fs.mkdir(path.dirname(filepath()), { recursive: true })
        await fs.writeFile(filepath(), snapshot, { mode: 0o600 })
      })
      .catch((error) => log.debug("failed to persist era cache", { error }))
    return writeQueue
  }

  function fresh(v: Verdict, now = Date.now()) {
    const ttl = v.era === "legacy" ? LEGACY_TTL_MS : MODERN_TTL_MS
    return now - v.at < ttl
  }

  /** The cached verdict for a server, when it is still fresh and for the same target. */
  export async function prior(key: string, target: string): Promise<PriorDiscovery | undefined> {
    const v = (await load())[key]
    if (!v || v.target !== target || !fresh(v)) return undefined
    if (v.era === "legacy") return { kind: "legacy" }
    if (!v.discover) return undefined
    return { kind: "modern", discover: v.discover as DiscoverResult }
  }

  export async function remember(key: string, target: string, client: Client) {
    const era = client.getProtocolEra()
    if (!era) return
    const data = await load()
    data[key] = {
      era,
      target,
      at: Date.now(),
      ...(era === "modern" ? { discover: client.getDiscoverResult() } : {}),
    }
    await persist()
  }

  /** Drop a server's verdict (URL change, re-auth, a stale modern verdict). */
  export async function forget(key: string) {
    const data = await load()
    if (!(key in data)) return
    delete data[key]
    await persist()
  }

  /** The era a connected client speaks, for status output. */
  export function eraOf(client: Pick<Client, "getProtocolEra">): Era | undefined {
    return client.getProtocolEra()
  }

  /**
   * `ClientOptions.versionNegotiation` for a connect. `legacy` transports (SSE) and
   * `protocol: "legacy"` servers never probe; `modern` pins 2026-07-28 with no fallback.
   */
  export function negotiation(
    mode: Mode,
    probeTimeoutMs: number,
  ): NonNullable<ClientOptions["versionNegotiation"]> {
    if (mode === "legacy") return { mode: "legacy" }
    if (mode === "modern") return { mode: { pin: MODERN_PROTOCOL_VERSION }, probe: { timeoutMs: probeTimeoutMs } }
    return { mode: "auto", probe: { timeoutMs: probeTimeoutMs } }
  }

  export interface ConnectInput<T extends Transport> {
    /** Server name, the cache key. */
    key: string
    /** URL or command line; a change invalidates the cached verdict. */
    target: string
    mode?: Mode
    /** Overall connect timeout (probe + handshake). */
    timeoutMs: number
    /** Probe timeout; defaults to `timeoutMs` (HTTP: silence is an outage, not a legacy signal). */
    probeTimeoutMs?: number
    /** Builds a client with the given negotiation options (a fresh one per attempt). */
    client: (versionNegotiation: NonNullable<ClientOptions["versionNegotiation"]>) => Client
    /** Builds a transport (a fresh one per attempt). */
    transport: () => T
    /** Called with each attempt's transport, so an OAuth caller can keep it for `finishAuth`. */
    onTransport?: (transport: T) => void
  }

  /**
   * Connect with era detection + the per-server verdict cache. Throws the SDK's own
   * errors unchanged (callers check `instanceof UnauthorizedError`).
   */
  export async function connect<T extends Transport>(input: ConnectInput<T>): Promise<{ client: Client; transport: T }> {
    const mode = input.mode ?? "auto"
    const probeTimeoutMs = input.probeTimeoutMs ?? input.timeoutMs
    const cached = mode === "auto" ? await prior(input.key, input.target) : undefined

    const attempt = async (usePrior: PriorDiscovery | undefined) => {
      const client = input.client(negotiation(mode, probeTimeoutMs))
      const transport = input.transport()
      input.onTransport?.(transport)
      try {
        await withTimeout(
          (async () => {
            await client.connect(transport, { timeout: input.timeoutMs, ...(usePrior ? { prior: usePrior } : {}) })
            // Adopting a cached DiscoverResult makes no round trip, so a server that has since
            // gone back to the 2025 era would only fail at the first real request. Re-discover
            // on the live connection to confirm the verdict and refresh capabilities (`ping` is
            // gone in 2026-07-28). On stdio this still saves the probe's sibling-process spawn.
            if (usePrior?.kind === "modern") await client.discover({ timeout: input.timeoutMs })
          })(),
          input.timeoutMs,
        )
      } catch (error) {
        await client.close().catch(() => {})
        throw error
      }
      return { client, transport }
    }

    let connected: { client: Client; transport: T }
    try {
      connected = await attempt(cached)
    } catch (error) {
      if (!cached || error instanceof UnauthorizedError) throw error
      // A cached verdict that no longer holds (server downgraded, redeployed, or its
      // DiscoverResult went stale): forget it and probe once more.
      log.info("cached era verdict failed, re-probing", { key: input.key, era: cached.kind })
      await forget(input.key)
      connected = await attempt(undefined)
    }
    if (mode === "auto") await remember(input.key, input.target, connected.client)
    log.info("connected", {
      key: input.key,
      era: connected.client.getProtocolEra(),
      version: connected.client.getNegotiatedProtocolVersion(),
    })
    return connected
  }
}
