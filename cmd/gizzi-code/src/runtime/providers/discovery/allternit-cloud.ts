/**
 * Allternit Cloud discovery — live models from api.allternit.com/v1/models
 * and the caller's Free/Plus/Super/Ultra plan from /api/v1/billing/subscription.
 *
 * Runs as a default Discovery channel. Auth is ALLTERNIT_API_KEY / ALLTERNIT_API_TOKEN
 * when present; the public model catalog still loads without a key.
 */

import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import type { DiscoveredModel, DiscoveredProvider } from "./index"

export type AllternitPlan = {
  id: string
  label: string
  plan_tier: string
  status: string
}

let cachedPlan: AllternitPlan | null = null

export function getCachedAllternitPlan(): AllternitPlan | null {
  return cachedPlan
}

const PAID_PLAN_IDS = new Set(["plus", "super", "ultra"])
const PAID_STATUSES = new Set(["active", "trialing"])

export function isPaidAllternitPlan(plan: AllternitPlan | null | undefined): boolean {
  if (!plan) return false
  const id = (plan.id || "").toLowerCase()
  if (!PAID_PLAN_IDS.has(id)) return false
  const status = (plan.status || "").toLowerCase()
  if (status && !PAID_STATUSES.has(status)) return false
  return true
}

/**
 * Allternit Cloud's origin. Desktop sets ALLTERNIT_API_URL to the LOCAL
 * allternit-api (http://127.0.0.1:8013) for gizzi's local calls; using it
 * here sent Cloud discovery to the local API, which answered 401, and
 * Allternit Cloud vanished from every Desktop picker. A loopback URL is
 * never the Cloud. ALLTERNIT_CLOUD_URL overrides explicitly.
 */
export function cloudOrigin(): string {
  const candidates = [
    process.env.ALLTERNIT_CLOUD_URL,
    process.env.ALLTERNIT_API_URL,
    process.env.ALLTERNIT_API_BASE_URL,
  ]
  for (const raw of candidates) {
    const value = (raw || "").trim().replace(/\/+$/, "")
    if (value && !isLoopback(value)) return value
  }
  return CLOUD_URLS.api
}

function isLoopback(url: string): boolean {
  try {
    const host = new URL(url).hostname.replace(/^\[|\]$/g, "")
    return host === "localhost" || host === "::1" || host.startsWith("127.") || host === "0.0.0.0"
  } catch {
    return false
  }
}

export function cloudAuthToken(): string | undefined {
  return (
    process.env.ALLTERNIT_API_KEY?.trim() ||
    process.env.ALLTERNIT_API_TOKEN?.trim() ||
    undefined
  )
}

/**
 * The credential for Cloud chat: ALLTERNIT_API_KEY, then ALLTERNIT_API_TOKEN
 * (Desktop and every cloud computer pass the computer's device token here),
 * then this runtime's own `gizzi login` / `gizzi pair` device token. Cloud
 * accepts a device token for chat as the computer's owner. Reading only
 * ALLTERNIT_API_KEY sent every Desktop and cloud-computer chat unsigned.
 */
export async function cloudInferenceToken(): Promise<string | undefined> {
  const env = cloudAuthToken()
  if (env) return env
  // Loaded on demand: only a runtime with no env credential reads its pairing.
  const { Pairing } = await import("@/runtime/services/pairing/pairing")
  const stored = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
}

function authHeaders(): Record<string, string> {
  const token = cloudAuthToken()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

export async function refreshAllternitPlan(): Promise<AllternitPlan | null> {
  const token = cloudAuthToken()
  if (!token) {
    cachedPlan = null
    return null
  }
  try {
    const res = await fetch(`${cloudOrigin()}/api/v1/billing/subscription`, {
      headers: authHeaders(),
      signal: AbortSignal.timeout(8000),
    })
    if (!res.ok) {
      cachedPlan = null
      return null
    }
    const json = (await res.json()) as {
      plan_id?: string
      label?: string
      plan_tier?: string
      status?: string
    }
    const id = (json.plan_id || "free").toLowerCase()
    const label =
      json.label ||
      (id === "plus" ? "Plus" : id === "super" ? "Super" : id === "ultra" ? "Ultra" : "Free")
    cachedPlan = {
      id,
      label,
      plan_tier: json.plan_tier || (id === "free" ? "free" : "pro"),
      status: json.status || "none",
    }
    return cachedPlan
  } catch {
    cachedPlan = null
    return null
  }
}

type CloudModel = {
  id?: string
  owned_by?: string
  provider?: string
  name?: string
  extra?: { name?: string; context_length?: number }
  context_length?: number
}

export async function discoverAllternitCloud(): Promise<DiscoveredProvider[]> {
  const planTask = refreshAllternitPlan()
  try {
    const res = await fetch(`${cloudOrigin()}/v1/models`, {
      headers: authHeaders(),
      signal: AbortSignal.timeout(8000),
    })
    if (!res.ok) {
      await planTask
      return []
    }
    const json = (await res.json()) as { data?: CloudModel[] }
    const models: DiscoveredModel[] = []
    for (const item of json.data ?? []) {
      const id = item.id?.trim()
      if (!id) continue
      const name = item.name || item.extra?.name || id
      const context = item.context_length ?? item.extra?.context_length
      models.push({
        id,
        name,
        context: typeof context === "number" ? context : 128000,
        output: 8192,
      })
    }
    if (models.length === 0) {
      await planTask
      return []
    }
    await planTask
    return [
      {
        id: "allternit",
        name: "Allternit Cloud",
        auth_type: "api_key",
        base_url: `${cloudOrigin()}/v1`,
        source: "platform",
        models,
      },
    ]
  } catch {
    await planTask.catch(() => null)
    return []
  }
}
