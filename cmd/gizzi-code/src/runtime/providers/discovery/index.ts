/**
 * Provider Discovery
 *
 * Auto-discovers LLM providers available on the local machine without
 * requiring any user configuration. Runs at startup and injects results
 * into the provider state alongside API-key providers from models.dev.
 *
 * Three discovery channels:
 *   1. subprocess — CLI tools in PATH already authenticated (claude, llm, ollama, …)
 *   2. local      — HTTP servers on known local ports (Ollama, LM Studio, Jan, vLLM, …)
 *   3. subscription — providers that surface via a CLI but are subscription-authed
 *
 * Extension:
 *   Any service (Allternit platform, enterprise gateway, plugin) can register its own
 *   discovery hook via `Discovery.register()` before the state initializes.
 *   The hook returns zero or more DiscoveredProvider entries which are merged
 *   into the provider map under the same rules as built-in providers.
 */

import { discoverSubprocessProviders } from "./subprocess"
import { discoverLocalProviders } from "./local"
import { discoverAllternitCloud } from "./allternit-cloud"
import { discoverSubscriptionFabric } from "../fabric/discovery"
import { discoverSiwc } from "../siwc/discovery"

export { getCachedAllternitPlan, refreshAllternitPlan } from "./allternit-cloud"

export interface DiscoveredProvider {
  /** Unique provider ID — shown in /model list */
  id: string
  name: string
  auth_type: "api_key" | "none" | "bearer" | "subprocess"
  /** Set for subprocess providers */
  subprocess_cmd?: string
  /** Set for local/bearer providers */
  base_url?: string
  /** Set for bearer providers */
  token?: string
  /** Source label shown in /model list */
  source: "subprocess" | "local" | "subscription" | "plugin" | "platform"
  /** Provider options merged into the provider (e.g. `runtime: "fabric"`) */
  options?: Record<string, unknown>
  models: DiscoveredModel[]
}

export interface DiscoveredModel {
  id: string
  name: string
  context?: number
  output?: number
}

/** Extension hook type — any module can register a discovery hook */
export type DiscoveryHook = () => Promise<DiscoveredProvider[]>

// Registry of extension hooks — populated before first state() call
const _hooks: DiscoveryHook[] = []
let _last: DiscoveredProvider[] = []
let _running: Promise<DiscoveredProvider[]> | null = null
let _attempted = false

export const Discovery = {
  last(): DiscoveredProvider[] {
    return _last
  },

  prefetch(): void {
    if (_running || _attempted) return
    void this.run()
  },
  /**
   * Register an external discovery hook.
   * Called by Allternit platform, enterprise plugins, or custom integrations.
   *
   * Example (Allternit platform plugin):
   *   Discovery.register(async () => [{
   *     id: "ai-allternit",
   *     name: "Allternit Platform LLM",
   *     auth_type: "bearer",
   *     base_url: "https://api.allternit.io/v1",
   *     token: await Allternit.getToken(),
   *     source: "platform",
   *     models: [{ id: "allternit-sonnet", name: "Allternit Sonnet" }],
   *   }])
   */
  register(hook: DiscoveryHook): void {
    _hooks.push(hook)
  },

  /**
   * Run all discovery channels and return every discovered provider.
   * Results are deduplicated by provider ID — first discovery wins.
   */
  async run(): Promise<DiscoveredProvider[]> {
    if (_running) return _running
    _attempted = true
    _running = (async () => {
      const results = await Promise.allSettled([
        discoverAllternitCloud(),
        discoverSubprocessProviders(),
        discoverLocalProviders(),
        // Before the fabric catalog: first discovery wins a provider id, and
        // SIWC is the preferred path for the ChatGPT subscription lane.
        discoverSiwc(),
        discoverSubscriptionFabric(),
        ..._hooks.map((h) => h()),
      ])

      const seen = new Set<string>()
      const providers: DiscoveredProvider[] = []

      for (const r of results) {
        if (r.status === "rejected") continue
        for (const p of r.value) {
          if (seen.has(p.id)) continue
          seen.add(p.id)
          providers.push(p)
        }
      }

      _last = providers
      return providers
    })()
    try {
      return await _running
    } finally {
      _running = null
    }
  },
}
