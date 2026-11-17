/**
 * lookup kind `convert` for currencies: Frankfurter (free, no key; European
 * Central Bank reference rates, published once per working day). The card
 * shows the rate date and credits Frankfurter / ECB. Cache: TTL.fx.
 *
 * Lane E1-2b fills in lookupFx.
 */
import { LookupError, type ConvertParams, type LookupContext, type LookupResult } from "./types"

/** The ECB reference-rate currencies Frankfurter serves (ISO 4217). */
export const FX_CURRENCIES: ReadonlySet<string> = new Set([
  "AUD", "BGN", "BRL", "CAD", "CHF", "CNY", "CZK", "DKK", "EUR", "GBP", "HKD", "HUF", "IDR", "ILS", "INR", "ISK", "JPY",
  "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN", "RON", "SEK", "SGD", "THB", "TRY", "USD", "ZAR",
])

/** True for a currency code Frankfurter can convert ("usd" counts). */
export function isCurrency(code: string): boolean {
  return FX_CURRENCIES.has(code.trim().toUpperCase())
}

export async function lookupFx(params: ConvertParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("convert", "Currency conversion is not available yet. Use websearch and cite the source.")
}
