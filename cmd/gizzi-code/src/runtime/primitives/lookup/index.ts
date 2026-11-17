/**
 * The `lookup` primitive: fetch or compute one fact and return it as text plus
 * a validated answer card, so the model never retypes the numbers.
 *
 * Shared core for the `lookup` tool (tools/builtins/lookup.ts) and its Ink
 * adapter. One entry point dispatches by kind:
 *
 *   weather → weather.ts (MET Norway)       places  → places.ts (Nominatim)
 *   convert → fx.ts (Frankfurter) when both sides are currencies, else units.ts
 *   time    → zones.ts (local Intl maths)   link    → link.ts (guarded fetch)
 *   images  → images.ts (Wikimedia Commons, then Openverse)
 *
 * Every card is checked against the synced answer contract before it leaves;
 * a card that fails is dropped and the text still goes out.
 */
import { parseAnswerEnvelope } from "@/runtime/integrations/artifacts/answer-contract"
import { lookupFx, isCurrency } from "./fx"
import { lookupImages } from "./images"
import { lookupLink } from "./link"
import { lookupPlaces } from "./places"
import { lookupUnits } from "./units"
import { lookupWeather } from "./weather"
import { lookupZones } from "./zones"
import { LIMITS, LOCAL_KINDS, LookupError, type LookupContext, type LookupParams, type LookupResult } from "./types"

export * from "./types"

const HERE = /^\s*(?:here|near me|nearby|my location|current location|where i am)\s*$/i

/** "here" and friends mean the shared location: the kind gets no place and uses ctx.location or asks. */
export function placeOrHere(text: string | undefined): string | undefined {
  const trimmed = text?.trim()
  return !trimmed || HERE.test(trimmed) ? undefined : trimmed
}

/** True when the call needs no network (time, and unit conversion). The tool asks no permission for these. */
export function isLocalLookup(params: Pick<LookupParams, "kind" | "from" | "to">): boolean {
  if (LOCAL_KINDS.has(params.kind)) return true
  return params.kind === "convert" && !(isCurrency(params.from ?? "") && isCurrency(params.to ?? ""))
}

async function run(params: LookupParams, ctx: LookupContext): Promise<LookupResult> {
  switch (params.kind) {
    case "weather":
      return lookupWeather(
        { place: placeOrHere(params.place ?? params.query), days: params.count === undefined ? undefined : Math.min(params.count, LIMITS.forecastDays) },
        ctx,
      )
    case "places":
      return lookupPlaces(
        { query: params.query, near: placeOrHere(params.place), count: Math.min(params.count ?? LIMITS.placesMax, LIMITS.placesMax) },
        ctx,
      )
    case "convert": {
      if (params.amount === undefined || !params.from || !params.to)
        throw new LookupError("convert", "convert needs amount, from and to.")
      const convert = { amount: params.amount, from: params.from, to: params.to }
      return isCurrency(params.from) && isCurrency(params.to) ? lookupFx(convert, ctx) : lookupUnits(convert, ctx)
    }
    case "time":
      return lookupZones(
        { query: params.query, zones: (params.zones ?? (params.place ? [params.place] : [])).slice(0, LIMITS.zonesMax) },
        ctx,
      )
    case "link":
      return lookupLink({ url: params.query.trim() }, ctx)
    case "images":
      return lookupImages(
        { query: params.query, count: Math.max(LIMITS.imagesMin, Math.min(params.count ?? LIMITS.imagesMax, LIMITS.imagesMax)) },
        ctx,
      )
  }
}

/** Run one lookup. Throws LookupError (safe to show) when the kind cannot answer. */
export async function lookup(params: LookupParams, ctx: LookupContext): Promise<LookupResult> {
  const result = await run(params, ctx)
  if (!result.card) return result
  const checked = parseAnswerEnvelope(JSON.stringify(result.card))
  return checked.answer && result.credit.trim() ? result : { text: result.text, credit: result.credit }
}
