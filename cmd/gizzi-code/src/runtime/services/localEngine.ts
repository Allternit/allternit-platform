/**
 * Client for the Allternit Local Engine HTTP surface.
 *
 * Used by the gizzi CLI to query hardware status, the dynamic model catalog,
 * per-model assessments, and recommendations.
 */

const LOCAL_ENGINE_URL = process.env.LOCAL_ENGINE_URL?.trim().replace(/\/$/, '') ?? 'http://127.0.0.1:3015'

export interface HardwareProfile {
  hardware_id: string
  os: string
  arch: string
  cpu_model: string
  cpu_cores: number
  cpu_threads: number
  ram_total_bytes: number
  ram_used_bytes: number
  gpu_name?: string
  apple_chip?: string
  unified_memory: boolean
  backends: { metal: boolean; cuda: boolean; cpu_fallback: boolean }
}

export interface StatusResponse {
  status: string
  active_runtimes: number
  cached_models: number
  hardware_id: string
  apple_chip?: string
  unified_memory: boolean
  backends: { metal: boolean; cuda: boolean; cpu_fallback: boolean }
  platform: { os: string; arch: string; kernel?: string; hostname?: string }
  cpu: { model: string; cores: number; threads: number }
  ram: { total_bytes: number; used_bytes: number; total_mb: number; used_mb: number }
  gpu?: Array<{ name: string; memory_total_mb?: number; memory_used_mb?: number }>
}

export interface CatalogEntry {
  repo_id: string
  downloads: number
  likes: number
  tags?: string[]
  pipeline_tag?: string
  last_modified?: string
  source: 'polled' | 'seed'
}

export interface TokPerSecondEstimates {
  context_4k: number
  context_8k: number
  context_16k: number
  context_32k: number
}

export interface Assessment {
  repo_id: string
  fit: 'fits' | 'tight' | 'no'
  fit_reason: string
  estimated_download_bytes: number
  estimated_loaded_bytes: number
  estimated_tok_per_second: TokPerSecondEstimates
  recommended_backend: 'llama_cpp' | 'mlx'
  confidence: 'measured' | 'inferred' | 'guess'
  quantization_bits: number
  hardware_id: string
}

export interface Recommendation {
  repo_id: string
  fit: 'fits' | 'tight' | 'no'
  fit_reason: string
  estimated_download_bytes: number
  estimated_loaded_bytes: number
  estimated_tok_per_second: TokPerSecondEstimates
  recommended_backend: 'llama_cpp' | 'mlx'
  confidence: 'measured' | 'inferred' | 'guess'
  score: number
  explanation: string
  downloads: number
  likes: number
}

export type RecommendationIntent = 'balanced' | 'smartest' | 'fastest' | 'lightweight'

import type { ModelVerification } from './modelVerification'

class LocalEngineError extends Error {
  constructor(message: string, public status?: number) {
    super(message)
  }
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const url = `${LOCAL_ENGINE_URL}${path}`
  const res = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...(options?.headers ?? {}),
    },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => 'unknown error')
    throw new LocalEngineError(`Local Engine ${res.status}: ${text}`, res.status)
  }
  return res.json() as Promise<T>
}

export async function getStatus(): Promise<StatusResponse> {
  return request('/status')
}

export async function getCatalog(
  source: 'polled' | 'seed' | 'all' = 'all',
  limit = 50,
): Promise<{ models: CatalogEntry[]; count: number }> {
  const qs = new URLSearchParams({ source, limit: String(limit) })
  return request(`/catalog?${qs.toString()}`)
}

export async function refreshCatalog(): Promise<{ refreshed: boolean; count: number }> {
  return request('/catalog/refresh', { method: 'POST', body: JSON.stringify({}) })
}

export async function assessRepo(
  repoId: string,
  quantization?: string,
  contextLength?: number,
): Promise<Assessment> {
  return request('/assess', {
    method: 'POST',
    body: JSON.stringify({ repo_id: repoId, quantization, context_length: contextLength }),
  })
}

export async function recommendRepos(
  intent: RecommendationIntent = 'balanced',
  limit = 5,
): Promise<{ recommendations: Recommendation[]; hardware_id: string; timestamp: string }> {
  return request('/recommend', {
    method: 'POST',
    body: JSON.stringify({ intent, limit }),
  })
}

