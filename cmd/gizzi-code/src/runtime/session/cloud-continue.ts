import { cloudOrigin, cloudAuthToken } from "@/runtime/providers/discovery/allternit-cloud"
import { allternitCredentials } from "@/runtime/providers/quota"

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
  /** Used when the paused model has no Cloud counterpart. */
  export const DEFAULT_MODEL = "deepseek-v4-pro"

  /** A row of `GET /v1/models`. */
  export interface CatalogModel {
    id?: string
    name?: string
    upstream_id?: string
    aliases?: string[]
    lane?: string
  }

  export type Funds = { kind: "balance" | "plan" | "free"; usd: number } | { kind: "none" }

  export interface Offer {
    modelID: string
    label: string
    funds: Funds
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
   * (by id, alias or upstream id), else the Cloud default. Open-lane models
   * are never picked.
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
    const fallback = usable.find((m) => m.id === DEFAULT_MODEL)
    return fallback ? label(fallback) : undefined
  }

  function num(v: unknown): number | undefined {
    const n = typeof v === "string" ? Number(v) : v
    return typeof n === "number" && Number.isFinite(n) ? n : undefined
  }

  /**
   * What the account can spend on Cloud: prepaid balance first, then what's
   * left of a paid plan's monthly grant, then the free monthly allowance.
   * `credits` is `/api/v1/billing/credits`, `usage` is `/api/v1/me/usage`.
   */
  export function fundsFrom(credits: any, usage: any): Funds {
    const balance = num(credits?.balance_usd) ?? 0
    if (balance > 0.005) return { kind: "balance", usd: balance }
    const plan = typeof usage?.plan === "string" ? usage.plan.toLowerCase() : undefined
    const grant = num(usage?.weeklyLimit)
    const used = num(usage?.weeklyUsed)
    const grantLeft = grant !== undefined && used !== undefined ? grant - used : 0
    if (plan && plan !== "free" && grantLeft > 0.005) return { kind: "plan", usd: grantLeft }
    const free = num(credits?.free_inference?.remaining_usd) ?? (plan === "free" ? grantLeft : 0)
    if (free > 0.005) return { kind: "free", usd: free }
    return { kind: "none" }
  }

  /** "$4.20 Cloud balance" / "$12.00 left on your plan" / "$1.50 free left this month" / "No Cloud credits left". */
  export function fundsLabel(funds: Funds): string {
    if (funds.kind === "none") return "No Cloud credits left"
    const usd = `$${funds.usd.toFixed(2)}`
    if (funds.kind === "balance") return `${usd} Cloud balance`
    if (funds.kind === "plan") return `${usd} left on your plan`
    return `${usd} free left this month`
  }

  /** The note the transcript shows where the session moved to Cloud. */
  export function note(label: string, limit: string): string {
    return `Continued on Allternit Cloud (${label}) after the ${limit} was reached.`
  }

  type Fetch = (url: string, init?: RequestInit) => Promise<Response>

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
