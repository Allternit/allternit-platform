/**
 * O15 cost ledger: report every model call gizzi-code makes to allternit-api
 * (`POST /api/v1/usage/ledger`, backend: cmd/allternit-api/src/usage_ledger.rs),
 * so one ledger (`llm_usage_events`) covers gateway, internal and gizzi calls.
 *
 * - Reuses the existing allternit-api auth exactly as the usage telemetry client
 *   does (`Authorization: Bearer $ALLTERNIT_API_TOKEN` when set, plus
 *   `x-allternit-user-id`). No new auth path.
 * - Sends only when allternit-api is explicitly configured (`ALLTERNIT_API_URL`
 *   or `ALLTERNIT_API_BASE_URL`; the desktop shell always passes it), and stays
 *   silent under the global telemetry kill switches or `GIZZI_USAGE_LEDGER=off`.
 * - Metering only: model/provider ids, token counts, cost, latency, session id
 *   and surface. No prompt content, no tool arguments, no file paths.
 * - Sessions allternit-api itself drives (gateway, internal completions) are
 *   deduped server-side by session id, so they are never double-counted.
 * - Fire-and-forget: short timeout, never throws.
 */

import { isTelemetryDisabled } from '@/shared/utils/privacyLevel'
import { Log } from '@/shared/util/log'

const log = Log.create({ service: 'usageLedgerReport' })
const POST_TIMEOUT_MS = 2_500

/** Surfaces the ledger accepts from gizzi (session surface tags). */
const SURFACES = new Set(['chat', 'cowork', 'code', 'browser', 'design'])

export type LedgerCall = {
  sessionID: string
  messageID: string
  providerID: string
  modelID: string
  surface?: string
  tokens: { input: number; output: number; reasoning: number; cache: { read: number; write: number } }
  cost: number
  latencyMs?: number
}

export function ledgerBaseUrl(env: NodeJS.ProcessEnv = process.env): string | undefined {
  const value = (env.ALLTERNIT_API_URL || env.ALLTERNIT_API_BASE_URL || '').trim()
  return value ? value.replace(/\/+$/, '') : undefined
}

export function isUsageLedgerEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  if ((env.GIZZI_USAGE_LEDGER ?? '').trim().toLowerCase() === 'off') return false
  if (isTelemetryDisabled()) return false
  return ledgerBaseUrl(env) !== undefined
}

/** Lane: the user's subscription (Subscription Fabric `subs-*` providers),
 *  a local model, or a metered API. */
export function laneFor(providerID: string): 'api' | 'subscription-cli' | 'local' {
  const p = providerID.toLowerCase()
  if (p.startsWith('subs-')) return 'subscription-cli'
  if (['ollama', 'lmstudio', 'llama.cpp', 'llamacpp', 'local', 'bonsai'].some((l) => p === l || p.startsWith(`${l}-`))) return 'local'
  return 'api'
}

let seq = 0

/** The wire shape `POST /api/v1/usage/ledger` takes (one call). */
export function toLedgerPayload(call: LedgerCall, callId: string): Record<string, unknown> {
  return {
    call_id: callId,
    session_id: call.sessionID,
    surface: call.surface && SURFACES.has(call.surface) ? call.surface : undefined,
    tier: 'S2',
    lane: laneFor(call.providerID),
    provider_id: call.providerID,
    model_id: call.modelID,
    input_tokens: call.tokens.input,
    output_tokens: call.tokens.output,
    reasoning_tokens: call.tokens.reasoning,
    cache_read_tokens: call.tokens.cache.read,
    cache_write_tokens: call.tokens.cache.write,
    cost_usd: Number.isFinite(call.cost) ? call.cost : 0,
    latency_ms: call.latencyMs ?? 0,
  }
}

/** Report one model call. Resolves `true` when allternit-api accepted it. */
export async function reportLedgerCall(
  call: LedgerCall,
  env: NodeJS.ProcessEnv = process.env,
  fetchImpl: typeof fetch = fetch,
): Promise<boolean> {
  if (!isUsageLedgerEnabled(env)) return false
  const base = ledgerBaseUrl(env)!
  // Stable per process; a retry of this report reuses it (server upserts).
  const callId = `${call.sessionID}:${call.messageID}:${Date.now().toString(36)}:${(seq += 1)}`
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    'x-allternit-user-id': (env.ALLTERNIT_USER_ID || env.ALLTERNIT_API_USER_ID || 'gizzi-local').trim(),
  }
  const token = env.ALLTERNIT_API_TOKEN?.trim()
  if (token) headers['Authorization'] = `Bearer ${token}`
  const body = JSON.stringify({ calls: [toLedgerPayload(call, callId)] })
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const res = await fetchImpl(`${base}/api/v1/usage/ledger`, {
        method: 'POST',
        headers,
        body,
        signal: AbortSignal.timeout(POST_TIMEOUT_MS),
      })
      if (res.ok) return true
      if (res.status < 500) break
    } catch {
      // retry once
    }
  }
  log.debug('usage ledger report not delivered', { sessionID: call.sessionID })
  return false
}
