// Price estimate for an eval run (PLAN.md D10, CONTRACT.md section 6).
// A live run needs a fresh estimate id first. Estimates live in memory for 10 minutes.
import crypto from "crypto"
import fs from "fs/promises"
import path from "path"
import { EvalStore } from "./store"
import { Case, Suite } from "./schema"

export const ESTIMATE_TTL_MS = 10 * 60_000
export const DEFAULT_TOKENS = 20_000
export const DEFAULT_MINUTES = 4
const MIN_MINUTES = 1
// Typical agent runs read far more than they write: price tokens as 70% input, 30% output.
const INPUT_SHARE = 0.7

export type Billing = "plan" | "metered" | "unknown"

export interface Estimate {
  estimate_id: string
  expires_at: number
  suite: string
  case_ids: string[]
  cases: number
  k: number
  model?: string
  attempts: number
  tokens_est: number
  minutes_est: number
  billing: Billing
  usd_est?: number
  probe_required: boolean
}

export interface EstimateInput {
  suite?: string
  suiteDir?: string
  caseIds?: string[]
  k?: number
  model?: string
}

/** The edges that touch the machine: sign-in state, the model price table, the default model. */
export interface EstimateDeps {
  now: () => number
  /** "oauth" = plan login, "api" = API key, undefined = no stored login. */
  authType: (providerID: string) => Promise<"oauth" | "api" | "wellknown" | undefined>
  /** Source of a registered provider, e.g. "env" for a key from the environment. */
  providerSource: (providerID: string) => Promise<string | undefined>
  /** $ per 1M tokens. undefined or zero price = unknown. */
  price: (providerID: string, modelID: string) => Promise<{ input: number; output: number } | undefined>
  defaultModel: () => Promise<string | undefined>
}

async function realDeps(): Promise<Pick<EstimateDeps, "authType" | "providerSource" | "price" | "defaultModel">> {
  const { Auth } = await import("@/runtime/integrations/auth")
  const { Provider } = await import("@/runtime/providers/provider")
  return {
    authType: async (id) => (await Auth.get(id))?.type,
    providerSource: async (id) => (await Provider.getProvider(id))?.source as string | undefined,
    price: async (providerID, modelID) => {
      const m = await Provider.getModel(providerID, modelID)
      return m?.cost ? { input: m.cost.input, output: m.cost.output } : undefined
    },
    defaultModel: async () => {
      const ref = await Provider.defaultModel()
      return ref ? `${ref.providerID}/${ref.modelID}` : undefined
    },
  }
}

const store = new Map<string, Estimate>()

export interface LoadedCase {
  id: string
  case: Case
  caseDir: string
}

export async function suiteDirOf(input: { suite?: string; suiteDir?: string }): Promise<string> {
  const dir = input.suiteDir ?? (input.suite ? path.join(EvalStore.root(), "suites", input.suite) : undefined)
  if (!dir) throw new Error("An estimate needs a suite or a suiteDir.")
  return dir
}

/** Read the suite file and every case in it (or only caseIds). Cases that fail to parse are left out. */
export async function loadSuite(input: { suite?: string; suiteDir?: string; caseIds?: string[] }) {
  const dir = await suiteDirOf(input)
  const parsed = Suite.safeParse(JSON.parse(await fs.readFile(path.join(dir, "suite.json"), "utf8").catch(() => "null")))
  if (!parsed.success) throw new Error(`No valid suite.json in ${dir}`)
  const want = input.caseIds ? new Set(input.caseIds) : undefined
  const cases: LoadedCase[] = []
  for (const ref of parsed.data.cases) {
    if (want && !want.has(ref.id)) continue
    const file = path.join(dir, ref.source.path)
    const c = Case.safeParse(JSON.parse(await fs.readFile(file, "utf8").catch(() => "null")))
    if (c.success) cases.push({ id: ref.id, case: c.data, caseDir: path.dirname(file) })
  }
  return { suite: parsed.data, dir, cases }
}

function median(xs: number[]): number | undefined {
  if (!xs.length) return undefined
  const s = [...xs].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  return s.length % 2 ? s[mid] : (s[mid - 1]! + s[mid]!) / 2
}

