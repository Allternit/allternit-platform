/**
 * Subscription Fabric tasks — submit, follow, cancel, capabilities and
 * artifact download, all through allternit-api's forwarder (see client.ts).
 *
 * Submitting needs a human action (D16). Nothing here mints one: callers pass
 * the id they were handed by the surface where a person sent or confirmed
 * the task (the chat bridge for a send, the approval relay for a tool call).
 */

import { createHash } from "crypto"
import { Log } from "@/shared/util/log"
import { FabricError, HUMAN_ACTION_HEADER, fabricConfigured, fabricFetch, fabricJson, readSse } from "./client"

const log = Log.create({ service: "fabric-tasks" })

export const TERMINAL_STATUSES = new Set(["completed", "partial", "failed", "needs_user", "cancelled"])
const MAX_RECONNECTS = 20

export interface FabricTask {
  task_id: string
  status: string
  status_detail: string | null
  result: { text?: string; artifact_ids: string[] } | null
  error: { class?: string; detail?: string; user_action?: string | null } | null
}

export interface FabricTaskBody {
  capability: string
  prompt: string
  routing: { provider: string }
  options?: Record<string, unknown>
  thread_id?: string
  priority?: "interactive" | "normal" | "background"
  idempotency_key?: string
}

/** POST /v1/tasks with the human action that allows it. */
export function submitFabricTask(body: FabricTaskBody, humanAction: string, signal?: AbortSignal): Promise<FabricTask> {
  return fabricJson<FabricTask>("POST", "/v1/tasks", {
    body,
    headers: { [HUMAN_ACTION_HEADER]: humanAction },
    signal,
  })
}

export function cancelFabricTask(taskID: string): Promise<unknown> {
  return fabricFetch("POST", `/v1/tasks/${taskID}/cancel`, { body: {} }).catch((error) =>
    log.warn("cancel failed", { taskID, error: String(error) }),
  )
}

export interface FollowHandlers {
  onText?: (delta: string) => void
  onProgress?: (label: string) => void
}

/**
 * Follow a task's event stream until it is terminal and return the final
 * task. A dropped stream reconnects (the gateway replays missed events).
 */
export async function followFabricTask(
  taskID: string,
  signal: AbortSignal | undefined,
  handlers: FollowHandlers = {},
): Promise<FabricTask> {
  for (let attempt = 0; attempt <= MAX_RECONNECTS; attempt++) {
    try {
      const res = await fabricFetch("GET", `/v1/tasks/${taskID}/events`, {
        accept: "text/event-stream",
        signal,
      })
      if (!res.ok || !res.body) {
        const body = await res.text().catch(() => "")
        throw new FabricError(`event stream failed (${res.status}) ${body.slice(0, 200)}`, res.status)
      }
      for await (const msg of readSse(res.body, signal)) {
        let payload: any
        try {
          payload = JSON.parse(msg.data)
        } catch {
          continue
        }
        if (msg.event === "reply" && payload?.event?.type === "reply.text.delta") {
          handlers.onText?.(String(payload.event.delta ?? ""))
          continue
        }
        if (msg.event === "progress") {
          const label = payload?.label ?? payload?.event?.label
          if (typeof label === "string" && label.trim()) handlers.onProgress?.(label.trim())
          continue
        }
        if (msg.event === "task.status" && TERMINAL_STATUSES.has(payload?.status)) {
          return await fabricJson<FabricTask>("GET", `/v1/tasks/${taskID}`)
        }
      }
      if (signal?.aborted) throw new DOMException("aborted", "AbortError")
    } catch (error) {
      if (signal?.aborted) throw error
      log.warn("event stream dropped", { taskID, attempt, error: error instanceof Error ? error.message : String(error) })
    }
    // The stream ended without a terminal status: check, then reconnect.
    const task = await fabricJson<FabricTask>("GET", `/v1/tasks/${taskID}`)
    if (TERMINAL_STATUSES.has(task.status)) return task
    await new Promise((r) => setTimeout(r, Math.min(1000 * 2 ** attempt, 10_000)))
  }
  throw new Error("Lost the connection to the subscription task.")
}

// ── Capabilities ─────────────────────────────────────────────────────────────

export interface FabricCapabilityEntry {
  capability: string
  provider: string
  status: "stable" | "beta" | "disabled"
  entitlements?: Array<{ account_id: string; available: boolean; reason_unavailable?: string }>
}

const CAPABILITIES_TTL_MS = 60_000
const CAPABILITIES_FAILURE_TTL_MS = 60_000
const CAPABILITIES_FAILURE_TTL_MAX_MS = 5 * 60_000
let capabilitiesCache:
  | { at: number; ttl: number; entries: FabricCapabilityEntry[]; failureTtl?: number }
  | undefined
let capabilitiesInflight: Promise<FabricCapabilityEntry[]> | undefined

/**
 * GET /v1/capabilities, cached. Tool listing runs on every turn in every
 * session, so it never waits on a slow or unreachable gateway once anything
 * is cached: a stale list is served while one refresh runs behind it. A
 * failed refresh keeps the last good list and backs off (1 min, doubling to
 * 5 min). Empty when the fabric is not set up or has never answered.
 */
