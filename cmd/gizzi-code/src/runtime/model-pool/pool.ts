/**
 * ModelPool (Agency Kernel WP7) — the capability + residency registry.
 *
 * gizzi-code owns providers, so gizzi-code owns the pool and serves it over
 * HTTP (`GET /model-pool`). The commrails Cognitive Execution Router consumes
 * it by capability and emits ExecutionPlanV1s that carry only ids.
 *
 * Single source of what exists: gizzi's provider registry, which already
 * folds in allternit-api's Fabric model_catalog (the "allternit" provider via
 * the Allternit Cloud discovery channel, `GET /v1/models`) next to local and
 * configured providers. Nothing here calls a model.
 *
 * Output entries are ABI 1.0.0 `ModelPoolEntryV1` (closed contract). Concrete
 * model identity is registry DATA only: it lives in `extensions["x-model_ref"]`
 * and never in `backend_id`, which is an opaque hash, so a plan that copies
 * backend ids cannot leak a vendor or model name.
 *
 * Confidence / latency numbers are PRIORS (`x-confidence_source: "prior"`)
 * until the CapabilityEvalRegistry measures them.
 */

import { createHash } from "node:crypto"

export const POOL_ENTRY_SCHEMA_ID = "allternit.kernel.ModelPoolEntryV1"
export const SCHEMA_VERSION = "1.0.0"
/** allternit-api's Fabric catalog surfaces in gizzi as this provider id. */
export const CATALOG_PROVIDER_ID = "allternit"

export type Role = "S0" | "S1" | "S2" | "S3"
export type Mode =
  | "M0.DETERMINISTIC"
  | "M1.LOGIT_READOUT"
  | "M2.CALIBRATED_READOUT"
  | "M3.HIDDEN_HEAD"
  | "M4.DEDICATED_DECIDER"
  | "M5.GENERATIVE"
  | "M6.DEEP_SOLVER"
export type Residency = "PINNED" | "HOT" | "WARM" | "COLD" | "REMOTE"

export interface ModelPoolEntryV1 {
  schema_id: typeof POOL_ENTRY_SCHEMA_ID
  schema_version: string
  backend_id: string
  cognitive_roles: Role[]
  modes: Mode[]
  capabilities: string[]
  trust_tags: string[]
  confidence_estimate: number
  latency_ms: number
  cost: number
  residency: Residency
  memory_mb?: number | null
  extensions: Record<`x-${string}`, unknown>
}

/** The slice of a gizzi `Provider.Model` the pool reads. */
export interface ProviderModelView {
  providerID: string
  modelID: string
  url?: string
  status?: string
  reasoning: boolean
  toolcall: boolean
  imageIn: boolean
  textOut: boolean
  context: number
  /** USD per 1M tokens. */
  costIn: number
  costOut: number
}

const LOCAL_HOST = /^(https?:\/\/)?(localhost|127\.|0\.0\.0\.0|\[?::1\]?)/i

export function backendId(providerID: string, modelID: string): string {
  return "be." + createHash("sha256").update(`${providerID}/${modelID}`).digest("hex").slice(0, 16)
}

export function capabilitiesFor(m: ProviderModelView): string[] {
  const caps: string[] = []
  if (m.textOut) caps.push("cap.text.generate", "cap.code.edit")
  if (m.toolcall) caps.push("cap.agent.tool_use")
  if (m.reasoning) caps.push("cap.reason.deep")
  if (m.imageIn) caps.push("cap.vision.read")
  return caps
}

