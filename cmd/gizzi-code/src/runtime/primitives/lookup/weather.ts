/**
 * lookup kind `weather`: current conditions and up to LIMITS.forecastDays days
 * from MET Norway Locationforecast 2.0 (free, no key; needs the identifying
 * User-Agent in http.ts; CC BY 4.0, so the card credits MET Norway).
 * Not Open-Meteo: its free tier is non-commercial.
 *
 * Place → coordinates through places.ts `geocode`; no place means the shared
 * location, and without one the lookup asks for a place. Cache: TTL.weather.
 *
 * Lane E1-2a fills this in.
 */
import { LookupError, type LookupContext, type LookupResult, type WeatherParams } from "./types"

export async function lookupWeather(params: WeatherParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("weather", "Weather lookup is not available yet. Use websearch and cite the source.")
}
