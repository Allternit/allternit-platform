/**
 * lookup kind `weather`: current conditions and up to LIMITS.forecastDays days
 * from MET Norway Locationforecast 2.0 (free, no key; needs the identifying
 * User-Agent in http.ts; CC BY 4.0, so the card credits MET Norway).
 * Not Open-Meteo: its free tier is non-commercial.
 *
 * Place → coordinates through places.ts `geocode`; no place means the shared
 * location, and without one the lookup asks for a place.
 *
 * MET's terms: identical requests no more than every 30 min (3 h outside the
 * Nordics), so the cache is keyed by coordinates rounded to 4 decimals and
 * lives 30 min (not TTL.weather's 10 min).
 */
import { fetchJson } from "./http"
import { geocode } from "./places"
import { LIMITS, LookupError, type LookupContext, type LookupResult, type WeatherData, type WeatherParams } from "./types"

export const MET_CREDIT = "Weather data: MET Norway"
export const MET_CREDIT_URL = "https://api.met.no/"
export const WEATHER_CACHE_MS = 30 * 60_000

interface Details {
  air_temperature?: number
  relative_humidity?: number
  wind_speed?: number
  precipitation_amount?: number
}
interface Period {
  summary?: { symbol_code?: string }
  details?: Details
}
interface Entry {
  time: string
  data: { instant: { details: Details }; next_1_hours?: Period; next_6_hours?: Period; next_12_hours?: Period }
}
interface MetResponse {
  properties?: { timeseries?: Entry[] }
}

const SYMBOLS: Record<string, string> = {
  clearsky: "Clear",
  fair: "Mostly clear",
  partlycloudy: "Partly cloudy",
  cloudy: "Cloudy",
  fog: "Fog",
  rain: "Rain",
  lightrain: "Light rain",
  heavyrain: "Heavy rain",
  rainshowers: "Rain showers",
  lightrainshowers: "Light rain showers",
  heavyrainshowers: "Heavy rain showers",
  sleet: "Sleet",
  lightsleet: "Light sleet",
  heavysleet: "Heavy sleet",
  snow: "Snow",
  lightsnow: "Light snow",
  heavysnow: "Heavy snow",
  snowshowers: "Snow showers",
  lightsnowshowers: "Light snow showers",
  heavysnowshowers: "Heavy snow showers",
}

export function describeSymbol(code?: string): string {
  if (!code) return "Unknown"
  const base = code.replace(/_(day|night|polartwilight)$/, "")
  if (SYMBOLS[base]) return SYMBOLS[base]
  const thunder = base.includes("andthunder")
  const rest = base.replace(/andthunder$/, "")
  const text = SYMBOLS[rest] ?? rest
  return thunder ? `${text} with thunder` : text
}

const round4 = (n: number) => n.toFixed(4)
const c2f = (c: number) => Math.round((c * 9) / 5 + 32)

/** Default unit: Fahrenheit for US zones, else Celsius. */
function defaultUnit(ctx: LookupContext): "C" | "F" {
  const z = ctx.timeZone ?? ""
  return /^(America\/(New_York|Chicago|Denver|Los_Angeles|Phoenix|Anchorage|Detroit|Boise|Indiana|Kentucky|North_Dakota)|US\/|Pacific\/Honolulu)/.test(z)
    ? "F"
    : "C"
}

