import { describe, expect, test } from 'bun:test'
import {
  isUsageLedgerEnabled,
  laneFor,
  reportLedgerCall,
  toLedgerPayload,
  type LedgerCall,
} from '../../src/runtime/services/telemetry/usageLedgerReport'

const call: LedgerCall = {
  sessionID: 'ses_1',
  messageID: 'msg_1',
  providerID: 'subs-claude',
  modelID: 'm',
  surface: 'cowork',
  tokens: { input: 10, output: 4, reasoning: 1, cache: { read: 30, write: 2 } },
  cost: 0.012,
  latencyMs: 80,
}

const env = (extra: Record<string, string> = {}) =>
  ({ ALLTERNIT_API_URL: 'http://api.test/', ALLTERNIT_API_TOKEN: 'tok', ALLTERNIT_USER_ID: 'u1', ...extra }) as NodeJS.ProcessEnv

describe('usage ledger report (O15)', () => {
  test('silent unless allternit-api is explicitly configured, or when switched off', () => {
    expect(isUsageLedgerEnabled({} as NodeJS.ProcessEnv)).toBe(false)
    expect(isUsageLedgerEnabled(env())).toBe(true)
    expect(isUsageLedgerEnabled(env({ GIZZI_USAGE_LEDGER: 'off' }))).toBe(false)
  })

  test('lane comes from the provider', () => {
    expect(laneFor('subs-claude')).toBe('subscription-cli')
    expect(laneFor('ollama')).toBe('local')
    expect(laneFor('openrouter')).toBe('api')
  })

  test('payload carries tokens, cache, cost and surface; never content', () => {
    const p = toLedgerPayload(call, 'id1')
    expect(p).toMatchObject({
      call_id: 'id1', session_id: 'ses_1', surface: 'cowork', tier: 'S2', lane: 'subscription-cli',
      input_tokens: 10, output_tokens: 4, reasoning_tokens: 1, cache_read_tokens: 30, cache_write_tokens: 2,
      cost_usd: 0.012, latency_ms: 80,
    })
    expect(toLedgerPayload({ ...call, surface: 'secret' }, 'x').surface).toBeUndefined()
  })

  test('posts to /api/v1/usage/ledger with the existing auth headers and retries once', async () => {
    const seen: { url: string; init: RequestInit }[] = []
    let n = 0
    const fake = (async (url: string, init: RequestInit) => {
      seen.push({ url, init })
      n += 1
      return new Response('{}', { status: n === 1 ? 503 : 200 })
    }) as unknown as typeof fetch
    expect(await reportLedgerCall(call, env(), fake)).toBe(true)
    expect(seen).toHaveLength(2)
    expect(seen[0].url).toBe('http://api.test/api/v1/usage/ledger')
    const headers = seen[0].init.headers as Record<string, string>
    expect(headers.Authorization).toBe('Bearer tok')
    expect(headers['x-allternit-user-id']).toBe('u1')
    // Same call id on the retry (server-side upsert, never a double row).
    expect(seen[0].init.body).toBe(seen[1].init.body)
    expect(JSON.parse(String(seen[0].init.body)).calls).toHaveLength(1)
  })

  test('never throws when the API is down; does not post when disabled', async () => {
    const down = (async () => { throw new Error('ECONNREFUSED') }) as unknown as typeof fetch
    expect(await reportLedgerCall(call, env(), down)).toBe(false)
    let called = false
    const spy = (async () => { called = true; return new Response('{}') }) as unknown as typeof fetch
    expect(await reportLedgerCall(call, {} as NodeJS.ProcessEnv, spy)).toBe(false)
    expect(called).toBe(false)
  })
})
