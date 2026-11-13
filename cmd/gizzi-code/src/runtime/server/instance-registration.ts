// Tunnel self-registration for `gizzi serve --tunnel`. Once cloudflared hands
// us a public URL, we PUT it to the platform's instance registry so signed-in
// clients (iOS app) can discover this machine. Strictly best-effort: any
// failure logs a warning and never blocks or fails server startup.
//
// Token precedence:
// 1. Paired runtime-device token from runtime-device.json (`gizzi pair`, or a
//    BYO-VPS bootstrap, which writes the same file). Durable (~90 days), so
//    registration is refreshed on an interval while the server runs — this is
//    the heartbeat that keeps the registry entry fresh. The token is also
//    rotated (Pairing.rotateIfNeeded) before each resolve once it nears
//    expiry, so the registry always sees the fresh token.
// 2. ALLTERNIT_API_TOKEN env var (outbound Bearer convention). A runtime
//    device token (allternit_runtime_… — e.g. injected into the systemd env
//    file by the BYO-VPS bootstrap) is durable and gets the same refresh
//    loop; anything else is treated as short-lived and stays one-shot.
// 3. Stored Clerk session JWT from the browser login bridge (auth.json
//    wellknown entries). These are short-lived (~60s), so the JWT payload is
//    decoded and expired tokens are treated as missing.
// Clerk/env-token registration stays one-shot unless the token is a durable
// device token: short-lived tokens die too quickly for a refresh loop to
// mean anything.
import os from "node:os"
import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"
import { Auth } from "@/runtime/integrations/auth"
import { Pairing } from "@/runtime/services/pairing/pairing"
import { isSelfHosted } from "@/shared/selfhost/offline"

export namespace InstanceRegistration {
  const log = Log.create({ service: "instance-registration" })

  const REGISTER_TIMEOUT_MS = 10_000
  // Re-PUT cadence when a durable device token is in use; env override exists
  // for tests (GIZZI_REGISTER_INTERVAL_MS). Read lazily at schedule time:
  // freezing this at module load makes the override ineffective whenever
  // another test file imports this module first.
  function registerIntervalMs(): number {
    return Number(process.env.GIZZI_REGISTER_INTERVAL_MS) || 5 * 60 * 1000
  }

  let refreshTimer: ReturnType<typeof setInterval> | undefined

  // Decodes the JWT payload segment (base64url, no library) and returns true
  // only when the token carries an `exp` that is still in the future.
  function isUsableJwt(token: string): boolean {
    const parts = token.split(".")
    if (parts.length !== 3) return false
    try {
      const payload = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"))
      return typeof payload.exp === "number" && payload.exp * 1000 > Date.now()
    } catch {
      return false
    }
  }

  async function resolveToken(): Promise<{ token: string; durable: boolean; name?: string } | undefined> {
    const device = await Pairing.load()
    if (Pairing.tokenUsable(device)) {
      return { token: device.deviceToken!, durable: true, name: device.name }
    }
    const envToken = process.env.ALLTERNIT_API_TOKEN?.trim()
    // A runtime device token is durable (~90 days): give it the same refresh
    // loop as a paired device so the registry entry stays fresh. Rotation is
    // not possible without the stored identity — the token simply ages out.
    if (envToken) return { token: envToken, durable: envToken.startsWith("allternit_runtime_") }
    // auth.json wellknown entries are keyed by the platform URL the user logged
    // in against; any of them holds a Clerk session JWT accepted by the registry.
    const all = await Auth.all()
    for (const info of Object.values(all)) {
      if (info.type !== "wellknown" || !info.token) continue
      if (!isUsableJwt(info.token)) continue
      return { token: info.token, durable: false }
    }
    return undefined
  }

  async function putRegistration(token: string, name: string, url: string): Promise<boolean> {
    const platform = Flag.GIZZI_PLATFORM_API_URL.replace(/\/+$/, "")
    try {
      const response = await fetch(`${platform}/api/v1/gizzi-instances/self`, {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({ url, name }),
        signal: AbortSignal.timeout(REGISTER_TIMEOUT_MS),
      })
      if (!response.ok) {
        const body = await response.text().catch(() => "")
        log.warn("instance registration failed", { status: response.status, body })
        if (response.status === 401) {
          log.warn("the registry rejected the credential; if this machine was paired, run `gizzi pair` to re-pair")
        }
        return false
      }
      log.info(`gizzi instance registered as ${name} (${url})`)
      return true
    } catch (err) {
      log.warn("instance registration failed", { error: err instanceof Error ? err.message : String(err) })
      return false
    }
  }

  export async function register(opts: { url: string; name?: string }): Promise<void> {
    // Self-hosted: no Allternit instance registry.
    if (isSelfHosted()) {
      log.info("self-hosted mode; skipping instance registration")
      return
    }
    // Rotate a soon-to-expire device token before resolving, so the very first
    // registration PUT already carries the fresh token. Best-effort: a failed
    // rotation leaves the old token in place and registration proceeds with it.
    await Pairing.rotateIfNeeded()
    const credential = await resolveToken()
    if (!credential) {
      log.warn(
        "no auth token available; this instance will not be discoverable by the iOS app. " +
          "Run `gizzi pair` to pair this machine as a runtime device (preferred), " +
          "run the TUI login flow (gizzi), or set ALLTERNIT_API_TOKEN to a valid token.",
      )
      return
    }

    const name = credential.name ?? opts.name ?? os.hostname()
    await putRegistration(credential.token, name, opts.url)

    // Only the durable device token gets a refresh loop; Clerk JWTs and env
    // tokens are short-lived and stay one-shot.
    if (credential.durable) {
      stop()
      refreshTimer = setInterval(async () => {
        // Cheap local expiry check on every tick; only hits the network when
        // the token is actually within the rotation window.
        await Pairing.rotateIfNeeded()
        const current = await resolveToken()
        if (!current?.durable) {
          log.warn("paired device token no longer usable; stopping registration refresh. Run `gizzi pair` to re-pair.")
          stop()
          return
        }
        await putRegistration(current.token, current.name ?? name, opts.url)
      }, registerIntervalMs())
      refreshTimer.unref()
    }
  }

  /** Publish a capability record by registering its primary endpoint URL. */
  export async function registerCapabilityRecord(record: {
    name?: string
    endpoints?: Array<{ url?: string }>
  }): Promise<void> {
    const url = record.endpoints?.find((e) => e.url)?.url
    if (!url) {
      log.warn("capability record has no endpoint URL; registration skipped")
      return
    }
    await register({ url, name: record.name })
  }

  // Clears the refresh loop. Called from Server.stop alongside Tunnel.stop so a
  // graceful shutdown never leaves the interval dangling (it is also unref'd,
  // so it can never keep the process alive on its own).
  export function stop() {
    if (refreshTimer) {
      clearInterval(refreshTimer)
      refreshTimer = undefined
    }
  }
}