export async function lookupWeather(params: WeatherParams, ctx: LookupContext): Promise<LookupResult> {
  let lat: number
  let lon: number
  let label: string
  if (params.place?.trim()) {
    const g = await geocode(params.place, ctx)
    ;({ lat, lon, label } = g)
  } else if (ctx.location) {
    lat = ctx.location.lat
    lon = ctx.location.lon
    label = ctx.location.label ?? "your location"
  } else {
    throw new LookupError("weather", "Which place? Name a city; I can't tell where you are unless you share your location.")
  }

  const url = `https://api.met.no/weatherapi/locationforecast/2.0/compact?lat=${round4(lat)}&lon=${round4(lon)}`
  const body = await fetchJson<MetResponse>(url, { signal: ctx.signal, ttlMs: WEATHER_CACHE_MS })
  const series = body.properties?.timeseries
  if (!series?.length) throw new LookupError("weather", `No forecast came back for ${label}. Try again later or use websearch.`)

  const unit = params.unit ?? defaultUnit(ctx)
  const temp = (c: number) => (unit === "F" ? c2f(c) : Math.round(c))
  const now = series[0]!
  const nd = now.data.instant.details
  const nowC = nd.air_temperature
  if (nowC === undefined) throw new LookupError("weather", `The forecast for ${label} had no temperature.`)
  const windMs = nd.wind_speed ?? 0
  const wind = unit === "F" ? `${Math.round(windMs * 2.23694)} mph` : `${Math.round(windMs * 3.6)} km/h`
  const conditions = describeSymbol(
    now.data.next_1_hours?.summary?.symbol_code ?? now.data.next_6_hours?.summary?.symbol_code ?? now.data.next_12_hours?.summary?.symbol_code,
  )

  const tz = ctx.timeZone ?? "UTC"
  const dayKey = new Intl.DateTimeFormat("en-CA", { timeZone: tz, year: "numeric", month: "2-digit", day: "2-digit" })
  const dayName = new Intl.DateTimeFormat("en-US", { timeZone: tz, weekday: "short" })
  const hourOf = new Intl.DateTimeFormat("en-US", { timeZone: tz, hour: "numeric", hourCycle: "h23" })
  const days = new Map<string, { name: string; hi: number; lo: number; sym?: string; symDist: number; rain: number }>()
  for (const e of series) {
    const t = e.data.instant.details.air_temperature
    if (t === undefined) continue
    const d = new Date(e.time)
    const key = dayKey.format(d)
    let day = days.get(key)
    if (!day) days.set(key, (day = { name: dayName.format(d), hi: t, lo: t, symDist: 99, rain: 0 }))
    day.hi = Math.max(day.hi, t)
    day.lo = Math.min(day.lo, t)
    const p = e.data.next_1_hours ?? e.data.next_6_hours
    day.rain += p?.details?.precipitation_amount ?? 0
    const dist = Math.abs(Number(hourOf.format(d)) - 12)
    const sym = e.data.next_6_hours?.summary?.symbol_code ?? e.data.next_1_hours?.summary?.symbol_code
    if (sym && dist < day.symDist) ((day.sym = sym), (day.symDist = dist))
  }
  const wanted = Math.max(0, Math.min(LIMITS.forecastDays, Math.floor(params.days ?? LIMITS.forecastDays)))
  const forecast = [...days.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .slice(0, wanted)
    .map(([, d]) => ({
      day: d.name,
      high: temp(d.hi),
      low: temp(d.lo),
      conditions: describeSymbol(d.sym),
      precipitation: d.rain >= 0.1 ? (unit === "F" ? `${(d.rain / 25.4).toFixed(2)} in` : `${d.rain.toFixed(1)} mm`) : undefined,
    }))

  const data: WeatherData = {
    location: label,
    observedAt: now.time,
    temperature: temp(nowC),
    unit,
    conditions,
    humidity: Math.round(nd.relative_humidity ?? 0),
    wind,
    source: "MET Norway",
    ...(forecast.length ? { forecast } : {}),
    credit: MET_CREDIT,
    creditUrl: MET_CREDIT_URL,
  }
  const text =
    `Weather in ${label}: ${data.temperature}°${unit}, ${conditions.toLowerCase()}, humidity ${data.humidity}%, wind ${wind}.` +
    (forecast.length
      ? `\nForecast:\n` + forecast.map((f) => `${f.day}: ${f.high}°/${f.low}°${unit}, ${f.conditions.toLowerCase()}${f.precipitation ? `, ${f.precipitation}` : ""}`).join("\n")
      : "") +
    `\n${MET_CREDIT} (${MET_CREDIT_URL})`
  return {
    text,
    credit: MET_CREDIT,
    card: { version: 1, template: "weather", title: `Weather in ${label}`, summary: `${data.temperature}°${unit}, ${conditions}`, data },
  }
}