export async function downloadRepo(
  repoId: string,
  quantization?: string,
  opts?: { allowUnverified?: boolean; revision?: string },
): Promise<DownloadResult> {
  return request('/models/download', {
    method: 'POST',
    body: JSON.stringify({
      repo_id: repoId,
      quantization,
      ...(opts?.revision ? { revision: opts.revision } : {}),
      ...(opts?.allowUnverified ? { allow_unverified: true } : {}),
    }),
  })
}

export interface DownloadResult {
  id: string
  repo_id?: string
  status?: string
  message?: string
  verification?: ModelVerification
}

export interface CachedModel {
  id: string
  name: string
  /** Where the engine keeps it: a directory, or the single model file. */
  path?: string
  status: string
  downloaded_bytes: number
  total_bytes: number
  verification?: ModelVerification | null
  source_used?: string
  files?: VerifyFile[]
  error?: string | null
}

export async function listModels(): Promise<{ models: CachedModel[] }> {
  return request('/models')
}

export interface VerifyFile {
  path: string
  size?: number
  sha256?: string
  expected_sha256?: string
  ok: boolean
  state?: string
  reason?: string
}

export interface VerifyResponse {
  repo?: string
  revision?: string
  ok?: boolean
  verification?: ModelVerification | null
  source_used?: string
  files?: VerifyFile[]
  fallback_reason?: string
  reason?: string
}

export type VerifyResult =
  | { kind: 'ok'; data: VerifyResponse }
  /** 404 with a JSON body: the route exists, the repo is not downloaded. */
  | { kind: 'not_downloaded'; reason?: string }
  /** 404 with no JSON body: the route is missing, so the engine predates verification. */
  | { kind: 'old_engine' }

/** POST /models/verify. */
export async function verifyRepo(repoId: string, quantization?: string, revision?: string): Promise<VerifyResult> {
  const res = await fetch(`${LOCAL_ENGINE_URL}/models/verify`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ repo_id: repoId, revision, quantization }),
  })
  if (res.status === 404) {
    const body = (await res.json().catch(() => null)) as { reason?: string; error?: string } | null
    if (body && typeof body === 'object' && (body.reason || body.error)) {
      return { kind: 'not_downloaded', reason: body.reason ?? body.error }
    }
    return { kind: 'old_engine' }
  }
  if (!res.ok) {
    throw new LocalEngineError(`Local Engine ${res.status}: ${await res.text().catch(() => 'unknown error')}`, res.status)
  }
  return { kind: 'ok', data: (await res.json()) as VerifyResponse }
}



// ---- On-device (W10): hardware, engine packs, runtimes, chat ----

export interface HardwareResponse {
  hardware_id: string
  os: string
  arch: string
  cpu_model: string
  cpu_threads: number
  ram_total_bytes: number
  ram_available_bytes: number
  gpus: Array<{ name: string; vendor: string; vram_total_bytes: number; vram_free_bytes: number; discrete: boolean }>
  unified_memory: boolean
  apple_chip?: string | null
  disk_free_bytes: number
  models_dir: string
  budget: { kind: 'discrete' | 'unified' | 'cpu'; fast_bytes: number; spill_bytes: number }
  engines: Array<{
    id: 'llama.cpp' | 'mlx'
    backend: string
    version: string | null
    pack_id: string | null
    state: 'ready' | 'installed' | 'fallback' | 'missing' | 'off'
  }>
}

/** GET /hardware. Engines older than C3 answer 404: the caller falls back to /status. */
export async function getHardware(): Promise<HardwareResponse | null> {
  try {
    return await request<HardwareResponse>('/hardware')
  } catch (err) {
    if (err instanceof LocalEngineError && err.status === 404) return null
    throw err
  }
}

export interface RunningInfo {
  model_id: string
  mem_bytes: number | null
  tok_per_sec: number | null
  first_token_ms: number | null
  ctx_used: number | null
  ctx_max: number | null
  idle_unload_sec: number
  idle_sec: number
  requests_in_flight: number
}

export interface RuntimeInstance {
  id: string
  model_id: string
  port: number
  status: 'starting' | 'running' | 'stopped' | 'error' | string
  health?: { reachable: boolean; error?: string | null } | null
  engine_source?: string | null
  pack_id?: string | null
  pack_version?: string | null
  backend?: string | null
  running?: RunningInfo | null
  stop_reason?: string | null
}

export async function listRuntimes(): Promise<{ runtimes: RuntimeInstance[] }> {
  return request('/runtimes')
}

