import { cloudOrigin, cloudAuthToken } from "@/runtime/providers/discovery/allternit-cloud"
import { allternitCredentials } from "@/runtime/providers/quota"
import type { CloudCreditsCode } from "@/shared/util/provider-error"

/**
 * "Keep going on Allternit Cloud": when the user's own subscription hits its
 * usage limit and the session pauses until the reset, one button continues
 * the same session on Allternit Cloud credits. Never automatic, nothing is
 * charged without the tap, and nothing switches back at the reset.
 *
 * This module picks the Cloud model and reads the account's Cloud funds;
 * `SessionPause.continueOnCloud` does the switch.
 */
export namespace CloudContinue {
  export const PROVIDER_ID = "allternit"
  /**
   * Used when the paused model has no Cloud counterpart and no catalog row
   * is flagged `default: true` (an older Cloud). Normally the catalog's own
   * default wins: it moves to a funded model when an upstream runs dry.
   */
  export const DEFAULT_MODEL = "deepseek-v4.1-flash"

  /** A row of `GET /v1/models`. */
  export interface CatalogModel {
    id?: string
    name?: string
    upstream_id?: string
    aliases?: string[]
    lane?: string
    default?: boolean
    hidden?: boolean
    /** The catalog provider that serves the row ("openrouter"…); a key the person connected for it unlocks the row. */
    provider?: string
    /** Retail price per 1M tokens, USD. */
    pricing?: { input_per_1m_usd?: number; output_per_1m_usd?: number }
    /** Allternit Open rows: `access` "plan" or "research". */
    open?: { access?: string }
  }

  export type OpenReason = "terms_required" | "plan_required" | "allowance_used" | "bought_credits_required"

  /** `open_models` on `/api/v1/billing/credits` and `/api/v1/me/usage` (contract §3). */
  export interface OpenEntitlement {
    allowed: boolean
    reason: OpenReason | null
    termsAccepted: boolean
    plan: string
    planAllowanceUsd: number
    usedUsd: number
    planRemainingUsd: number
    boughtBalanceUsd: number
    periodEnd?: string
    research: boolean
  }

  /** The open entitlement in a payload; undefined when absent (= open rows not locked, as today). */
  export function openEntitlementFrom(payload: any): OpenEntitlement | undefined {
    const o = payload?.open_models
    if (!o || typeof o !== "object" || typeof o.allowed !== "boolean") return undefined
    const reasons: OpenReason[] = ["terms_required", "plan_required", "allowance_used", "bought_credits_required"]
    const reason = reasons.find((r) => r === o.reason) ?? null
    return {
      allowed: o.allowed,
      reason: o.allowed ? null : reason,
      termsAccepted: o.terms_accepted !== false,
      plan: typeof o.plan === "string" ? o.plan : "",
      planAllowanceUsd: num(o.plan_allowance_usd) ?? 0,
      usedUsd: num(o.used_usd) ?? 0,
      planRemainingUsd: num(o.plan_remaining_usd) ?? 0,
      boughtBalanceUsd: num(o.bought_balance_usd) ?? 0,
      periodEnd: typeof o.period_end === "string" ? o.period_end : undefined,
      research: o.research === true,
    }
  }

  /** The lock tag for an open row, or undefined when the account may use open models. */
  export function openLockTag(open: OpenEntitlement | undefined): string | undefined {
    if (!open || open.allowed) return undefined
    switch (open.reason) {
      case "plan_required":
        return "needs a paid plan"
      case "allowance_used":
        return "allowance used"
      case "bought_credits_required":
        return "plus: bought credits only"
      case "terms_required":
        return "accept terms on the web"
      default:
        return "locked"
    }
  }

  /** The one usage line for open models. */
  export function openUsageLine(open: OpenEntitlement): string {
    const usd = (n: number) => `$${n.toFixed(2)}`
    if (open.plan === "free" || open.plan === "" || open.reason === "plan_required") return "Open models: need a paid plan"
    const bought = `bought credits ${usd(open.boughtBalanceUsd)}`
    if (open.plan === "plus" || open.planAllowanceUsd <= 0) return `Open models: Plus runs on bought credits; ${bought}`
    const resets = open.periodEnd ? new Date(open.periodEnd) : undefined
    const date = resets && !Number.isNaN(resets.getTime()) ? `, resets ${resets.toISOString().slice(0, 10)}` : ""
    return `Open models: ${usd(open.usedUsd)} of ${usd(open.planAllowanceUsd)} plan credits used${date}; ${bought}`
  }

