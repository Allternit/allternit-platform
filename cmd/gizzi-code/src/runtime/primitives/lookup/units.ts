/**
 * lookup kind `convert` for units: local maths, no network (length, mass,
 * volume, temperature, speed, area, data, time). The card credits the
 * computation ("Computed on this device"). Unknown units and mixed dimensions
 * are refused with a message that names the problem.
 */
import { LIMITS, LookupError, type ConvertParams, type LookupContext, type LookupResult } from "./types"

type Dimension = "length" | "mass" | "volume" | "temperature" | "speed" | "area" | "data" | "time"
interface Unit {
  dim: Dimension
  /** Canonical symbol shown to the user. */
  symbol: string
  /** Multiply by this to reach the dimension's base unit (not used for temperature). */
  factor: number
}

const UNITS = new Map<string, Unit>()
function def(dim: Dimension, symbol: string, factor: number, names: string[]) {
  const u: Unit = { dim, symbol, factor }
  for (const n of [symbol, ...names]) UNITS.set(n.toLowerCase(), u)
}

// length (base m)
def("length", "mm", 0.001, ["millimeter", "millimeters", "millimetre", "millimetres"])
def("length", "cm", 0.01, ["centimeter", "centimeters", "centimetre", "centimetres"])
def("length", "m", 1, ["meter", "meters", "metre", "metres"])
def("length", "km", 1000, ["kilometer", "kilometers", "kilometre", "kilometres"])
def("length", "in", 0.0254, ["inch", "inches", '"'])
def("length", "ft", 0.3048, ["foot", "feet", "'"])
def("length", "yd", 0.9144, ["yard", "yards"])
def("length", "mi", 1609.344, ["mile", "miles"])
def("length", "nmi", 1852, ["nautical mile", "nautical miles"])
// mass (base kg)
def("mass", "mg", 1e-6, ["milligram", "milligrams"])
def("mass", "g", 0.001, ["gram", "grams"])
def("mass", "kg", 1, ["kilogram", "kilograms", "kilo", "kilos"])
def("mass", "t", 1000, ["tonne", "tonnes", "metric ton", "metric tons"])
def("mass", "oz", 0.028349523125, ["ounce", "ounces"])
def("mass", "lb", 0.45359237, ["pound", "pounds", "lbs"])
def("mass", "st", 6.35029318, ["stone", "stones"])
// volume (base L)
def("volume", "mL", 0.001, ["ml", "milliliter", "milliliters", "millilitre", "millilitres"])
def("volume", "L", 1, ["l", "liter", "liters", "litre", "litres"])
def("volume", "tsp", 0.00492892159375, ["teaspoon", "teaspoons"])
def("volume", "tbsp", 0.01478676478125, ["tablespoon", "tablespoons"])
def("volume", "fl oz", 0.0295735295625, ["floz", "fluid ounce", "fluid ounces"])
def("volume", "cup", 0.2365882365, ["cups"])
def("volume", "pt", 0.473176473, ["pint", "pints"])
def("volume", "qt", 0.946352946, ["quart", "quarts"])
def("volume", "gal", 3.785411784, ["gallon", "gallons"])
// speed (base m/s)
def("speed", "m/s", 1, ["mps", "meters per second", "metres per second"])
def("speed", "km/h", 1 / 3.6, ["kmh", "kph", "kilometers per hour", "kilometres per hour"])
def("speed", "mph", 0.44704, ["mi/h", "miles per hour"])
def("speed", "kn", 1852 / 3600, ["knot", "knots", "kt", "kts"])
def("speed", "ft/s", 0.3048, ["fps", "feet per second"])
// area (base m²)
def("area", "m²", 1, ["m2", "sqm", "square meter", "square meters", "square metre", "square metres"])
def("area", "cm²", 1e-4, ["cm2", "square centimeter", "square centimeters"])
def("area", "km²", 1e6, ["km2", "sq km", "square kilometer", "square kilometers"])
def("area", "ft²", 0.09290304, ["ft2", "sqft", "sq ft", "square foot", "square feet"])
def("area", "in²", 0.00064516, ["in2", "sq in", "square inch", "square inches"])
def("area", "yd²", 0.83612736, ["yd2", "square yard", "square yards"])
def("area", "mi²", 2589988.110336, ["mi2", "sq mi", "square mile", "square miles"])
def("area", "ha", 10000, ["hectare", "hectares"])
def("area", "ac", 4046.8564224, ["acre", "acres"])
// data (base byte; kB = 1000, KiB = 1024)
def("data", "bit", 0.125, ["bits"])
def("data", "B", 1, ["byte", "bytes"])
def("data", "kB", 1e3, ["kb", "kilobyte", "kilobytes"])
def("data", "MB", 1e6, ["mb", "megabyte", "megabytes"])
def("data", "GB", 1e9, ["gb", "gigabyte", "gigabytes"])
def("data", "TB", 1e12, ["tb", "terabyte", "terabytes"])
def("data", "KiB", 1024, ["kib", "kibibyte", "kibibytes"])
def("data", "MiB", 1024 ** 2, ["mib", "mebibyte", "mebibytes"])
def("data", "GiB", 1024 ** 3, ["gib", "gibibyte", "gibibytes"])
def("data", "TiB", 1024 ** 4, ["tib", "tebibyte", "tebibytes"])
// time (base s)
def("time", "ms", 0.001, ["millisecond", "milliseconds"])
def("time", "s", 1, ["sec", "secs", "second", "seconds"])
def("time", "min", 60, ["mins", "minute", "minutes"])
def("time", "h", 3600, ["hr", "hrs", "hour", "hours"])
def("time", "d", 86400, ["day", "days"])
def("time", "wk", 604800, ["week", "weeks"])
def("time", "yr", 31557600, ["year", "years"])

