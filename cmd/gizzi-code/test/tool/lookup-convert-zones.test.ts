import { beforeEach, describe, expect, test } from "bun:test"
import { clearLookupCache, transport } from "../../src/runtime/primitives/lookup/http"
import { lookupFx, parseFrankfurter, isCurrency } from "../../src/runtime/primitives/lookup/fx"
import { convertUnits, isUnit, lookupUnits } from "../../src/runtime/primitives/lookup/units"
import { lookupZones } from "../../src/runtime/primitives/lookup/zones"

const ctx = (now = "2026-11-17T20:03:05Z") => ({ signal: new AbortController().signal, now: () => new Date(now) })

describe("units", () => {
  test("length, mass, data", () => {
    expect(convertUnits(5, "km", "mi").value).toBeCloseTo(3.10686, 4)
    expect(convertUnits(1, "lb", "oz").value).toBeCloseTo(16, 6)
    expect(convertUnits(1, "GiB", "MB").value).toBeCloseTo(1073.741824, 6)
    expect(convertUnits(90, "min", "h").value).toBeCloseTo(1.5, 9)
  })
  test("temperature uses offsets", () => {
    expect(convertUnits(100, "°C", "°F").value).toBeCloseTo(212, 9)
    expect(convertUnits(-40, "F", "C").value).toBeCloseTo(-40, 9)
    expect(convertUnits(0, "C", "K").value).toBeCloseTo(273.15, 9)
  })
  test("mismatched and unknown units are refused", async () => {
    await expect(lookupUnits({ amount: 1, from: "kg", to: "km" }, ctx())).rejects.toThrow(/different things/)
    await expect(lookupUnits({ amount: 1, from: "parsec", to: "km" }, ctx())).rejects.toThrow(/not a unit/)
    expect(isUnit("miles")).toBe(true)
    expect(isUnit("usd")).toBe(false)
  })
  test("result carries text, card and credit", async () => {
    const r = await lookupUnits({ amount: 10, from: "km", to: "mi" }, ctx())
    expect(r.text).toContain("10 km = 6.21371 mi")
    expect(r.card?.template).toBe("conversion")
    expect(r.credit).toBe("Computed on this device")
  })
})

describe("fx", () => {
  const body = { amount: 1, base: "USD", date: "2026-11-16", rates: { EUR: 0.86, GBP: 0.75, JPY: 150 } }
  let calls = 0
  beforeEach(() => {
    clearLookupCache()
    calls = 0
    transport.fetch = (async () => {
      calls++
      return new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } })
    }) as any
  })
  test("parses a fixture and shows the rate date and credit", async () => {
    const r = await lookupFx({ amount: 100, from: "usd", to: "eur" }, ctx())
    expect(r.text).toContain("100.00 USD = 86.00 EUR")
    expect(r.text).toContain("2026-11-16")
    expect(r.credit).toBe("Rates: European Central Bank via Frankfurter")
    const d = (r.card as any).data
    expect(d.kind).toBe("currency")
    expect(d.asOf).toBe("2026-11-16")
    expect(d.rate).toBe(0.86)
  })
  test("second identical call is served from cache", async () => {
    await lookupFx({ amount: 1, from: "USD", to: "EUR" }, ctx())
    await lookupFx({ amount: 5, from: "USD", to: "EUR" }, ctx())
    expect(calls).toBe(1)
  })
  test("unknown currency refused; bad body rejected", async () => {
    await expect(lookupFx({ amount: 1, from: "USD", to: "XYZ" }, ctx())).rejects.toThrow(/not a currency/)
    expect(() => parseFrankfurter({})).toThrow()
    expect(isCurrency("jpy")).toBe(true)
  })
})

describe("zones", () => {
  test("now across zones", async () => {
    const r = await lookupZones({ zones: ["America/New_York", "Tokyo"] }, ctx())
    const z = (r.card as any).data.zones
    expect(z[0].time).toBe("15:03")
    expect(z[0].offset).toBe("UTC-05:00")
    expect(z[1].time).toBe("05:03")
    expect(z[1].date).toContain("Nov 18")
  })
  test("9am Chicago in Tokyo", async () => {
    const r = await lookupZones({ query: "9am Chicago", zones: ["Chicago", "Tokyo"] }, ctx())
    const z = (r.card as any).data.zones
    expect(z[0].time).toBe("09:00")
    expect(z[1].time).toBe("00:00")
  })
  test("DST boundary: New York on 2026-03-08", async () => {
    const before = await lookupZones({ query: "2026-03-08 01:30", zones: ["America/New_York", "UTC"] }, ctx())
    const after = await lookupZones({ query: "2026-03-08 03:30", zones: ["America/New_York", "UTC"] }, ctx())
    const b = (before.card as any).data.zones
    const a = (after.card as any).data.zones
    expect(b[0].offset).toBe("UTC-05:00")
    expect(a[0].offset).toBe("UTC-04:00")
    expect(b[1].time).toBe("01:30")
    expect(a[0].time).toBe("03:30")
  })
  test("unknown zone and unreadable time are refused", async () => {
    await expect(lookupZones({ zones: ["Atlantis"] }, ctx())).rejects.toThrow(/not a zone/)
    await expect(lookupZones({ query: "soonish", zones: ["Tokyo"] }, ctx())).rejects.toThrow(/Could not read/)
  })
})
