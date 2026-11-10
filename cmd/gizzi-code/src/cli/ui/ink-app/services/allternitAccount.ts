/**
 * The signed-in Allternit account as Desktop shows it: the plan (Free, Plus,
 * Max, Ultra), the credit balance and the monthly meter. One read of
 * `GET /api/v1/me/usage` on Allternit Cloud (the same route Desktop's shell
 * meter uses); it costs nothing and runs no model. Credentials, in order:
 * ALLTERNIT_API_TOKEN, ALLTERNIT_API_KEY, then this computer's `gizzi login`
 * device token. A source that is missing or rejected yields `signed-out`,
 * never a made-up plan.
 */
import { cloudOrigin } from '@/runtime/providers/discovery/allternit-cloud'

export type AllternitAccount = {
  /** Plan id: free, plus, super (shown as Max), ultra. */
  plan: string
  /** Display name from the server: Free, Plus, Max, Ultra. */
  label: string
  status: string
  /** Credit balance in USD. */
  credits: number
  /** Monthly grant (paid) or free allowance, USD. */
  monthlyLimit: number
  monthlyUsed: number
  resetsAt?: string
  monthToDateUsd: number
}

export type AccountResult =
  | { status: 'ok'; account: AllternitAccount }
  | { status: 'signed-out'; message: string }
  | { status: 'error'; message: string }

const CACHE_MS = 60_000
let cache: { at: number; result: AccountResult } | undefined
let inflight: Promise<AccountResult> | undefined
const listeners = new Set<() => void>()

function num(value: unknown): number {
  const n = typeof value === 'string' ? Number(value) : value
  return typeof n === 'number' && Number.isFinite(n) ? n : 0
}

export function parseAccountUsage(payload: unknown): AllternitAccount | undefined {
  if (!payload || typeof payload !== 'object') return undefined
  const p = payload as Record<string, unknown>
  if (typeof p.plan !== 'string') return undefined
  const plan = p.plan.toLowerCase()
  // The Max plan keeps the id `super` (cloud-api billing_subscriptions.rs).
  const fallback = plan === 'super' ? 'Max' : plan.charAt(0).toUpperCase() + plan.slice(1)
  return {
    plan,
    label: typeof p.label === 'string' && p.label ? p.label : fallback,
    status: typeof p.status === 'string' ? p.status : 'none',
    credits: num(p.credits),
    // Named weekly* on the wire, but they carry the monthly grant / allowance.
    monthlyLimit: num(p.weeklyLimit),
    monthlyUsed: num(p.weeklyUsed),
    resetsAt: typeof p.resetsAt === 'string' ? p.resetsAt : undefined,
    monthToDateUsd: num(p.monthToDateUsageUsd),
  }
}

async function credentials(): Promise<string[]> {
  const env = [process.env.ALLTERNIT_API_TOKEN, process.env.ALLTERNIT_API_KEY]
  let device: string | undefined
  try {
    const { Pairing } = await import('@/runtime/services/pairing/pairing')
    const stored = await Pairing.load().catch(() => undefined)
    device = Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
  } catch {}
  return [...new Set([...env, device].map(v => (v ?? '').trim()).filter(Boolean))]
}

async function read(fetchImpl: typeof fetch = fetch): Promise<AccountResult> {
  const tokens = await credentials()
  if (tokens.length === 0) {
    return { status: 'signed-out', message: 'Not signed in to Allternit. Run `gizzi login` to see your plan and credits.' }
  }
  let rejected = 0
  for (const token of tokens) {
    const res = await fetchImpl(`${cloudOrigin()}/api/v1/me/usage`, {
      headers: { Authorization: `Bearer ${token}`, Accept: 'application/json' },
      signal: AbortSignal.timeout(8000),
    }).catch(() => undefined)
    if (res?.status === 401 || res?.status === 403) {
      rejected++
      continue
    }
    if (!res?.ok) continue
    const account = parseAccountUsage(await res.json().catch(() => undefined))
    if (account) return { status: 'ok', account }
  }
  if (rejected === tokens.length) {
    return { status: 'signed-out', message: 'Allternit rejected the saved sign-in. Run `gizzi login` again.' }
  }
  return { status: 'error', message: "Couldn't reach Allternit to read your plan." }
}

/** The account, cached 60 s; concurrent callers share one request. */
export async function getAllternitAccount(opts: { fresh?: boolean; fetchImpl?: typeof fetch } = {}): Promise<AccountResult> {
  if (!opts.fresh && cache && Date.now() - cache.at < CACHE_MS) return cache.result
  inflight ??= read(opts.fetchImpl).then(result => {
    cache = { at: Date.now(), result }
    inflight = undefined
    for (const l of listeners) l()
    return result
  })
  return inflight
}

/** Last read, without fetching (for the footer chip). */
export function peekAllternitAccount(): AccountResult | undefined {
  return cache?.result
}

export function onAllternitAccountChange(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Test hook. */
export function resetAllternitAccountCache(): void {
  cache = undefined
  inflight = undefined
}

export function formatUsd(value: number): string {
  return `$${value.toFixed(2)}`
}

/** Footer chip: the credit balance, e.g. "credits $41.20". */
export function creditsChip(result: AccountResult | undefined): string | null {
  if (result?.status !== 'ok') return null
  return `credits ${formatUsd(result.account.credits)}`
}

/** `/usage` and Settings › Usage lines for the account. */
export function accountLines(result: AccountResult, bar: (ratio: number) => string, resetsIn: (iso?: string) => string | undefined): string[] {
  const lines = ['Allternit account']
  if (result.status !== 'ok') {
    lines.push(`  ${result.message}`)
    return lines
  }
  const a = result.account
  const paid = a.plan !== 'free'
  lines.push(`  Plan:      ${a.label}${paid && a.status && a.status !== 'active' && a.status !== 'none' ? ` (${a.status})` : ''}`)
  lines.push(`  Credits:   ${formatUsd(a.credits)} Cloud balance`)
  if (a.monthlyLimit > 0) {
    const ratio = Math.min(1, Math.max(0, a.monthlyUsed / a.monthlyLimit))
    const resets = resetsIn(a.resetsAt)
    lines.push(
      `  ${paid ? 'Monthly' : 'Free monthly'}  ${bar(ratio)}  ${formatUsd(a.monthlyUsed)} of ${formatUsd(a.monthlyLimit)}${resets ? ` · resets in ${resets}` : ''}`,
    )
  }
  lines.push(`  This month: ${formatUsd(a.monthToDateUsd)} spent from credits`)
  if (!paid) lines.push('  Plans: Plus, Max, Ultra · https://ai.allternit.com/settings/billing')
  return lines
}
