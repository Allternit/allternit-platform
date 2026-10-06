/**
 * Allternit Platform API client for task/cron sync.
 *
 * Task and cron state lives in the Allternit platform (allternit-api). There is
 * no local-file fallback in the runtime tools; the API is always the source of
 * truth. Local dev uses the loopback gateway default (ALLTERNIT_GATEWAY_BASE).
 *
 * Auth:
 * 1. ALLTERNIT_API_TOKEN as a Bearer token (Clerk JWT in production)
 * 2. Local dev: x-allternit-user-id only, against a loopback API. No token is
 *    ever sent to a non-loopback target without ALLTERNIT_API_TOKEN — a
 *    one-time stderr warning is emitted instead.
 *
 * The canvas functions below (HTML artifact publish) use a wider precedence —
 * see `getAllternitApiConfigWithDeviceToken`.
 */

import { Pairing } from '@/runtime/services/pairing/pairing'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

const DEFAULT_API_URL = ALLTERNIT_GATEWAY_BASE

/** Default request deadline for apiFetch when the caller passes no signal. */
export const API_FETCH_DEFAULT_TIMEOUT_MS = 15_000

/**
 * Resolve the Allternit API origin. Explicit configuration always wins.
 * Development keeps the loopback default; an explicit production build
 * (NODE_ENV=production, as injected by script/build-production.js) must be
 * configured deliberately — silently phoning home to a guessed URL is worse
 * than failing fast. The desktop shell and hosted runtimes always pass
 * ALLTERNIT_API_URL explicitly.
 */
function resolveApiUrl(): string {
  const explicit = (process.env.ALLTERNIT_API_URL || process.env.ALLTERNIT_API_BASE_URL || '').trim()
  if (explicit) {
    return explicit.replace(/\/+$/, '')
  }
  if (process.env.NODE_ENV === 'production') {
    throw new Error(
      'ALLTERNIT_API_URL is required in production mode. Set ALLTERNIT_API_URL (or ' +
      'ALLTERNIT_API_BASE_URL) to the Allternit API origin, e.g. ' +
      'https://api.allternit.com. Development mode falls back to ' + DEFAULT_API_URL + '.',
    )
  }
  return DEFAULT_API_URL
}

export type AllternitApiConfig = {
  baseUrl: string
  token?: string
  userId: string
  userEmail?: string
  userName?: string
}

export function getAllternitApiConfig(): AllternitApiConfig {
  const baseUrl = resolveApiUrl()

  const token = process.env.ALLTERNIT_API_TOKEN?.trim()
  const userId = (
    process.env.ALLTERNIT_USER_ID ||
    process.env.ALLTERNIT_API_USER_ID ||
    'gizzi-local'
  ).trim()

  return {
    baseUrl,
    token,
    userId,
    userEmail: process.env.ALLTERNIT_USER_EMAIL?.trim(),
    userName: process.env.ALLTERNIT_USER_NAME?.trim(),
  }
}

/**
 * Same as `getAllternitApiConfig`, but for the canvas endpoints (HTML
 * artifact publish): prefers this machine's paired runtime-device token
 * (`gizzi pair`, stored by `Pairing` in runtime-device.json) over
 * `ALLTERNIT_API_TOKEN` when one is usable. allternit-api's canvas routes
 * accept `allternit_runtime_…` bearer tokens via the same device-token
 * mechanism `instance-registration.ts` already uses to register this
 * machine with allternit-cloud-api — this is that same token
 * source/storage, just pointed at a different backend. Falls through to
 * the existing `ALLTERNIT_API_TOKEN` / desktop-bootstrap dev-fallback
 * precedence unchanged when this machine isn't paired (most local dev
 * setups): unpaired is the common case, not an error.
 */
export async function getAllternitApiConfigWithDeviceToken(): Promise<AllternitApiConfig> {
  const base = getAllternitApiConfig()
  const device = await Pairing.load()
  if (Pairing.tokenUsable(device)) {
    return { ...base, token: device.deviceToken! }
  }
  return base
}

function getHeaders(config: AllternitApiConfig): Record<string, string> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    'x-allternit-user-id': config.userId,
  }

  if (config.token) {
    headers['Authorization'] = `Bearer ${config.token}`
  } else {
    warnMissingTokenOnce(config)
  }

  if (config.userEmail) {
    headers['x-allternit-user-email'] = config.userEmail
  }
  if (config.userName) {
    headers['x-allternit-user-name'] = config.userName
  }

  return headers
}

let warnedMissingToken = false

/** Warn once (to stderr) when requests go out without any Bearer token to a
 *  non-loopback target — the API will reject them, and silently sending
 *  unauthenticated requests at an arbitrary server is a security smell. */
