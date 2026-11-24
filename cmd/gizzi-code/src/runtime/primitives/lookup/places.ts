/**
 * lookup kind `places`, and the geocoder weather.ts uses: OpenStreetMap
 * Nominatim (free, no key). Its policy: at most 1 request/second
 * (http.ts `minIntervalMs`), an identifying User-Agent, results cached
 * (TTL.geocode), and the "© OpenStreetMap contributors" credit on every card.
 * Up to LIMITS.placesMax results, each with a map link.
 *
 * Search runs only on an explicit request, never as type-ahead.
 * "Here" works only with the shared location (ctx.location); otherwise the
 * lookup asks for a place.
 */
import { fetchJson } from "./http"
import { LIMITS, LookupError, TTL, type ClientLocation, type LookupContext, type LookupResult, type PlacesParams } from "./types"

export const OSM_CREDIT = "© OpenStreetMap contributors"
export const OSM_CREDIT_URL = "https://www.openstreetmap.org/copyright"
const BASE = "https://nominatim.openstreetmap.org/search"

interface NominatimHit {
  name?: string
  display_name?: string
  lat: string
  lon: string
  category?: string
  type?: string
  osm_type?: string
  osm_id?: number
}

function search(q: string, limit: number, ctx: LookupContext, extra = ""): Promise<NominatimHit[]> {
  const url = `${BASE}?q=${encodeURIComponent(q)}&format=jsonv2&limit=${limit}${extra}`
  return fetchJson<NominatimHit[]>(url, { signal: ctx.signal, ttlMs: TTL.geocode, minIntervalMs: 1000 })
}

function humanize(s?: string): string | undefined {
  return s ? s.replace(/_/g, " ") : undefined
}

/** One place name → coordinates and a display label. Throws LookupError when nothing matches. */
export async function geocode(place: string, ctx: LookupContext): Promise<ClientLocation & { label: string }> {
  const q = place.trim()
  if (!q) throw new LookupError("places", "Which place? Name a city or address; I can't tell where you are unless you share your location.")
  const hits = await search(q, 1, ctx)
  const hit = hits[0]
  const lat = hit ? Number(hit.lat) : NaN
  const lon = hit ? Number(hit.lon) : NaN
  if (!hit || !Number.isFinite(lat) || !Number.isFinite(lon)) {
    throw new LookupError("places", `I couldn't find a place called "${q}". Try a nearby city or add the country.`)
  }
  const label = hit.name || hit.display_name?.split(",")[0]?.trim() || q
  return { lat, lon, label }
}

export async function lookupPlaces(params: PlacesParams, ctx: LookupContext): Promise<LookupResult> {
  const query = params.query?.trim()
  if (!query) throw new LookupError("places", "What kind of place? Say what to look for, such as \"coffee shops near Lyon\".")
  const count = Math.max(1, Math.min(LIMITS.placesMax, Math.floor(params.count ?? LIMITS.placesMax)))

  let near: (ClientLocation & { label?: string }) | undefined
  if (params.near?.trim()) near = await geocode(params.near, ctx)
  else if (ctx.location) near = ctx.location

  let extra = ""
  if (near) {
    const d = 0.2
    const f = (n: number) => n.toFixed(4)
    extra = `&viewbox=${f(near.lon - d)},${f(near.lat + d)},${f(near.lon + d)},${f(near.lat - d)}&bounded=1`
  }
  const hits = await search(query, count, ctx, extra)
  const places = hits.slice(0, count).map((h) => {
    const lat = Number(h.lat)
    const lon = Number(h.lon)
    const address = h.display_name ?? ""
    const name = h.name || address.split(",")[0]?.trim() || query
    return {
      name,
      address,
      category: humanize(h.type) ?? humanize(h.category),
      lat: Number.isFinite(lat) ? lat : undefined,
      lon: Number.isFinite(lon) ? lon : undefined,
      url: Number.isFinite(lat) && Number.isFinite(lon) ? `https://www.openstreetmap.org/?mlat=${lat}&mlon=${lon}#map=17/${lat}/${lon}` : undefined,
    }
  })
  const nearLabel = params.near?.trim() || near?.label
  if (!places.length) {
    throw new LookupError("places", `No results for "${query}"${nearLabel ? ` near ${nearLabel}` : ""}. Try a broader search term.`)
  }
  const title = `${query}${nearLabel ? ` near ${nearLabel}` : ""}`
  const text =
    `${places.length} result${places.length === 1 ? "" : "s"} for ${title}:\n` +
    places.map((p, i) => `${i + 1}. ${p.name}${p.category ? ` (${p.category})` : ""} - ${p.address}`).join("\n") +
    `\n${OSM_CREDIT}`
  return {
    text,
    credit: OSM_CREDIT,
    card: {
      version: 1,
      template: "places",
      title,
      summary: `${places.length} place${places.length === 1 ? "" : "s"}`,
      data: { query, near: nearLabel, places, credit: OSM_CREDIT, creditUrl: OSM_CREDIT_URL },
    },
  }
}