// temperature (offsets, so handled apart)
const TEMPS = new Map<string, "C" | "F" | "K">()
for (const n of ["c", "°c", "celsius", "degc", "deg c", "degrees celsius", "degree celsius"]) TEMPS.set(n, "C")
for (const n of ["f", "°f", "fahrenheit", "degf", "deg f", "degrees fahrenheit", "degree fahrenheit"]) TEMPS.set(n, "F")
for (const n of ["k", "kelvin", "kelvins"]) TEMPS.set(n, "K")

const norm = (s: string) => s.trim().toLowerCase().replace(/\s+/g, " ")

/** Case-sensitive ambiguity: "Mb"/"MB" differ only by case in real life, but the table keeps one meaning per lowercase name. */
function resolve(unit: string): { dim: Dimension; symbol: string; unit?: Unit; temp?: "C" | "F" | "K" } | undefined {
  const n = norm(unit)
  const t = TEMPS.get(n)
  if (t) return { dim: "temperature", symbol: `°${t === "K" ? "" : ""}${t}`.replace("°K", "K"), temp: t }
  const u = UNITS.get(n)
  return u ? { dim: u.dim, symbol: u.symbol, unit: u } : undefined
}

/** True when `unit` is a unit this module converts ("km", "miles", "°F"). */
export function isUnit(unit: string): boolean {
  return resolve(unit) !== undefined
}

function toC(v: number, t: "C" | "F" | "K"): number {
  return t === "C" ? v : t === "F" ? ((v - 32) * 5) / 9 : v - 273.15
}
function fromC(c: number, t: "C" | "F" | "K"): number {
  return t === "C" ? c : t === "F" ? (c * 9) / 5 + 32 : c + 273.15
}

/** Round to 6 significant digits, trimming float noise. */
function sig(n: number): number {
  if (n === 0 || !Number.isFinite(n)) return n
  return Number(n.toPrecision(6))
}

function fmt(n: number): string {
  const r = sig(n)
  return Math.abs(r) >= 1e15 || (Math.abs(r) < 1e-6 && r !== 0)
    ? r.toExponential(4)
    : r.toLocaleString("en-US", { maximumFractionDigits: 8 })
}

export function convertUnits(amount: number, from: string, to: string): { value: number; from: string; to: string; dim: Dimension } {
  const a = resolve(from)
  const b = resolve(to)
  if (!a) throw new LookupError("convert", `"${from}" is not a unit this lookup knows. Try a standard unit such as km, lb, L, °F, mph, m², GB or h.`)
  if (!b) throw new LookupError("convert", `"${to}" is not a unit this lookup knows. Try a standard unit such as km, lb, L, °F, mph, m², GB or h.`)
  if (a.dim !== b.dim)
    throw new LookupError("convert", `Cannot convert ${a.dim} (${a.symbol}) to ${b.dim} (${b.symbol}); the units measure different things.`)
  let value: number
  if (a.temp && b.temp) {
    value = fromC(toC(amount, a.temp), b.temp)
    if (toC(amount, a.temp) < -273.15 - 1e-9) throw new LookupError("convert", "That temperature is below absolute zero.")
  } else {
    value = (amount * a.unit!.factor) / b.unit!.factor
  }
  return { value, from: a.symbol, to: b.symbol, dim: a.dim }
}

const ALSO: Partial<Record<Dimension, string[]>> = {
  length: ["mm", "cm", "m", "km", "in", "ft", "yd", "mi"],
  mass: ["g", "kg", "oz", "lb"],
  volume: ["mL", "L", "cup", "pt", "qt", "gal"],
  temperature: ["C", "F", "K"],
  speed: ["m/s", "km/h", "mph", "kn"],
  area: ["m²", "ft²", "ac", "ha"],
  data: ["kB", "MB", "GB", "KiB", "MiB", "GiB"],
  time: ["min", "h", "d", "wk"],
}

export async function lookupUnits(params: ConvertParams, _ctx: LookupContext): Promise<LookupResult> {
  if (!Number.isFinite(params.amount)) throw new LookupError("convert", "The amount must be a number.")
  const r = convertUnits(params.amount, params.from, params.to)
  const also = (ALSO[r.dim] ?? [])
    .filter((s) => resolve(s)!.symbol !== r.from && resolve(s)!.symbol !== r.to)
    .slice(0, LIMITS.conversionsAlsoMax)
    .map((s) => {
      const x = convertUnits(params.amount, params.from, s)
      return { value: sig(x.value), unit: x.to }
    })
  const credit = "Computed on this device"
  const value = sig(r.value)
  const text = `${fmt(params.amount)} ${r.from} = ${fmt(value)} ${r.to} (${credit}).`
  return {
    text,
    credit,
    card: {
      version: 1,
      template: "conversion",
      title: `${r.from} to ${r.to}`,
      summary: `${fmt(params.amount)} ${r.from} = ${fmt(value)} ${r.to}`,
      data: {
        kind: "unit",
        from: { value: params.amount, unit: r.from },
        to: { value, unit: r.to },
        ...(also.length ? { also } : {}),
        credit,
      },
    },
  }
}
