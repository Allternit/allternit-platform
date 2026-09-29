/**
 * Subscription Fabric discovery — the gateway's D13 catalog (via the
 * allternit-api forwarder) as gizzi providers: one `subs-<provider>` per
 * connected subscription, one model per model class. They then appear in
 * every model picker and stream through the normal chat pipeline.
 */

import { Log } from "@/shared/util/log"
import type { DiscoveredProvider } from "../discovery"
import { FABRIC_PROVIDER_PREFIX, fabricConfigured, fabricJson } from "./client"

const log = Log.create({ service: "fabric-discovery" })

interface CatalogEntry {
  id: string
  name: string
  provider: string
  tier: string
  health: string
  fabric: { capability: string; options?: { model_class?: string } }
}

const PROVIDER_NAMES: Record<string, string> = {
  chatgpt: "ChatGPT",
  claude: "Claude",
  kimi: "Kimi",
}

const CLASS_NAMES: Record<string, string> = {
  fast: "Fast",
  standard: "Standard",
  reasoning: "Reasoning",
  deep: "Deep",
}

// Fabric chat keeps its own history on the provider side and gizzi sends only
// the new turn, so gizzi-side context never needs compacting.
const FABRIC_CONTEXT = 1_000_000
const FABRIC_OUTPUT = 32_000

export function providersFromCatalog(entries: CatalogEntry[]): DiscoveredProvider[] {
  const byProvider = new Map<string, DiscoveredProvider>()
  for (const entry of entries) {
    if (entry.fabric?.capability !== "chat.create") continue
    const modelClass = entry.fabric.options?.model_class
    if (!modelClass) continue
    const id = `${FABRIC_PROVIDER_PREFIX}${entry.provider}`
    const providerName = PROVIDER_NAMES[entry.provider] ?? entry.provider
    let provider = byProvider.get(id)
    if (!provider) {
      provider = {
        id,
        name: `${providerName} (subscription)`,
        auth_type: "none",
        source: "subscription",
        options: { runtime: "fabric", fabricProvider: entry.provider },
        models: [],
      }
      byProvider.set(id, provider)
    }
    // Several accounts can offer the same class; the gateway picks the account.
    if (provider.models.some((m) => m.id === modelClass)) continue
    provider.models.push({
      id: modelClass,
      name: `${providerName} ${CLASS_NAMES[modelClass] ?? modelClass}`,
      context: FABRIC_CONTEXT,
      output: FABRIC_OUTPUT,
    })
  }
  return [...byProvider.values()]
}

export async function discoverSubscriptionFabric(): Promise<DiscoveredProvider[]> {
  if (!fabricConfigured()) return []
  try {
    const entries = await fabricJson<CatalogEntry[]>("GET", "/v1/catalog", {
      signal: AbortSignal.timeout(8000),
    })
    const providers = providersFromCatalog(Array.isArray(entries) ? entries : [])
    log.info("discovered", { providers: providers.map((p) => `${p.id}:${p.models.length}`) })
    return providers
  } catch (error) {
    // No binding / no forwarder / gateway down: simply no subscription models.
    log.info("unavailable", { error: error instanceof Error ? error.message : String(error) })
    return []
  }
}