function warnMissingTokenOnce(config: AllternitApiConfig): void {
  if (warnedMissingToken) return
  warnedMissingToken = true
  try {
    const host = new URL(config.baseUrl).hostname.replace(/^\[|\]$/g, '')
    const loopback =
      host === 'localhost' ||
      host.endsWith('.localhost') ||
      host === '127.0.0.1' ||
      host === '::1'
    if (!loopback) {
      process.stderr.write(
        `[allternitApi] warning: no ALLTERNIT_API_TOKEN configured; sending unauthenticated requests to ${config.baseUrl}\n`,
      )
    }
  } catch {
    // Unparseable baseUrl — stay silent; the fetch itself will fail.
  }
}

export async function apiFetch(
  config: AllternitApiConfig,
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const url = `${config.baseUrl}${path}`
  return fetch(url, {
    ...init,
    // Default deadline so a hung gateway can never stall a caller forever;
    // an explicit caller-provided signal always wins.
    signal: init.signal ?? AbortSignal.timeout(API_FETCH_DEFAULT_TIMEOUT_MS),
    headers: {
      ...getHeaders(config),
      ...(init.headers as Record<string, string>),
    },
  })
}

export async function apiFetchJson<T>(
  config: AllternitApiConfig,
  path: string,
  init: RequestInit = {},
): Promise<T> {
  const res = await apiFetch(config, path, init)
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(
      `Allternit API ${init.method || 'GET'} ${path} failed: ${res.status} ${res.statusText}${text ? ` — ${text}` : ''}`,
    )
  }
  return (await res.json()) as T
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tasks
// ═══════════════════════════════════════════════════════════════════════════════

export type ApiTask = {
  id: string
  title: string
  description?: string
  status: string
  assignee_id?: string
  assignee_name?: string
  metadata?: string
  created_at: string
  updated_at: string
}

export type ApiTaskListResponse = {
  tasks: ApiTask[]
  count: number
}