  /**
   * `none` carries why, when the server said: `credits_required` (no credits
   * yet) or `credits_exhausted` (used up). Free accounts get no Cloud usage.
   */
  export type Funds = { kind: "balance" | "plan"; usd: number } | { kind: "none"; reason?: CloudCreditsCode }

  export interface Offer {
    modelID: string
    label: string
    funds: Funds
  }

  /** `cloud_models` on `/api/v1/billing/credits` and `/api/v1/me/usage`. */
  export interface Entitlement {
    allowed: boolean
    reason: CloudCreditsCode | null
    balanceUsd: number
    /** Catalog provider ids the account has its own key for. */
    byokProviders: string[]
  }

  /** The entitlement in a credits or usage payload; undefined when the server sent none (= not locked). */
  export function entitlementFrom(payload: any): Entitlement | undefined {
    const c = payload?.cloud_models
    if (!c || typeof c !== "object" || typeof c.allowed !== "boolean") return undefined
    const reason = c.reason === "credits_required" || c.reason === "credits_exhausted" ? c.reason : null
    return {
      allowed: c.allowed,
      reason: c.allowed ? null : (reason ?? "credits_required"),
      balanceUsd: num(c.balance_usd) ?? 0,
      byokProviders: Array.isArray(c.byok_providers) ? c.byok_providers.filter((p: unknown): p is string => typeof p === "string") : [],
    }
  }

  /**
   * Whether a Cloud row is locked: the account may not run Cloud models and
   * the row's provider is not one it has its own key for (the server routes
   * that row through the key first). No entitlement = not locked.
   */
  export function isLocked(entitlement: Entitlement | undefined, row?: Pick<CatalogModel, "provider">): boolean {
    if (!entitlement || entitlement.allowed) return false
    return !(row?.provider && entitlement.byokProviders.includes(row.provider))
  }

  /**
   * A lock check by Cloud model id. Reads the catalog only when a locked
   * account has keys of its own (to learn each row's provider); otherwise
   * every row shares one answer.
   */
  export async function lockCheck(
    entitlement: Entitlement | undefined,
    fetchFn: Fetch = fetch,
  ): Promise<(modelID: string) => boolean> {
    if (!entitlement || entitlement.allowed) return () => false
    if (entitlement.byokProviders.length === 0) return () => true
    const rows = (await catalog(fetchFn)) ?? []
    const byID = new Map(rows.flatMap((m) => (m.id ? [[m.id, m] as const] : [])))
    return (modelID) => isLocked(entitlement, byID.get(modelID))
  }

