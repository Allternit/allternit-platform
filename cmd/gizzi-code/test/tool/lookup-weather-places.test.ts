import { beforeEach, describe, expect, test } from "bun:test"
import { clearLookupCache, transport } from "../../src/runtime/primitives/lookup/http"
import { geocode, lookupPlaces } from "../../src/runtime/primitives/lookup/places"
import { lookupWeather } from "../../src/runtime/primitives/lookup/weather"
import { LookupError } from "../../src/runtime/primitives/lookup/types"

const ctx = () => ({ signal: new AbortController().signal, timeZone: "Europe/Oslo" })

const NOMINATIM = [
  { name: "Oslo", display_name: "Oslo, Norway", lat: "59.9133", lon: "10.7389", category: "boundary", type: "administrative" },
  { name: "Cafe A", display_name: "Cafe A, Street 1, Oslo", lat: "59.91", lon: "10.75", category: "amenity", type: "cafe" },
]

function entry(time: string, t: number, sym: string, rain: number) {
  return {
    time,
    data: {
      instant: { details: { air_temperature: t, relative_humidity: 70, wind_speed: 5 } },
      next_1_hours: { summary: { symbol_code: sym }, details: { precipitation_amount: rain } },
    },
  }
}
const MET = {
  properties: {
    timeseries: [
      entry("2026-11-17T10:00:00Z", 4, "partlycloudy_day", 0),
      entry("2026-11-17T12:00:00Z", 6, "rain", 1.5),
      entry("2026-11-18T12:00:00Z", 2, "lightsnow", 0.4),
    ],
  },
}

let urls: string[] = []
let times: number[] = []
function stub() {
  urls = []
  times = []
  transport.fetch = (async (input: string | URL | Request) => {
    const u = String(input)
    urls.push(u)
    times.push(Date.now())
    const body = u.includes("api.met.no") ? MET : u.includes("q=nowhere") ? [] : NOMINATIM
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } })
  }) as typeof transport.fetch
}

beforeEach(() => {
  clearLookupCache()
  stub()
})

describe("lookup weather", () => {
  test("parses MET forecast into the weather card", async () => {
    const r = await lookupWeather({ place: "Oslo", days: 2 }, ctx())
    expect(r.credit).toBe("Weather data: MET Norway")
    expect(r.card?.template).toBe("weather")
    const d = (r.card as any).data
    expect(d.temperature).toBe(4)
    expect(d.unit).toBe("C")
    expect(d.forecast).toHaveLength(2)
    expect(d.forecast[0]).toMatchObject({ high: 6, low: 4 })
    expect(d.forecast[0].precipitation).toBe("1.5 mm")
    expect(r.text).toContain("MET Norway")
  })

  test("cache hit: second call does not refetch; coords rounded to 4 decimals", async () => {
    await lookupWeather({ place: "Oslo" }, ctx())
    const n = urls.length
    await lookupWeather({ place: "Oslo" }, ctx())
    expect(urls.length).toBe(n)
    expect(urls.find((u) => u.includes("api.met.no"))).toContain("lat=59.9133&lon=10.7389")
  })

  test("shared location works; Fahrenheit converts", async () => {
    const r = await lookupWeather({ unit: "F" }, { ...ctx(), location: { lat: 1.23456789, lon: 2.3456789 } })
    expect((r.card as any).data.temperature).toBe(39)
    expect(urls[0]).toContain("lat=1.2346&lon=2.3457")
  })

  test("no place and no location asks for a place", async () => {
    await expect(lookupWeather({}, ctx())).rejects.toThrow(LookupError)
    await expect(lookupWeather({}, ctx())).rejects.toThrow(/Which place/)
  })
})

describe("lookup places", () => {
  test("returns places with credit and map links", async () => {
    const r = await lookupPlaces({ query: "cafe", near: "Oslo", count: 5 }, ctx())
    expect(r.credit).toBe("© OpenStreetMap contributors")
    const d = (r.card as any).data
    expect(d.places[0]).toMatchObject({ name: "Oslo", category: "administrative" })
    expect(d.places.length).toBeLessThanOrEqual(10)
    expect(urls.some((u) => u.includes("bounded=1"))).toBe(true)
  })

  test("missing place is a clear error", async () => {
    await expect(geocode("", ctx())).rejects.toThrow(/Which place/)
    await expect(geocode("nowhere", ctx())).rejects.toThrow(/couldn't find/)
  })

  test("rate limiter spaces Nominatim calls by about 1 s", async () => {
    await geocode("Oslo", ctx())
    await geocode("Bergen", ctx())
    expect(times[1]! - times[0]!).toBeGreaterThanOrEqual(950)
  })
})
