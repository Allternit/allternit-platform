/**
 * Model class → concrete model for subagents.
 *
 *   inherit / standard → the parent's model.
 *   small → provider `auto`: the auto-tier "simple" pick. Otherwise
 *           Provider.getSmallModel on the parent's provider (honours
 *           `small_model` in config); a result on another provider is only
 *           used when `small_model` pinned it, so a session never silently
 *           moves to a different vendor or the sidecar.
 *   deep  → provider `auto`: the auto-tier "reasoning" pick. Otherwise the
 *           strongest known model on the parent's provider.
 *
 * Never throws: any routing miss falls back to the parent model, so a spawn
 * never fails over model choice.
 */
import type { ModelClass } from "./builtin-catalog"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "agents.model-class" })

export type ModelRef = { providerID: string; modelID: string }

const DEEP_PRIORITY = [
  "claude-opus",
  "opus",
  "gpt-5.5-pro",
  "gpt-5-pro",
  "gpt-5.5",
  "o3-pro",
  "o3",
  "gemini-3-pro",
  "gemini-2.5-pro",
  "kimi-k2-thinking",
  "deepseek-reasoner",
]

export async function resolveModelClass(cls: ModelClass | undefined, parent: ModelRef): Promise<ModelRef> {
  if (!cls || cls === "inherit" || cls === "standard") return parent
  try {
    const { Provider } = await import("@/runtime/providers/provider")

    if (parent.providerID === "auto") {
      const tiers = await Provider.buildAutoTiers()
      const pick = cls === "small" ? tiers.simple : tiers.reasoning
      return pick?.providerID && pick?.modelID ? pick : parent
    }

    if (cls === "small") {
      const { Config } = await import("@/runtime/context/config/config")
      const cfg = await Config.get().catch(() => undefined)
      const small = await Provider.getSmallModel(parent.providerID).catch(() => undefined)
      if (!small) return parent
      const sameProvider = small.providerID === parent.providerID
      if (!sameProvider && !cfg?.small_model) return parent
      // Subscription Fabric models run only on a human send (D16).
      if (!sameProvider && Provider.isFabricModel({ providerID: small.providerID })) return parent
      return { providerID: small.providerID, modelID: small.id }
    }

    // deep
    const providers = await Provider.list()
    const models = Object.keys(providers[parent.providerID]?.models ?? {})
    for (const needle of DEEP_PRIORITY) {
      const hit = models
        .filter((id) => id.toLowerCase().includes(needle))
        .sort()
        .reverse()[0]
      if (hit) return { providerID: parent.providerID, modelID: hit }
    }
    return parent
  } catch (error) {
    log.warn("model class routing failed; using parent model", { cls, error: String(error) })
    return parent
  }
}
