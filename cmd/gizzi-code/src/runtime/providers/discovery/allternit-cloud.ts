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

export function cloudOrigin(): string {
  const explicit = (process.env.ALLTERNIT_API_URL || process.env.ALLTERNIT_API_BASE_URL || "").trim()
  if (explicit) return explicit.replace(/\/+$/, "")
  return CLOUD_URLS.api
}

export function cloudAuthToken(): string | undefined {
  return (
    process.env.ALLTERNIT_API_KEY?.trim() ||
    process.env.ALLTERNIT_API_TOKEN?.trim() ||
    undefined
  )
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
