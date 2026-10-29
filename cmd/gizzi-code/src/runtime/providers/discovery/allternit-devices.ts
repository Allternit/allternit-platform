/**
 * Allternit devices discovery — models the user's own devices serve through
 * the cloud model-host relay (allternit-cloud-api `routes/model_hosts.rs`).
 * A phone with the PWA open hosts a small on-device model; gizzi talks to it
 * as an OpenAI-compatible provider at `${cloud}/api/v1/model-hosts/v1`.
 *
 * Model ids are `<host_id>/<model_id>`, so they carry a slash; refs split on
 * the first slash only (`Provider.parseModel`), which keeps them intact.
 *
 * Hosts come and go, so `Provider` re-runs `scanAllternitDevices` every 30 s
 * while gizzi runs (see `watchAllternitDevices` in provider.ts).
 */

import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { Pairing } from "@/runtime/services/pairing/pairing"
import type { DiscoveredModel, DiscoveredProvider } from "./index"

export const ALLTERNIT_DEVICES_PROVIDER_ID = "allternit-devices"

/** The cloud API (not local allternit-api, which owns ALLTERNIT_API_URL in fabric/client.ts). */
export function devicesCloudBase(): string {
  const explicit =
    process.env.ALLTERNIT_CLOUD_API_URL?.trim() || process.env.GIZZI_PLATFORM_API_URL?.trim() || CLOUD_URLS.api
  return explicit.replace(/\/+$/, "")
}

/**
 * Same credential precedence as `getAllternitApiConfigWithDeviceToken`: this
 * machine's paired `allternit_runtime_…` device token first, then an API key
 * or token from the environment. Undefined = not signed in, nothing to scan.
 */
export async function devicesCredential(): Promise<string | undefined> {
  const stored = await Pairing.load().catch(() => undefined)
  if (Pairing.tokenUsable(stored)) return stored.deviceToken!
  return process.env.ALLTERNIT_API_KEY?.trim() || process.env.ALLTERNIT_API_TOKEN?.trim() || undefined
}

type HostModel = {
  id?: string
  owned_by?: string
  context_length?: number
}

/**
 * One scan of the relay's model list.
 * - undefined: no credential, or the cloud rejected it (401/403) — no provider.
 * - a provider (possibly with zero models): the current list of online hosts.
 * - throws on network/server errors, so a rescan can keep the last good list.
 */
export async function scanAllternitDevices(timeoutMs = 8000): Promise<DiscoveredProvider | undefined> {
  const token = await devicesCredential()
  if (!token) return undefined
  const baseURL = `${devicesCloudBase()}/api/v1/model-hosts/v1`
  const res = await fetch(`${baseURL}/models`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(timeoutMs),
  })
  if (res.status === 401 || res.status === 403) return undefined
  if (!res.ok) throw new Error(`model-hosts models: HTTP ${res.status}`)
  const json = (await res.json()) as { data?: HostModel[] }
  const models: DiscoveredModel[] = []
  for (const item of json.data ?? []) {
    const id = item.id?.trim()
    if (!id) continue
    const host = item.owned_by?.trim()
    const bare = id.slice(id.indexOf("/") + 1)
    models.push({
      id,
      name: host ? `${bare} (${host})` : id,
      context: typeof item.context_length === "number" ? item.context_length : 8192,
      output: 2048,
    })
  }
  return {
    id: ALLTERNIT_DEVICES_PROVIDER_ID,
    name: "Your devices",
    auth_type: "bearer",
    base_url: baseURL,
    token,
    source: "platform",
    models,
  }
}

export async function discoverAllternitDevices(): Promise<DiscoveredProvider[]> {
  try {
    const provider = await scanAllternitDevices()
    return provider && provider.models.length > 0 ? [provider] : []
  } catch {
    return []
  }
}