  /**
   * One spelling per model: "anthropic/claude-sonnet-4.6",
   * "claude-sonnet-4-6-20260215" and "Claude-Sonnet-4.6[1m]" all become
   * "claude-sonnet-4-6".
   */
  export function normalize(id: string): string {
    const last = id.trim().toLowerCase().split("/").at(-1) ?? ""
    return last
      .replace(/\[[^\]]*\]$/, "")
      .replace(/[._:\s]+/g, "-")
      .replace(/-(\d{8}|latest)$/, "")
      .replace(/-+$/, "")
  }

  /**
   * The Cloud model to continue on: the same model when the catalog lists it
   * (by id, alias or upstream id), else the row the catalog flags as its
   * default, else `DEFAULT_MODEL`. Open-lane models are never picked, and
   * hidden rows only as the same model.
   */
  export function pickModel(models: CatalogModel[], currentModelID?: string): { modelID: string; label: string } | undefined {
    const usable = models.filter((m): m is CatalogModel & { id: string } => Boolean(m.id) && m.lane !== "open")
    const label = (m: CatalogModel & { id: string }) => ({ modelID: m.id, label: m.name?.trim() || m.id })
    if (currentModelID) {
      const key = normalize(currentModelID)
      const same = usable.find(
        (m) =>
          normalize(m.id) === key ||
          (m.aliases ?? []).some((a) => normalize(a) === key) ||
          (m.upstream_id ? normalize(m.upstream_id) === key : false),
      )
      if (same) return label(same)
    }
    const offered = usable.filter((m) => m.hidden !== true)
    const fallback = offered.find((m) => m.default === true) ?? offered.find((m) => m.id === DEFAULT_MODEL)
    return fallback ? label(fallback) : undefined
  }

  function num(v: unknown): number | undefined {
    const n = typeof v === "string" ? Number(v) : v
    return typeof n === "number" && Number.isFinite(n) ? n : undefined
  }

  /**
   * What the account can spend on Cloud: prepaid balance first, then what's
   * left of a paid plan's monthly grant. A `cloud_models` entitlement that
   * says not allowed wins: then there is nothing to spend.
   * `credits` is `/api/v1/billing/credits`, `usage` is `/api/v1/me/usage`.
   */
  export function fundsFrom(credits: any, usage: any): Funds {
    const entitlement = entitlementFrom(credits) ?? entitlementFrom(usage)
    if (entitlement && !entitlement.allowed) return { kind: "none", reason: entitlement.reason ?? undefined }
    const balance = num(credits?.balance_usd) ?? 0
    if (balance > 0.005) return { kind: "balance", usd: balance }
    const plan = typeof usage?.plan === "string" ? usage.plan.toLowerCase() : undefined
    const grant = num(usage?.weeklyLimit)
    const used = num(usage?.weeklyUsed)
    const grantLeft = grant !== undefined && used !== undefined ? grant - used : 0
    if (plan && plan !== "free" && grantLeft > 0.005) return { kind: "plan", usd: grantLeft }
    return { kind: "none" }
  }

  /** "$4.20 Cloud balance" / "$12.00 left on your plan" / "No Cloud credits". */
  export function fundsLabel(funds: Funds): string {
    if (funds.kind === "none") return funds.reason === "credits_exhausted" ? "Cloud credits used up" : "No Cloud credits"
    const usd = `$${funds.usd.toFixed(2)}`
    if (funds.kind === "balance") return `${usd} Cloud balance`
    return `${usd} left on your plan`
  }

  /** The note the transcript shows where the session moved to Cloud. */
  export function note(label: string, limit: string): string {
    return `Continued on Allternit Cloud (${label}) after the ${limit} was reached.`
  }

  type Fetch = (url: string, init?: RequestInit) => Promise<Response>

  export async function catalogRows(fetchFn: Fetch = fetch): Promise<CatalogModel[] | undefined> {
    return catalog(fetchFn)
  }

  async function catalog(fetchFn: Fetch): Promise<CatalogModel[] | undefined> {
    const token = cloudAuthToken()
    const res = await fetchFn(`${cloudOrigin()}/v1/models`, {
      headers: { Accept: "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      signal: AbortSignal.timeout(8000),
    }).catch(() => undefined)
    if (!res?.ok) return
    const json = (await res.json().catch(() => undefined)) as { data?: CatalogModel[] } | undefined
    return json?.data
  }

  /** Each endpoint accepts only one of the app's credentials; try both. Null = signed out. */
  async function readAuthed(fetchFn: Fetch, path: string, tokens: string[]): Promise<any | null | undefined> {
    let rejected = 0
    for (const token of tokens) {
      const res = await fetchFn(`${cloudOrigin()}${path}`, {
        headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
        signal: AbortSignal.timeout(8000),
      }).catch(() => undefined)
      if (res?.status === 401 || res?.status === 403) rejected++
      if (res?.ok) return res.json().catch(() => undefined)
    }
    return rejected === tokens.length ? null : undefined
  }

  /**
   * The offer for a session paused on `currentModelID`, or undefined when
   * there is no Cloud account to continue on (signed out, credentials
   * rejected, Cloud unreachable, no model to pick).
   */
  export async function offer(currentModelID?: string, fetchFn: Fetch = fetch): Promise<Offer | undefined> {
    const tokens = allternitCredentials()
    if (tokens.length === 0) return
    const [models, credits, usage] = await Promise.all([
      catalog(fetchFn),
      readAuthed(fetchFn, "/api/v1/billing/credits", tokens),
      readAuthed(fetchFn, "/api/v1/me/usage", tokens),
    ])
    if (credits === null && usage === null) return
    if (credits === undefined && usage === undefined) return
    const model = pickModel(models ?? [], currentModelID)
    if (!model) return
    return { ...model, funds: fundsFrom(credits ?? undefined, usage ?? undefined) }
  }
}
