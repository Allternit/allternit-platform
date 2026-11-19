/**
 * lookup: one fact (weather, time, conversion, link preview, places, images)
 * as text plus a validated answer card (runtime/primitives/lookup). Clients
 * draw the card from the tool part's `metadata.card`; the text stands alone
 * for clients that draw none. Shares lookup.txt with the Ink adapter.
 *
 * Permissions: time and unit conversion are local and ask nothing. Network
 * kinds ask under the existing `websearch` permission (`webfetch` for a link
 * preview), so they follow the rules the user already set for the web.
 *
 * "Here": weather and places use the turn's shared location (TurnLocation,
 * from allternit-api's opt-in client_location). Without one the call asks
 * the model to get a place from the user, before any permission or fetch.
 */
import z from "zod/v4"
import { LabelField } from "@/runtime/primitives/gap-types"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Clock } from "@/runtime/primitives/clock"
import { LOOKUP_KINDS, LookupError, isLocalLookup, lookup, placeOrHere, type ClientLocation, type LookupCard, type LookupKind } from "@/runtime/primitives/lookup"
import DESCRIPTION from "@/runtime/tools/builtins/lookup.txt"

export type LookupToolMetadata = { kind: LookupKind; card?: LookupCard; credit?: string; error?: string }

export const LOOKUP_PARAMETERS = z.object({
  ...LabelField,
  kind: z.enum(LOOKUP_KINDS).describe("What to look up: weather, time, convert, link, places or images"),
  query: z.string().min(1).max(500).describe("The place, search, URL or moment, depending on kind"),
  place: z.string().max(200).optional().describe("Where: the place for weather, or where to search for places"),
  zones: z.array(z.string().min(1).max(64)).max(12).optional().describe("For time: IANA zones or place names"),
  amount: z.number().finite().optional().describe("For convert: the amount to convert"),
  from: z.string().min(1).max(32).optional().describe('For convert: the unit or currency code to convert from, e.g. "mi" or "USD"'),
  to: z.string().min(1).max(32).optional().describe('For convert: the unit or currency code to convert to, e.g. "km" or "EUR"'),
  count: z.number().int().min(1).max(10).optional().describe("Forecast days (weather, up to 7), places (up to 10) or images (3 to 5)"),
})

export type LookupToolParams = z.infer<typeof LOOKUP_PARAMETERS>

/**
 * The user's opt-in shared location for the turn now running, structured.
 * allternit-api validates and rounds `client_location` (two decimals, about
 * 1 km) and sends it as the turn-only `turnLocation` field
 * (`{ lat, lon, accuracy_m, time_zone, at, place? }`, client_location::turn_value)
 * beside the prose line it adds to `turnSystem` for the model.
 * SessionPrompt reads it once per turn (resolveTurnLocation), holds it here
 * for the turn and hands it to native tools as `ctx.extra.location`.
 * In memory only: never stored on a message, never logged.
 */
export interface TurnLocation {
  lat: number
  lon: number
  /** The radius allternit-api states, in km (never finer than 1). */
  accuracyKm?: number
  /** The device's IANA zone when it was shared. */
  timeZone?: string
  /** A place name, when the client sent one. */
  place?: string
  /** Shared precise for this turn only (the person tapped "Share precise location"). */
  precise?: boolean
  /** Radius in metres for a precise share (never finer than 10). */
  accuracyM?: number
}

const LOCATION_LINE =
  /Where the user is \([^)]*\):\s*about\s+(-?\d{1,3}(?:\.\d+)?),\s*(-?\d{1,3}(?:\.\d+)?)(?:,\s*within roughly\s+(\d+(?:\.\d+)?)\s*km)?\.(?:\s*Their time zone:\s*([A-Za-z0-9_+\-/]{1,64})\.)?/

/**
 * Legacy fallback only: the location line in a turnSystem layer, from an
 * allternit-api that predates `turnLocation` (Desktop runtimes and cloud
 * computers can run an older API than this runtime). Its wording is frozen by
 * client_location.rs's `valid_location_gives_layer_text` test. A current API
 * always sends `turnLocation`, so a wording change there cannot break "here".
 */
export function parseTurnLocation(layer: string | undefined): TurnLocation | undefined {
  const m = layer ? LOCATION_LINE.exec(layer) : null
  if (!m) return undefined
  return turnLocationOf({ lat: Number(m[1]), lon: Number(m[2]), accuracyKm: m[3] ? Number(m[3]) : undefined, timeZone: m[4] })
}

/**
 * The checked TurnLocation in allternit-api's turn-only `turnLocation` field
 * (`{ lat, lon, accuracy_m?, time_zone?, place?, at? }`), or undefined. A bad
 * value is ignored, never an error: a bad location never fails a turn.
 */
export function turnLocationFromField(raw: unknown): TurnLocation | undefined {
  if (!raw || typeof raw !== "object") return undefined
  const v = raw as Record<string, unknown>
  const meters = typeof v.accuracy_m === "number" && Number.isFinite(v.accuracy_m) && v.accuracy_m > 0 ? v.accuracy_m : undefined
  const precise = v.precision === "precise"
  return turnLocationOf({
    lat: v.lat,
    lon: v.lon,
    // Never finer than 1 km, as allternit-api states it to the model, unless shared precise for this turn.
    accuracyKm: meters === undefined ? undefined : Math.max(1, Math.round(meters / 1000)),
    timeZone: v.time_zone,
    place: v.place,
    ...(precise ? { precise: true, accuracyM: meters === undefined ? 10 : Math.max(10, Math.round(meters)) } : {}),
  })
}

