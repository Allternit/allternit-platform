/**
 * Self-hosted offline switch.
 *
 * When `ALLTERNIT_SELF_HOSTED=1` (set by allternit-agents-server for its
 * gizzi child, and by `gizzi worker`), gizzi makes no calls to any
 * Allternit-owned host (`*.allternit.com`, `*.gizziio.com`).
 *
 * Two layers:
 *   1. One central guard, installed before anything else runs (the first
 *      import of src/cli/main.ts): `fetch`, `WebSocket`, `EventSource` and
 *      node's `http`/`https` request functions refuse a vendor host with a
 *      `SelfHostedEgressError` (code `host_not_allowed`). It also turns on
 *      the existing "essential traffic only" switches so telemetry, update
 *      checks, release notes and similar nonessential calls skip themselves
 *      instead of failing.
 *   2. `isSelfHosted()` checks at the call sites that would otherwise log a
 *      failure on every start (model list, catalog sync, cloud proxy, update
 *      check, remote-control push, Clerk key validation).
 *
 * The guard is a refusal, never a rewrite: a blocked call fails fast with a
 * clear message. Nothing is retried against another host.
 */

const VENDOR_SUFFIXES = ["allternit.com", "gizziio.com"] as const

const TRUTHY = new Set(["1", "true", "yes", "on"])

export function isSelfHosted(): boolean {
  return TRUTHY.has((process.env.ALLTERNIT_SELF_HOSTED ?? "").trim().toLowerCase())
}

/** True for allternit.com, gizziio.com and any subdomain of either. */
export function isVendorHost(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/\.$/, "").replace(/^\[|\]$/g, "")
  if (!host) return false
  return VENDOR_SUFFIXES.some((suffix) => host === suffix || host.endsWith("." + suffix))
}

function hostnameOf(input: unknown): string | undefined {
  try {
    if (typeof input === "string") return new URL(input).hostname
    if (input instanceof URL) return input.hostname
    if (input && typeof input === "object") {
      const anyInput = input as { url?: unknown; hostname?: unknown; host?: unknown; href?: unknown }
      if (typeof anyInput.url === "string") return new URL(anyInput.url).hostname
      if (typeof anyInput.href === "string") return new URL(anyInput.href).hostname
      if (typeof anyInput.hostname === "string") return anyInput.hostname
      if (typeof anyInput.host === "string") return anyInput.host.split(":")[0]
    }
  } catch {
    return undefined
  }
  return undefined
}

/** True when `input` (string, URL, Request or node request options) targets a vendor host. */
export function isVendorUrl(input: unknown): boolean {
  const host = hostnameOf(input)
  return host ? isVendorHost(host) : false
}

export class SelfHostedEgressError extends Error {
  readonly code = "host_not_allowed"
  constructor(readonly host: string) {
    super(
      `host_not_allowed: ${host} is an Allternit host, and this gizzi runs in self-hosted mode ` +
        `(ALLTERNIT_SELF_HOSTED=1), which makes no calls to Allternit.`,
    )
    this.name = "SelfHostedEgressError"
  }
}

/** Throws `SelfHostedEgressError` when self-hosted and `input` targets a vendor host. */
export function assertNotVendor(input: unknown): void {
  if (!isSelfHosted()) return
  const host = hostnameOf(input)
  if (host && isVendorHost(host) && isSelfHosted()) throw new SelfHostedEgressError(host)
}

let installed = false

/** Env switches the existing nonessential-traffic and telemetry checks read. */
const QUIET_ENV: Record<string, string> = {
  GIZZI_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  GIZZI_DISABLE_TELEMETRY: "1",
  GIZZI_TELEMETRY: "off",
  DISABLE_TELEMETRY: "1",
  DO_NOT_TRACK: "1",
  DISABLE_AUTOUPDATER: "1",
  // The unknown-route forwarder to api.allternit.com stays off.
  GIZZI_ENABLE_CLOUD_PROXY: "false",
}

/**
 * Install the central guard. A no-op unless `ALLTERNIT_SELF_HOSTED` is set,
 * and safe to call more than once. Returns true when the guard is active.
 */
export function installSelfHostedGuard(): boolean {
  if (!isSelfHosted()) return false
  if (installed) return true
  installed = true

  for (const [key, value] of Object.entries(QUIET_ENV)) process.env[key] = value

  const g = globalThis as Record<string, any>

  const originalFetch = g.fetch as typeof fetch | undefined
  if (typeof originalFetch === "function") {
    const guardedFetch = function (input: any, init?: any) {
      const host = hostnameOf(input)
      if (host && isVendorHost(host) && isSelfHosted()) return Promise.reject(new SelfHostedEgressError(host))
      return originalFetch(input, init)
    }
    // Keep extra properties (Bun's fetch.preconnect, etc.).
    g.fetch = Object.assign(guardedFetch, originalFetch)
  }

  for (const name of ["WebSocket", "EventSource"] as const) {
    const Original = g[name]
    if (typeof Original !== "function") continue
    const Guarded = new Proxy(Original, {
      construct(target, args, newTarget) {
        const host = hostnameOf(args[0])
        if (host && isVendorHost(host) && isSelfHosted()) throw new SelfHostedEgressError(host)
        return Reflect.construct(target, args, newTarget)
      },
    })
    g[name] = Guarded
  }

  // node:http / node:https (axios and other node-style clients). Patches the
  // module objects; code holding an earlier destructured reference is not
  // covered, which is why the fetch guard and the call-site checks exist too.
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const req = typeof require === "function" ? require : undefined
    for (const id of ["node:http", "node:https"]) {
      const mod = req?.(id) as Record<string, any> | undefined
      if (!mod) continue
      for (const fn of ["request", "get"]) {
        const original = mod[fn]
        if (typeof original !== "function") continue
        mod[fn] = function (this: unknown, ...args: any[]) {
          const host = hostnameOf(args[0]) ?? hostnameOf(args[1])
          if (host && isVendorHost(host) && isSelfHosted()) throw new SelfHostedEgressError(host)
          return original.apply(this, args)
        }
      }
    }
  } catch {
    // Module objects may be frozen in some runtimes; the fetch guard still holds.
  }

  return true
}

/** Test hook: lets a test re-run installation after changing env. */
export function __resetSelfHostedGuardForTests(): void {
  installed = false
}
