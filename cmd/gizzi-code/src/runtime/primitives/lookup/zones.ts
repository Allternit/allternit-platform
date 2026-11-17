/**
 * lookup kind `time`: the same moment across zones, with local maths over the
 * IANA database Intl ships (no network). "now" uses ctx.now and the turn's
 * zone (ctx.timeZone); a reference like "9am Chicago" converts that moment.
 * Day shifts and DST come from Intl, never hand-written offsets.
 *
 * Lane E1-2b fills this in, with a DST test.
 */
import { LookupError, type LookupContext, type LookupResult, type ZonesParams } from "./types"

export async function lookupZones(params: ZonesParams, ctx: LookupContext): Promise<LookupResult> {
  void params
  void ctx
  throw new LookupError("time", "Time zone lookup is not available yet. Use current_time.")
}
