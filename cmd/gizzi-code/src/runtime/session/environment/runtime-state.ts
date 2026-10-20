/**
 * In-memory state of applied cloud environments, per session: the variables
 * tool processes get (the environment's .env vars, its secrets and the
 * network-policy proxy variables) and the egress proxy itself.
 *
 * Memory only, on purpose: secret values are never written to disk. After a
 * server restart the environment is resolved again on the session's next
 * turn (SessionEnvironment.ensure) before any tool runs.
 *
 * Free of Session imports so Session.remove can dispose it without a cycle.
 */
import { SecretRedactor } from "@/runtime/session/environment/redactor"
import type { EgressProxy } from "@/runtime/session/environment/egress-proxy"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "session-environment" })

export namespace EnvironmentRuntime {
  export type State = {
    environmentID: string
    network: "full" | "limited" | "none"
    /** Variables merged into every tool process's environment. */
    env: Record<string, string>
    proxy?: EgressProxy.Handle
    workspace: string
    dirs: string[]
  }

  const states = new Map<string, State>()

  export function get(sessionID: string): State | undefined {
    return states.get(sessionID)
  }

  export async function set(sessionID: string, state: State) {
    const previous = states.get(sessionID)
    states.set(sessionID, state)
    if (previous?.proxy && previous.proxy !== state.proxy) await previous.proxy.close().catch(() => {})
  }

  const PROXY_KEYS = new Set(["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "all_proxy", "no_proxy"])

  /**
   * What a tool process of this session adds to its environment ({} for
   * local sessions). `proxy: false` leaves out the network-policy proxy
   * variables, for commands that run somewhere the 127.0.0.1 proxy isn't
   * reachable (a session VM).
   */
  export function toolEnv(sessionID: string | undefined, opts: { proxy?: boolean } = {}): Record<string, string> {
    if (!sessionID) return {}
    const env = states.get(sessionID)?.env
    if (!env) return {}
    if (opts.proxy !== false) return env
    return Object.fromEntries(Object.entries(env).filter(([k]) => !PROXY_KEYS.has(k)))
  }

  export async function dispose(sessionID: string) {
    const state = states.get(sessionID)
    states.delete(sessionID)
    SecretRedactor.unregister(sessionID)
    if (state?.proxy) {
      await state.proxy.close().catch((error) => log.warn("closing egress proxy failed", { sessionID, error }))
    }
  }
}
