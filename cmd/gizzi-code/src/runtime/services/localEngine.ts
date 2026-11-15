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

export { LocalEngineError }
