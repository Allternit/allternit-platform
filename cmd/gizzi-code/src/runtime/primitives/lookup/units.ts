/**
 * lookup kind `convert` for units: local maths, no network (length, mass,
 * volume, temperature, speed, area, data). The card credits the computation
 * ("Computed on this device").
 *
 * Lane E1-2b fills this in, with unit tests.
 */
import { LookupError, type ConvertParams, type LookupContext, type LookupResult } from "./types"

/** True when `unit` is a unit this module converts ("km", "miles", "°F"). */
export function isUnit(unit: string): boolean {
  void unit
  return false
}

export async function lookupUnits(params: ConvertParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("convert", "Unit conversion is not available yet.")
}
