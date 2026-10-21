/**
 * Sign in with ChatGPT discovery — when the user is signed in with ChatGPT
 * plan usage, the account's real models become the `subs-chatgpt` provider:
 * the same lane the web-chat subscription adapter serves. SIWC is the
 * preferred path; the adapter stays behind it as the fallback (see
 * language-model.ts). Discovery order puts this before the fabric catalog, so
 * the lane id resolves to SIWC while it is available.
 */

import { Log } from "@/shared/util/log"
import type { DiscoveredProvider } from "../discovery"
import { SIWC_FABRIC_PROVIDER, SIWC_PROVIDER_ID, siwcConfigured, siwcModels } from "./broker"

const log = Log.create({ service: "siwc-discovery" })

// The catalog does not report limits here; keep gizzi's compaction conservative.
const SIWC_CONTEXT = 128_000
const SIWC_OUTPUT = 32_000

export function siwcProvider(models: Array<{ slug: string; display_name: string }>): DiscoveredProvider | undefined {
  if (models.length === 0) return undefined
  return {
    id: SIWC_PROVIDER_ID,
    name: "ChatGPT (subscription)",
    auth_type: "none",
    source: "subscription",
    options: { runtime: "siwc", fabricProvider: SIWC_FABRIC_PROVIDER },
    models: models.map((m) => ({ id: m.slug, name: m.display_name, context: SIWC_CONTEXT, output: SIWC_OUTPUT })),
  }
}

export async function discoverSiwc(): Promise<DiscoveredProvider[]> {
  if (!siwcConfigured()) return []
  const provider = siwcProvider(await siwcModels())
  if (!provider) {
    log.info("unavailable")
    return []
  }
  log.info("discovered", { models: provider.models.length })
  return [provider]
}
