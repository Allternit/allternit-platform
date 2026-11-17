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
 * Not registered yet: E1-7 adds it to registry.ts and the CLI bridge offer.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Clock } from "@/runtime/primitives/clock"
import { LOOKUP_KINDS, LookupError, isLocalLookup, lookup, type LookupCard, type LookupKind } from "@/runtime/primitives/lookup"
import DESCRIPTION from "@/runtime/tools/builtins/lookup.txt"

export type LookupToolMetadata = { kind: LookupKind; card?: LookupCard; credit?: string; error?: string }

export const LOOKUP_PARAMETERS = z.object({
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

export const LookupTool = Tool.define<typeof LOOKUP_PARAMETERS, LookupToolMetadata>("lookup", {
  description: DESCRIPTION,
  parameters: LOOKUP_PARAMETERS,
  async execute(params, ctx) {
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
    const timeZone = Clock.turnZoneOf(lastUser?.info.role === "user" ? lastUser.info.metadata : undefined)
    try {
      // The shared location reaches this call once E1-7 wires the turn's client_location.
      const result = await lookup(params, { signal: ctx.abort, timeZone })
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