export async function fabricCapabilities(opts: { fresh?: boolean } = {}): Promise<FabricCapabilityEntry[]> {
  if (!fabricConfigured()) return []
  const cached = capabilitiesCache
  if (!opts.fresh && cached) {
    if (Date.now() - cached.at >= cached.ttl) void refreshFabricCapabilities()
    return cached.entries
  }
  return refreshFabricCapabilities()
}

function refreshFabricCapabilities(): Promise<FabricCapabilityEntry[]> {
  capabilitiesInflight ??= (async () => {
    const now = Date.now()
    try {
      const entries = await fabricJson<FabricCapabilityEntry[]>("GET", "/v1/capabilities", {
        signal: AbortSignal.timeout(5000),
      })
      const list = Array.isArray(entries) ? entries : []
      capabilitiesCache = { at: now, ttl: CAPABILITIES_TTL_MS, entries: list }
      return list
    } catch (error) {
      log.info("capabilities unavailable", { error: error instanceof Error ? error.message : String(error) })
      const failureTtl = Math.min(
        (capabilitiesCache?.failureTtl ?? CAPABILITIES_FAILURE_TTL_MS / 2) * 2,
        CAPABILITIES_FAILURE_TTL_MAX_MS,
      )
      const entries = capabilitiesCache?.entries ?? []
      capabilitiesCache = { at: now, ttl: failureTtl, entries, failureTtl }
      return entries
    } finally {
      capabilitiesInflight = undefined
    }
  })()
  return capabilitiesInflight
}

/** Test hook. */
export function resetFabricCapabilitiesCache() {
  capabilitiesCache = undefined
  capabilitiesInflight = undefined
}

/**
 * Providers that can run `capability` right now: the adapter is not disabled
 * and at least one connected account is entitled (plan, health, quota).
 */
export function availableProviders(entries: FabricCapabilityEntry[], capability: string): string[] {
  const providers: string[] = []
  for (const entry of entries) {
    if (entry.capability !== capability || entry.status === "disabled") continue
    if (!(entry.entitlements ?? []).some((e) => e.available)) continue
    if (!providers.includes(entry.provider)) providers.push(entry.provider)
  }
  return providers
}

// ── Artifacts ────────────────────────────────────────────────────────────────

/** Largest artifact returned inline to the session. */
export const MAX_ARTIFACT_BYTES = 25 * 1024 * 1024

export interface FabricArtifactFile {
  artifactID: string
  filename: string
  mime: string
  bytes: Uint8Array
  sha256: string
}

/**
 * Download an artifact and check it against the gateway's recorded sha256
 * (the `x-artifact-sha256` header, else the artifact row). A file without a
 * recorded checksum, or one that does not match it, is refused.
 */
export async function downloadFabricArtifact(artifactID: string, signal?: AbortSignal): Promise<FabricArtifactFile> {
  const id = encodeURIComponent(artifactID)
  const res = await fabricFetch("GET", `/v1/artifacts/${id}/download`, { signal })
  if (!res.ok) {
    const body = await res.text().catch(() => "")
    throw new FabricError(`Could not download artifact ${artifactID} (${res.status}) ${body.slice(0, 200)}`, res.status)
  }
  const declared = Number(res.headers.get("content-length") ?? "0")
  if (declared > MAX_ARTIFACT_BYTES) {
    await res.body?.cancel().catch(() => {})
    throw new Error(`Artifact ${artifactID} is ${declared} bytes; the limit here is ${MAX_ARTIFACT_BYTES}.`)
  }
  const bytes = new Uint8Array(await res.arrayBuffer())
  if (bytes.byteLength > MAX_ARTIFACT_BYTES) {
    throw new Error(`Artifact ${artifactID} is ${bytes.byteLength} bytes; the limit here is ${MAX_ARTIFACT_BYTES}.`)
  }
  let expected = res.headers.get("x-artifact-sha256")?.trim().toLowerCase() || undefined
  let row: any
  if (!expected) {
    row = await fabricJson<any>("GET", `/v1/artifacts/${id}`, { signal }).catch(() => undefined)
    const recorded = row?.storage?.sha256
    expected = typeof recorded === "string" && recorded.trim() ? recorded.trim().toLowerCase() : undefined
  }
  if (!expected) throw new Error(`Artifact ${artifactID} has no recorded checksum, so it was not accepted.`)
  const actual = createHash("sha256").update(bytes).digest("hex")
  if (actual !== expected) {
    throw new Error(`Artifact ${artifactID} did not match its recorded checksum, so it was not accepted.`)
  }
  const mime = (res.headers.get("content-type") ?? "application/octet-stream").split(";")[0].trim()
  const disposition = res.headers.get("content-disposition") ?? ""
  const named = /filename="?([^";]+)"?/i.exec(disposition)?.[1]
  return { artifactID, filename: named ?? artifactID, mime, bytes, sha256: actual }
}

export function dataUrl(file: FabricArtifactFile): string {
  return `data:${file.mime};base64,${Buffer.from(file.bytes).toString("base64")}`
}