/**
 * The turn's shared location: the structured `turnLocation` field when the
 * API sent one; only when the field is absent (an older API) the location
 * line in `turnSystem`.
 */
export function resolveTurnLocation(field: unknown, turnSystem: string | undefined): TurnLocation | undefined {
  if (field !== undefined && field !== null) return turnLocationFromField(field)
  return parseTurnLocation(turnSystem)
}

/** A checked TurnLocation from any value (ctx.extra.location), or undefined. */
export function turnLocationOf(raw: unknown): TurnLocation | undefined {
  if (!raw || typeof raw !== "object") return undefined
  const v = raw as Record<string, unknown>
  const lat = v.lat
  const lon = v.lon
  if (typeof lat !== "number" || typeof lon !== "number" || !Number.isFinite(lat) || !Number.isFinite(lon)) return undefined
  if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return undefined
  const accuracyKm = typeof v.accuracyKm === "number" && Number.isFinite(v.accuracyKm) && v.accuracyKm > 0 ? v.accuracyKm : undefined
  const timeZone = Clock.validZone(v.timeZone)
  const place = typeof v.place === "string" && v.place.trim() ? v.place.trim().slice(0, 200) : undefined
  const precise = v.precise === true
  const accuracyM = precise && typeof v.accuracyM === "number" && Number.isFinite(v.accuracyM) ? Math.max(10, v.accuracyM) : undefined
  return {
    lat,
    lon,
    ...(accuracyKm ? { accuracyKm } : {}),
    ...(timeZone ? { timeZone } : {}),
    ...(place ? { place } : {}),
    ...(precise ? { precise: true, ...(accuracyM ? { accuracyM } : {}) } : {}),
  }
}

/**
 * Turn locations by session, set by SessionPrompt.prompt for the length of
 * one turn. Native tools get the value on `ctx.extra.location`; a CLI brain's
 * bridged call (cli-bridge, no extra) reads it here by session.
 */
export namespace TurnLocations {
  const bySession = new Map<string, TurnLocation>()
  export function set(sessionID: string, location: TurnLocation) {
    bySession.set(sessionID, location)
  }
  export function get(sessionID: string): TurnLocation | undefined {
    return bySession.get(sessionID)
  }
  /** Drop the session's location, unless a newer turn already replaced it. */
  export function release(sessionID: string, location: TurnLocation) {
    if (bySession.get(sessionID) === location) bySession.delete(sessionID)
  }
}

const ASK_FOR_PLACE: Partial<Record<LookupKind, string>> = {
  weather: "Which place? Name a city; I can't tell where you are unless you share your location.",
  places: "Which place? Name a city or address; I can't tell where you are unless you share your location.",
}

/** True when the call means "here" (no place, or "here"/"near me") for a kind that needs a place. */
function meansHere(params: LookupToolParams): boolean {
  if (params.kind === "weather") return placeOrHere(params.place ?? params.query) === undefined
  // places: no `place` searches everywhere; an explicit "here"/"near me" means the shared location.
  if (params.kind === "places") return !!params.place?.trim() && placeOrHere(params.place) === undefined
  return false
}

export const LookupTool = Tool.define<typeof LOOKUP_PARAMETERS, LookupToolMetadata>("lookup", {
  description: DESCRIPTION,
  parameters: LOOKUP_PARAMETERS,
  async execute(params, ctx) {
    const shared = turnLocationOf(ctx.extra?.location) ?? TurnLocations.get(ctx.sessionID)
    if (!shared && meansHere(params)) {
      // No shared location: nothing to look up yet, so ask nothing and fetch nothing.
      const message = ASK_FOR_PLACE[params.kind]!
      return { title: `lookup ${params.kind} needs a place`, output: message, metadata: { kind: params.kind, error: message } }
    }
    if (!isLocalLookup(params)) {
      const link = params.kind === "link"
      await ctx.ask({
        permission: link ? "webfetch" : "websearch",
        patterns: [link ? params.query : `lookup:${params.kind}`],
        always: ["*"],
        metadata: { lookup: params.kind, query: params.query },
      })
    }
    const lastUser = ctx.messages.findLast((message) => message.info.role === "user")
    const timeZone = Clock.turnZoneOf(lastUser?.info.role === "user" ? lastUser.info.metadata : undefined) ?? shared?.timeZone
    const location: ClientLocation | undefined = shared
      ? { lat: shared.lat, lon: shared.lon, ...(shared.place ? { label: shared.place } : {}) }
      : undefined
    try {
      const result = await lookup(params, { signal: ctx.abort, timeZone, location })
      return {
        title: result.card?.title ?? `lookup ${params.kind}`,
        output: `${result.text}\n\nCredit: ${result.credit}`,
        metadata: { kind: params.kind, card: result.card, credit: result.credit },
      }
    } catch (error) {
      if (!(error instanceof LookupError)) throw error
      return {
        title: `lookup ${params.kind} unavailable`,
        output: error.message,
        metadata: { kind: params.kind, error: error.message },
      }
    }
  },
})