export async function createApiTask(
  config: AllternitApiConfig,
  input: {
    title: string
    description?: string
    status?: string
    assignee_id?: string
    assignee_name?: string
    metadata?: string
  },
): Promise<ApiTask> {
  return apiFetchJson<ApiTask>(config, '/tasks', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export async function listApiTasks(
  config: AllternitApiConfig,
  query?: { status?: string; limit?: number },
): Promise<ApiTaskListResponse> {
  const params = new URLSearchParams()
  if (query?.status) params.set('status', query.status)
  if (query?.limit) params.set('limit', String(query.limit))
  const qs = params.toString()
  return apiFetchJson<ApiTaskListResponse>(
    config,
    `/tasks${qs ? `?${qs}` : ''}`,
  )
}

export async function getApiTask(
  config: AllternitApiConfig,
  id: string,
): Promise<ApiTask> {
  return apiFetchJson<ApiTask>(config, `/tasks/${id}`)
}

export async function updateApiTask(
  config: AllternitApiConfig,
  id: string,
  input: {
    title?: string
    description?: string
    status?: string
    assignee_id?: string
    assignee_name?: string
    metadata?: string
  },
): Promise<ApiTask> {
  return apiFetchJson<ApiTask>(config, `/tasks/${id}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })
}

export async function deleteApiTask(
  config: AllternitApiConfig,
  id: string,
): Promise<void> {
  const res = await apiFetch(config, `/tasks/${id}`, { method: 'DELETE' })
  if (!res.ok && res.status !== 404) {
    const text = await res.text().catch(() => '')
    throw new Error(
      `Allternit API DELETE /tasks/${id} failed: ${res.status} ${res.statusText}${text ? ` — ${text}` : ''}`,
    )
  }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Automation: Routines (persistent cron) and Loops (session cron)
// ═══════════════════════════════════════════════════════════════════════════════

export type ApiRoutine = {
  id: string
  name: string
  description?: string
  status: string
  schedule_type: string
  schedule_expression: string
  timezone?: string
  config: Record<string, unknown>
  max_runs?: number
  created_at: string
  updated_at: string
}

export type ApiLoop = {
  id: string
  name: string
  description?: string
  status: string
  schedule_type: string
  schedule_expression: string
  config: Record<string, unknown>
  expires_at?: string
  created_at: string
  updated_at: string
}

export async function createApiRoutine(
  config: AllternitApiConfig,
  input: {
    name: string
    schedule_expression: string
    description?: string
    timezone?: string
    config?: Record<string, unknown>
    max_runs?: number
  },
): Promise<ApiRoutine> {
  return apiFetchJson<ApiRoutine>(config, '/automation/routines', {
    method: 'POST',
    body: JSON.stringify({
      ...input,
      schedule_type: 'cron',
    }),
  })
}

export async function listApiRoutines(
  config: AllternitApiConfig,
): Promise<ApiRoutine[]> {
  return apiFetchJson<ApiRoutine[]>(config, '/automation/routines')
}

export async function deleteApiRoutine(
  config: AllternitApiConfig,
  id: string,
): Promise<void> {
  const res = await apiFetch(config, `/automation/routines/${id}`, {
    method: 'DELETE',
  })
  if (!res.ok && res.status !== 404) {
    const text = await res.text().catch(() => '')
    throw new Error(
      `Allternit API DELETE /automation/routines/${id} failed: ${res.status} ${res.statusText}${text ? ` — ${text}` : ''}`,
    )
  }
}

export async function createApiLoop(
  config: AllternitApiConfig,
  input: {
    name: string
    schedule_expression: string
    description?: string
    config?: Record<string, unknown>
    expires_at?: string
  },
): Promise<ApiLoop> {
  return apiFetchJson<ApiLoop>(config, '/automation/loops', {
    method: 'POST',
    body: JSON.stringify({
      ...input,
      schedule_type: 'cron',
    }),
  })
}

export async function listApiLoops(
  config: AllternitApiConfig,
): Promise<ApiLoop[]> {
  return apiFetchJson<ApiLoop[]>(config, '/automation/loops')
}

export async function deleteApiLoop(
  config: AllternitApiConfig,
  id: string,
): Promise<void> {
  const res = await apiFetch(config, `/automation/loops/${id}`, {
    method: 'DELETE',
  })
  if (!res.ok && res.status !== 404) {
    const text = await res.text().catch(() => '')
    throw new Error(
      `Allternit API DELETE /automation/loops/${id} failed: ${res.status} ${res.statusText}${text ? ` — ${text}` : ''}`,
    )
  }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Canvases (HTML artifact publish) — see docs/learnings/HTML_ARTIFACTS_PHASE_1_NOTES.md
// for the authoritative route contract this wraps.
//
// Unlike /tasks and /automation above, these paths include the /api/v1
// prefix explicitly — verified against a running allternit-api instance
// during Phase 1/2 (canvas_routes.rs is nested under /api/v1 in main.rs).
// ═══════════════════════════════════════════════════════════════════════════════

export type ApiCanvas = {
  id: string
  session_id: string
  title?: string
  components: unknown
  layout?: unknown
  metadata?: unknown
  artifact_key?: string | null
  version: number
  created_at: string
  updated_at: string
}

export type ApiCanvasListResponse = {
  canvases: ApiCanvas[]
}

/** Response shape of the stable-key upsert call — a narrower subset of
 * ApiCanvas fields (Phase 1's create/redeploy response doesn't echo back
 * title/components/etc, just the identity and version). */
export type ApiCanvasPublishResponse = {
  id: string
  session_id: string
  artifact_key?: string | null
  version: number
  updated_at?: string
}

/**
 * Publish or redeploy a canvas. Always send the full current content —
 * Phase 1's upsert-by-artifact_key is full-replace, not a partial patch
 * (see PHASE_1_NOTES.md, "For Phase 2"): a redeploy that omits a field
 * resets it, it does not preserve the previous value.
 */
export async function publishApiCanvas(
  config: AllternitApiConfig,
  sessionId: string,
  input: {
    title?: string
    components: unknown
    layout?: unknown
    metadata?: unknown
    artifact_key: string
  },
): Promise<ApiCanvasPublishResponse> {
  return apiFetchJson<ApiCanvasPublishResponse>(
    config,
    `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}/canvases`,
    {
      method: 'POST',
      body: JSON.stringify(input),
    },
  )
}

export async function listApiCanvases(
  config: AllternitApiConfig,
  sessionId: string,
): Promise<ApiCanvasListResponse> {
  return apiFetchJson<ApiCanvasListResponse>(
    config,
    `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}/canvases`,
  )
}

export async function getApiCanvas(
  config: AllternitApiConfig,
  canvasId: string,
): Promise<ApiCanvas> {
  return apiFetchJson<ApiCanvas>(
    config,
    `/api/v1/canvases/${encodeURIComponent(canvasId)}`,
  )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Factory peers (cross-session messaging)
// ═══════════════════════════════════════════════════════════════════════════════

export type ApiPeer = {
  peer_id: string
  name: string
  cwd: string
  vendor: string
  inbox_socket: string
  status: string
  registered_at: string
  last_heartbeat_at: string
}

export type ApiPeerListResponse = {
  peers: ApiPeer[]
}

export type ApiPeerRegisterResponse = {
  peer_id: string
  name: string
  inbox_socket: string
}

export type ApiPeerSendResponse = {
  delivered: boolean
  error?: string
}

export type ApiPeerInboxMessage = {
  id: number
  correlation_id: string
  to: string
  from: string
  kind: string
  payload: Record<string, unknown>
  transport: string
  status: string
  created_at: string
}

export type ApiPeerInboxResponse = {
  messages: ApiPeerInboxMessage[]
}

export async function listApiPeers(
  config: AllternitApiConfig,
): Promise<ApiPeerListResponse> {
  return apiFetchJson<ApiPeerListResponse>(config, '/api/factory/peers')
}

export async function registerApiPeer(
  config: AllternitApiConfig,
  input: { name: string; vendor?: string; cwd?: string },
): Promise<ApiPeerRegisterResponse> {
  return apiFetchJson<ApiPeerRegisterResponse>(config, '/api/factory/peers', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export async function sendApiPeerMessage(
  config: AllternitApiConfig,
  name: string,
  body: string,
  from?: string,
): Promise<ApiPeerSendResponse> {
  return apiFetchJson<ApiPeerSendResponse>(
    config,
    `/api/factory/peers/${encodeURIComponent(name)}/send`,
    {
      method: 'POST',
      body: JSON.stringify({ body, ...(from ? { from } : {}) }),
    },
  )
}

export async function pollApiPeerInbox(
  config: AllternitApiConfig,
  name: string,
  limit?: number,
): Promise<ApiPeerInboxResponse> {
  const params = new URLSearchParams()
  if (limit !== undefined) params.set('limit', String(limit))
  const qs = params.toString()
  return apiFetchJson<ApiPeerInboxResponse>(
    config,
    `/api/factory/peers/${encodeURIComponent(name)}/inbox${qs ? `?${qs}` : ''}`,
  )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Content artifacts (A:// Artifacts API — docs/design/artifacts-api.md §3)
// ═══════════════════════════════════════════════════════════════════════════════

export type ApiContentArtifactProvenance = {
  prompt?: string
  designSystemId?: string
  skillId?: string
  skillName?: string
  sourceSessionId?: string
}

export type ApiContentArtifact = {
  id: string
  address?: string
  title?: string
  type?: string
  version?: number
  projectId?: string
  provenance?: ApiContentArtifactProvenance
  sandboxPolicy?: string
  thumbnail?: string
  createdAt?: string
  updatedAt?: string
}

export type ApiContentArtifactListResponse = {
  artifacts?: ApiContentArtifact[]
  next_cursor?: string | null
}

export type ApiContentArtifactGetResponse = ApiContentArtifact & {
  body?: string
  bodySha256?: string
}

export type ApiContentArtifactVersion = {
  version?: number
  bodySha256?: string
  createdAt?: string
}

export async function listApiContentArtifacts(
  config: AllternitApiConfig,
  query: { type?: string; project?: string; q?: string; limit?: number } = {},
): Promise<ApiContentArtifactListResponse> {
  const params = new URLSearchParams()
  if (query.type) params.set('type', query.type)
  if (query.project) params.set('project', query.project)
  if (query.q) params.set('q', query.q)
  if (query.limit !== undefined) params.set('limit', String(query.limit))
  const qs = params.toString()
  return apiFetchJson<ApiContentArtifactListResponse>(
    config,
    `/api/v1/content-artifacts${qs ? `?${qs}` : ''}`,
  )
}

export async function getApiContentArtifact(
  config: AllternitApiConfig,
  id: string,
): Promise<{ artifact: ApiContentArtifactGetResponse }> {
  return apiFetchJson<{ artifact: ApiContentArtifactGetResponse }>(
    config,
    `/api/v1/content-artifacts/${encodeURIComponent(id)}`,
  )
}

export async function getApiContentArtifactVersion(
  config: AllternitApiConfig,
  id: string,
  version: number,
): Promise<{ artifactId: string; version: number; body: string; bodySha256?: string; createdAt?: string }> {
  return apiFetchJson(
    config,
    `/api/v1/content-artifacts/${encodeURIComponent(id)}/versions/${version}`,
  )
}

export async function listApiContentArtifactVersions(
  config: AllternitApiConfig,
  id: string,
): Promise<{ versions?: ApiContentArtifactVersion[] }> {
  return apiFetchJson(
    config,
    `/api/v1/content-artifacts/${encodeURIComponent(id)}/versions`,
  )
}

export async function createApiContentArtifact(
  config: AllternitApiConfig,
  input: {
    title: string
    type?: string
    body: string
    projectId?: string
    sourceSessionId?: string
    prompt?: string
    designSystemId?: string
    skillId?: string
    skillName?: string
    sandboxPolicy?: string
    idempotencyKey?: string
  },
): Promise<{ artifact: ApiContentArtifact }> {
  return apiFetchJson<{ artifact: ApiContentArtifact }>(
    config,
    '/api/v1/content-artifacts',
    { method: 'POST', body: JSON.stringify(input) },
  )
}
