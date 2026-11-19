/**
 * user_location (G7, PR4). The location the person's app attached to this
 * turn, approximate. Shapes in runtime/primitives/gap-types.ts.
 *
 * Source: allternit-api's opt-in `client_location` (client_location.rs). It
 * validates and rounds it (two decimals, never finer than 1 km) and hands it
 * to the runtime for this turn only as `turnLocation`; SessionPrompt holds it
 * in TurnLocations and passes it to native tools as `ctx.extra.location`. So
 * every value this tool can see is approximate.
 *
 * No location, or a precise request: the tool reads nothing and returns
 * `status: "requested"`. The app card then shows "Share location"
 * (approximate or precise); a tap sends it with the next message. That tap is
 * the person's consent each time, so precise needs no extra permission ask
 * here and is never remembered. Only a returned location goes through the
 * `user_location` permission (pattern = precision; approximate is allowed by
 * default in agent.ts), so a person's own deny rule still holds.
 *
 * Never stored: the tool writes nothing, and its output tells the model not
 * to save the location to memory.
 *
 * App clients only (registry GAP_TOOLS `appOnly`): a terminal has no
 * location service.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import {
  USER_LOCATION_TOOL_ID,
  UserLocationInput,
  type LocationPrecision,
  type UserLocationMetadata,
} from "@/runtime/primitives/gap-types"
import { TurnLocations, turnLocationOf } from "@/runtime/tools/builtins/lookup"
import DESCRIPTION from "@/runtime/tools/builtins/user-location.txt"

export const available: boolean = true

/**
 * UserLocationMetadata plus the "requested" status the location decision
 * adds (the card shows "Share location"). Collapses to UserLocationMetadata
 * once gap-types' status enum carries "requested".
 */
export type UserLocationToolMetadata = Omit<UserLocationMetadata, "status"> & {
  status?: NonNullable<UserLocationMetadata["status"]> | "requested"
}

/** The radius stated for an approximate location is never finer than this. */
const MIN_ACCURACY_M = 1000

function round2(x: number): number {
  return Math.round(x * 100) / 100 + 0
}

function requested(precision: LocationPrecision, attached: boolean) {
  const why = precision === "precise"
    ? attached
      ? "Only an approximate location is shared for this message, and the answer needs a precise one."
      : "No location is shared for this message."
    : "No location is shared for this message."
  const tap = precision === "precise" ? '"Share location" and choose precise' : '"Share location"'
  return {
    title: precision === "precise" ? "Precise location requested" : "Location requested",
    output: `${why} Ask the person to tap ${tap} on the card if they want to share it; it comes with their next message. Continue without it in the meantime: answer what you can, or ask them to name a place.`,
    metadata: { ok: false, status: "requested", precision } satisfies UserLocationToolMetadata,
  }
}

export const UserLocationTool = Tool.define<typeof UserLocationInput, UserLocationToolMetadata>(USER_LOCATION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: UserLocationInput,
  async execute(params, ctx) {
    const precision: LocationPrecision = params.precision ?? "approximate"
    // Native calls carry the turn value on ctx.extra; a bridged CLI-brain call reads it by session.
    const shared = turnLocationOf(ctx.extra?.location) ?? TurnLocations.get(ctx.sessionID)
    // Precise needs a precise share for this turn (the person's tap); otherwise ask for one.
    if (!shared || (precision === "precise" && !shared.precise)) return requested(precision, !!shared)
    if (precision === "precise" && shared.precise) {
      const accuracy = shared.accuracyM ?? 10
      const lines = [
        `Precise location shared for this message: ${shared.lat.toFixed(4)}, ${shared.lon.toFixed(4)}, within about ${accuracy} m.`,
        shared.place ? `Place: ${shared.place}.` : undefined,
        shared.timeZone ? `Time zone: ${shared.timeZone}.` : undefined,
        "Use it only for this answer. Don't repeat the coordinates unless needed, and don't save it to memory.",
      ].filter((line): line is string => !!line)
      return {
        title: shared.place ? `At ${shared.place}` : "Precise location",
        output: lines.join("\n"),
        metadata: {
          ok: true,
          status: "granted",
          precision: "precise",
          ...(shared.timeZone ? { time_zone: shared.timeZone } : {}),
          latitude: shared.lat,
          longitude: shared.lon,
          accuracy,
        } satisfies UserLocationToolMetadata,
      }
    }

    await ctx.ask({
      permission: USER_LOCATION_TOOL_ID,
      patterns: ["approximate"],
      always: ["approximate"],
      metadata: { precision: "approximate", reason: params.reason },
    })

    const latitude = round2(shared.lat)
    const longitude = round2(shared.lon)
    const accuracy = Math.max(MIN_ACCURACY_M, Math.round((shared.accuracyKm ?? 1) * 1000))
    const km = Math.round(accuracy / 1000)
    const lines = [
      `Approximate location shared for this message: about ${latitude.toFixed(2)}, ${longitude.toFixed(2)}, within roughly ${km} km.`,
      shared.place ? `Place: ${shared.place}.` : undefined,
      shared.timeZone ? `Time zone: ${shared.timeZone}.` : undefined,
      "Use it only for this answer. Don't save it to memory.",
    ].filter((line): line is string => !!line)
    return {
      title: shared.place ? `Near ${shared.place}` : "Approximate location",
      output: lines.join("\n"),
      metadata: {
        ok: true,
        status: "granted",
        precision: "approximate",
        ...(shared.timeZone ? { time_zone: shared.timeZone } : {}),
        latitude,
        longitude,
        accuracy,
      } satisfies UserLocationToolMetadata,
    }
  },
})