export type LaunchRecipe =
  | { backend: 'llama_cpp'; model_path: string; n_gpu_layers: number; n_ctx?: number; flash_attn?: boolean }
  | { backend: 'mlx'; model_path: string; n_ctx?: number }

/** POST /runtimes/launch. Errors carry the engine's message (501 names the pack to install, 503 a failed pack check). */
export async function launchRuntime(modelId: string, recipe: LaunchRecipe): Promise<{ runtime: RuntimeInstance }> {
  const res = await fetch(`${LOCAL_ENGINE_URL}/runtimes/launch`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ model_id: modelId, recipe }),
  })
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as { error?: string } | null
    throw new LocalEngineError(body?.error ?? `Local Engine ${res.status}`, res.status)
  }
  return res.json() as Promise<{ runtime: RuntimeInstance }>
}

export async function stopRuntime(id: string): Promise<RuntimeInstance> {
  return request(`/runtimes/${encodeURIComponent(id)}/stop`, { method: 'POST', body: '{}' })
}

export interface PacksResponse {
  platform: string
  installed: Array<{ pack_id: string; engine: string; backend: string; version: string }>
  jobs: Array<{ pack_id: string; state: 'queued' | 'downloading' | 'verifying' | 'installed' | 'failed'; bytes_done: number; bytes_total: number; error: string | null }>
  choices?: Array<{ pack_id: string; engine: string; backend: string }>
  started?: string[]
}

/** GET /engines/packs (W6). null when the engine predates engine packs. */
export async function getPacks(): Promise<PacksResponse | null> {
  try {
    return await request<PacksResponse>('/engines/packs')
  } catch (err) {
    if (err instanceof LocalEngineError && err.status === 404) return null
    throw err
  }
}

/** POST /engines/packs/ensure (W6). null when the engine predates engine packs. */
export async function ensurePacks(engines?: Array<'llamacpp' | 'mlx'>): Promise<PacksResponse | null> {
  try {
    return await request<PacksResponse>('/engines/packs/ensure', {
      method: 'POST',
      body: JSON.stringify(engines ? { engines } : {}),
    })
  } catch (err) {
    if (err instanceof LocalEngineError && err.status === 404) return null
    throw err
  }
}

export type ChatEvent =
  | { token: string }
  | { usage: { promptTokens?: number; completionTokens?: number; tokPerSec?: number; firstTokenMs?: number; memBytes?: number; ctxUsed?: number; ctxMax?: number } }

/** POST /v1/chat/completions, streaming. Yields tokens, then the engine's real numbers. */
export async function* streamChat(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  signal?: AbortSignal,
  opts?: { thinking?: boolean },
): AsyncGenerator<ChatEvent> {
  const res = await fetch(`${LOCAL_ENGINE_URL}/v1/chat/completions`, {
    method: 'POST',
    signal,
    headers: { 'Content-Type': 'application/json' },
    // enable_thinking:false is the llama.cpp / MLX chat-template switch (Qwen3 etc.); models without it ignore it.
    body: JSON.stringify({
      model,
      messages,
      stream: true,
      stream_options: { include_usage: true },
      ...(opts?.thinking === false ? { chat_template_kwargs: { enable_thinking: false } } : {}),
    }),
  })
  if (!res.ok || !res.body) {
    throw new LocalEngineError(`Local Engine ${res.status}: ${await res.text().catch(() => 'chat failed')}`, res.status)
  }
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buf += dec.decode(value, { stream: true })
    let nl: number
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).trim()
      buf = buf.slice(nl + 1)
      if (!line.startsWith('data:')) continue
      const data = line.slice(5).trim()
      if (!data || data === '[DONE]') continue
      let j: any
      try { j = JSON.parse(data) } catch { continue }
      const tok = j.choices?.[0]?.delta?.content
      if (typeof tok === 'string' && tok) yield { token: tok }
      if (j.allternit || j.usage) {
        const a = j.allternit ?? {}
        yield {
          usage: {
            promptTokens: j.usage?.prompt_tokens,
            completionTokens: j.usage?.completion_tokens,
            tokPerSec: a.tok_per_sec ?? j.timings?.predicted_per_second,
            firstTokenMs: a.first_token_ms,
            memBytes: a.mem_bytes,
            ctxUsed: a.ctx_used,
            ctxMax: a.ctx_max,
          },
        }
      }
    }
  }
}

export { LocalEngineError }
