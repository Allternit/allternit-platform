/**
 * current_time: the time where the user is (runtime/primitives/clock.ts).
 * Read-only; every client gets it (not bot-gated). The Ink adapter wraps the
 * same core and shares current-time.txt.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Clock } from "@/runtime/primitives/clock"
import DESCRIPTION from "@/runtime/tools/builtins/current-time.txt"

export type CurrentTimeResult = Clock.Now & { source: Clock.Source }

export const PROCESS_ZONE_NOTE =
  "This is this machine's time zone. It may not be the user's: say which zone you mean, or ask the user where they are."

/** The time for a call: the argument, else the turn's zone (`turnZone`), else the last client zone, else the process zone. */
export async function currentTime(input: { timezone?: string; turnZone?: string; at?: Date }): Promise<CurrentTimeResult> {
  const resolved = await Clock.resolveTimeZone({ argument: input.timezone, turnZone: input.turnZone })
  return { ...Clock.now(resolved.zone, input.at), source: resolved.source }
}

/** The tool's text output for a result. */
export function currentTimeOutput(result: CurrentTimeResult, requested?: string): string {
  const lines = [JSON.stringify(result, null, 2)]
  if (requested && result.source !== "argument") {
    lines.push(`"${requested}" is not a valid IANA time zone, so the user's zone was used instead.`)
  }
  if (result.source === "process") lines.push(PROCESS_ZONE_NOTE)
  return lines.join("\n")
}

export const CurrentTimeTool = Tool.define("current_time", {
  description: DESCRIPTION,
  parameters: z.object({
    timezone: z
      .string()
      .max(64)
      .optional()
      .describe('An IANA time zone such as "America/Chicago"; leave it out for the user\'s own zone'),
  }),
  async execute(params, ctx) {
    await ctx.ask({
      permission: "current_time",
      patterns: ["*"],
      always: ["*"],
      metadata: { timezone: params.timezone },
    })
    // This turn's zone: the latest user message's `metadata.time_zone`.
    const lastUser = ctx.messages.findLast((message) => message.info.role === "user")
    const turnZone = Clock.turnZoneOf(lastUser?.info.role === "user" ? lastUser.info.metadata : undefined)
    const result = await currentTime({ timezone: params.timezone, turnZone })
    return {
      title: `${result.weekday} ${result.iso.slice(11, 16)} ${result.time_zone}`,
      output: currentTimeOutput(result, params.timezone),
      metadata: result,
    }
  },
})