export function toPoolEntry(m: ProviderModelView): ModelPoolEntryV1 | null {
  if (m.status === "deprecated") return null
  const capabilities = capabilitiesFor(m)
  if (capabilities.length === 0) return null
  const local = !!m.url && LOCAL_HOST.test(m.url)
  const roles: Role[] = m.reasoning ? ["S2", "S3"] : ["S2"]
  const modes: Mode[] = m.reasoning ? ["M5.GENERATIVE", "M6.DEEP_SOLVER"] : ["M5.GENERATIVE"]
  // cost unit: USD per 1k tokens, blended in/out (a prior; the router only compares).
  const cost = Math.max(0, ((m.costIn || 0) + (m.costOut || 0)) / 2 / 1000)
  return {
    schema_id: POOL_ENTRY_SCHEMA_ID,
    schema_version: SCHEMA_VERSION,
    backend_id: backendId(m.providerID, m.modelID),
    cognitive_roles: roles,
    modes,
    capabilities,
    trust_tags: local ? ["PUBLIC", "INTERNAL", "RESTRICTED", "SECRET"] : ["PUBLIC", "INTERNAL"],
    confidence_estimate: m.reasoning ? 0.8 : 0.6,
    latency_ms: local ? 1000 : 2000,
    cost,
    residency: local ? "WARM" : "REMOTE",
    extensions: {
      "x-model_ref": `${m.providerID}/${m.modelID}`,
      "x-source": m.providerID === CATALOG_PROVIDER_ID ? "allternit.model_catalog" : "gizzi.provider",
      "x-context_limit": m.context,
      "x-confidence_source": "prior",
    },
  }
}

export interface PoolFilter {
  capability?: string
  role?: Role
  mode?: Mode
}

export function buildModelPool(models: ProviderModelView[], filter: PoolFilter = {}): ModelPoolEntryV1[] {
  const seen = new Set<string>()
  const out: ModelPoolEntryV1[] = []
  for (const m of models) {
    const e = toPoolEntry(m)
    if (!e || seen.has(e.backend_id)) continue
    seen.add(e.backend_id)
    if (filter.capability && !e.capabilities.includes(filter.capability)) continue
    if (filter.role && !e.cognitive_roles.includes(filter.role)) continue
    if (filter.mode && !e.modes.includes(filter.mode)) continue
    out.push(e)
  }
  return out.sort((a, b) => a.backend_id.localeCompare(b.backend_id))
}

export interface CapabilitySummary {
  capability: string
  backends: number
  roles: Role[]
  modes: Mode[]
  residency: Partial<Record<Residency, number>>
}

export function summarizeCapabilities(entries: ModelPoolEntryV1[]): CapabilitySummary[] {
  const by = new Map<string, CapabilitySummary>()
  for (const e of entries) {
    for (const cap of e.capabilities) {
      const s = by.get(cap) ?? { capability: cap, backends: 0, roles: [], modes: [], residency: {} }
      s.backends++
      for (const r of e.cognitive_roles) if (!s.roles.includes(r)) s.roles.push(r)
      for (const md of e.modes) if (!s.modes.includes(md)) s.modes.push(md)
      s.residency[e.residency] = (s.residency[e.residency] ?? 0) + 1
      by.set(cap, s)
    }
  }
  return [...by.values()].sort((a, b) => a.capability.localeCompare(b.capability))
}

/** Adapt gizzi's `Provider.list()` result (providerID → Info with models). */
export function viewsFromProviders(providers: Record<string, { models?: Record<string, any> }>): ProviderModelView[] {
  const views: ProviderModelView[] = []
  for (const [providerID, info] of Object.entries(providers ?? {})) {
    for (const [modelID, m] of Object.entries(info?.models ?? {})) {
      const caps = m?.capabilities ?? {}
      views.push({
        providerID: m?.providerID ?? providerID,
        modelID: m?.id ?? modelID,
        url: m?.api?.url,
        status: m?.status,
        reasoning: !!caps.reasoning,
        toolcall: !!caps.toolcall,
        imageIn: !!caps.input?.image,
        textOut: caps.output?.text ?? true,
        context: m?.limit?.context ?? 0,
        costIn: m?.cost?.input ?? 0,
        costOut: m?.cost?.output ?? 0,
      })
    }
  }
  return views
}
