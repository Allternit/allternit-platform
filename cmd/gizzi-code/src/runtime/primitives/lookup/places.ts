/**
 * lookup kind `places`, and the geocoder weather.ts uses: OpenStreetMap
 * Nominatim (free, no key). Its policy: at most 1 request/second
 * (http.ts `minIntervalMs`), an identifying User-Agent, results cached
 * (TTL.geocode), and the "© OpenStreetMap contributors" credit on every card.
 * Up to LIMITS.placesMax results, each with a map link.
 *
 * "Here" works only with the shared location (ctx.location); otherwise the
 * lookup asks for a place.
 *
 * Lane E1-2a fills this in.
 */
import { LookupError, type ClientLocation, type LookupContext, type LookupResult, type PlacesParams } from "./types"

/** One place name → coordinates and a display label. Throws LookupError when nothing matches. */
export async function geocode(place: string, ctx: LookupContext): Promise<ClientLocation & { label: string }> {
  void place
  void ctx
  throw new LookupError("places", "Place search is not available yet.")
}

export async function lookupPlaces(params: PlacesParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("places", "Place search is not available yet. Use websearch and cite the source.")
}