export async function estimate(input: EstimateInput, overrides: Partial<EstimateDeps> = {}): Promise<Estimate> {
  const deps = { ...(await realDeps()), now: Date.now, ...overrides } as EstimateDeps
  const k = Math.max(1, Math.floor(input.k ?? 1))
  const { suite, cases } = await loadSuite(input)

  // Tokens: recorded trajectory per case, the suite median for cases with none, 20k when nothing is known.
  const recorded = cases.map((c) => c.case.recorded?.trajectory)
  const known = recorded.map((t) => t?.tokens).filter((n): n is number => typeof n === "number" && n > 0)
  const fallbackTokens = median(known) ?? DEFAULT_TOKENS
  const perCaseTokens = recorded.map((t) => (t?.tokens && t.tokens > 0 ? t.tokens : fallbackTokens))
  const tokens_est = Math.round(perCaseTokens.reduce((a, b) => a + b, 0) * k)

  const minutes_est =
    Math.round(
      recorded.reduce((sum, t) => sum + (t?.latency_ms && t.latency_ms > 0 ? Math.max(t.latency_ms / 60_000, MIN_MINUTES) : DEFAULT_MINUTES), 0) * k * 10,
    ) / 10

  // Billing: which kind of login backs the chosen model.
  const modelRef = input.model ?? (await deps.defaultModel().catch(() => undefined))
  let billing: Billing = "unknown"
  let usd_est: number | undefined
  if (modelRef?.includes("/")) {
    const [providerID, ...rest] = modelRef.split("/")
    const modelID = rest.join("/")
    const auth = await deps.authType(providerID!).catch(() => undefined)
    if (auth === "oauth") billing = "plan"
    else if (auth === "api") billing = "metered"
    else {
      const source = await deps.providerSource(providerID!).catch(() => undefined)
      if (source === "env" || source === "config" || source === "api") billing = "metered"
    }
    // A plan shows the equal API price. Zero means the table does not know the model.
    const price = await deps.price(providerID!, modelID).catch(() => undefined)
    if (price && (price.input > 0 || price.output > 0) && billing !== "unknown") {
      const per1m = INPUT_SHARE * price.input + (1 - INPUT_SHARE) * price.output
      usd_est = Math.round((tokens_est * per1m) / 1_000_000 * 10_000) / 10_000
    }
  }
  const probe_required = billing === "metered" && usd_est === undefined

  const now = deps.now()
  const est: Estimate = {
    estimate_id: `est_${ulid(now)}`,
    expires_at: now + ESTIMATE_TTL_MS,
    suite: suite.id,
    case_ids: cases.map((c) => c.id),
    cases: cases.length,
    k,
    ...(input.model ? { model: input.model } : {}),
    attempts: cases.length * k,
    tokens_est,
    minutes_est,
    billing,
    ...(usd_est !== undefined ? { usd_est } : {}),
    probe_required,
  }
  store.set(est.estimate_id, est)
  for (const [id, e] of store) if (e.expires_at <= now) store.delete(id)
  return est
}

/** The estimate for this id if it is still fresh. */
export function getEstimate(id: string | undefined, now: number = Date.now()): Estimate | undefined {
  const e = id ? store.get(id) : undefined
  if (!e) return undefined
  if (e.expires_at <= now) {
    store.delete(id!)
    return undefined
  }
  return e
}

/** True when the estimate was made for this exact suite, case list and k. */
export function matches(e: Estimate, want: { suite: string; caseIds: string[]; k: number; model?: string }): boolean {
  const a = [...e.case_ids].sort()
  const b = [...want.caseIds].sort()
  return e.suite === want.suite && e.k === want.k && (e.model ?? "") === (want.model ?? "") && a.length === b.length && a.every((x, i) => x === b[i])
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
function ulid(now: number) {
  let t = now
  let time = ""
  for (let i = 0; i < 10; i++) {
    time = CROCKFORD[t % 32] + time
    t = Math.floor(t / 32)
  }
  let rand = ""
  for (const b of crypto.randomBytes(10)) rand += CROCKFORD[b % 32]
  return time + rand
}
