/**
 * lookup kind `convert` for currencies: Frankfurter (free, no key; European
 * Central Bank reference rates, published once per working day). The card
 * shows the rate date and credits Frankfurter / ECB. Cache: TTL.fx.
 *
 */
import { fetchJson } from "./http"
import { LIMITS, LookupError, TTL, type ConvertParams, type LookupContext, type LookupResult } from "./types"

/** The ECB reference-rate currencies Frankfurter serves (ISO 4217). */
export const FX_CURRENCIES: ReadonlySet<string> = new Set([
  "AUD", "BGN", "BRL", "CAD", "CHF", "CNY", "CZK", "DKK", "EUR", "GBP", "HKD", "HUF", "IDR", "ILS", "INR", "ISK", "JPY",
  "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN", "RON", "SEK", "SGD", "THB", "TRY", "USD", "ZAR",
])

/** True for a currency code Frankfurter can convert ("usd" counts). */
export function isCurrency(code: string): boolean {
  return FX_CURRENCIES.has(code.trim().toUpperCase())
}

const CREDIT = "Rates: European Central Bank via Frankfurter"
const CREDIT_URL = "https://frankfurter.dev"
const ALSO = ["USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF", "CNY"]

interface FrankfurterResponse {
  base?: string
  date?: string
  rates?: Record<string, number>
}

function money(n: number): string {
  const abs = Math.abs(n)
  const digits = abs >= 1 ? 2 : abs >= 0.01 ? 4 : 6
  return n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: digits })
}

/** Parse a Frankfurter latest-rates body into { date, rates }; throws LookupError on a bad shape. */
export function parseFrankfurter(body: unknown): { date: string; rates: Record<string, number> } {
  const b = body as FrankfurterResponse
  if (!b || typeof b.date !== "string" || !b.rates || typeof b.rates !== "object")
    throw new LookupError("convert", "The exchange-rate service returned an unexpected answer. Try again later.")
  const rates: Record<string, number> = {}
  for (const [k, v] of Object.entries(b.rates)) if (typeof v === "number" && Number.isFinite(v)) rates[k] = v
  return { date: b.date, rates }
}

export async function lookupFx(params: ConvertParams, ctx: LookupContext): Promise<LookupResult> {
  const from = params.from.trim().toUpperCase()
  const to = params.to.trim().toUpperCase()
  if (!Number.isFinite(params.amount)) throw new LookupError("convert", "The amount must be a number.")
  for (const c of [from, to])
    if (!isCurrency(c))
      throw new LookupError("convert", `${c} is not a currency this lookup can convert (ECB reference currencies only). Use websearch and cite the source.`)
  const others = ALSO.filter((c) => c !== from && c !== to).slice(0, LIMITS.conversionsAlsoMax)
  const symbols = from === to ? [] : [to, ...others]
  let date: string
  let rates: Record<string, number> = {}
  if (symbols.length === 0) {
    date = (ctx.now?.() ?? new Date()).toISOString().slice(0, 10)
  } else {
    const url = `https://api.frankfurter.dev/v1/latest?base=${from}&symbols=${symbols.join(",")}`
    try {
      const parsed = parseFrankfurter(await fetchJson<unknown>(url, { signal: ctx.signal, ttlMs: TTL.fx }))
      date = parsed.date
      rates = parsed.rates
    } catch (e) {
      if (e instanceof LookupError) throw e
      throw new LookupError("convert", "Exchange rates could not be fetched right now. Use websearch and cite the source.")
    }
  }
  const rate = from === to ? 1 : rates[to]
  if (typeof rate !== "number")
    throw new LookupError("convert", `No ${from} to ${to} rate was published. Use websearch and cite the source.`)
  const value = params.amount * rate
  const also = others
    .filter((c) => typeof rates[c] === "number")
    .map((c) => ({ value: Math.round(params.amount * rates[c]! * 100) / 100, unit: c }))
  const text =
    `${money(params.amount)} ${from} = ${money(value)} ${to} (1 ${from} = ${rate} ${to}, ECB reference rate for ${date}; ` +
    `rates update once per working day, so this is not a live market or card rate). ${CREDIT}.`
  return {
    text,
    credit: CREDIT,
    card: {
      version: 1,
      template: "conversion",
      title: `${from} to ${to}`,
      summary: `${money(params.amount)} ${from} = ${money(value)} ${to}`,
      data: {
        kind: "currency",
        from: { value: params.amount, unit: from },
        to: { value: Math.round(value * 1e6) / 1e6, unit: to },
        rate,
        asOf: date,
        ...(also.length ? { also } : {}),
        credit: CREDIT,
        creditUrl: CREDIT_URL,
      },
    },
  }
}
