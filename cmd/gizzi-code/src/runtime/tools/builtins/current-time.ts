/**
 * current_time: the time where the user is (runtime/primitives/clock.ts).
 * Read-only; every client gets it (not bot-gated). The Ink adapter
 * (cli/ui/ink-app/tools/CurrentTimeTool) wraps the same core and shares
 * current-time.txt.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Clock } from "@/runtime/primitives/clock"
import DESCRIPTION from "@/runtime/tools/builtins/current-time.txt"

export type CurrentTimeResult = Clock.Current

export const PROCESS_ZONE_NOTE = Clock.PROCESS_ZONE_NOTE

/** The time for a call (Clock.current): the argument, else the turn's zone (`turnZone`), else the last client zone, else the process zone. */
export const currentTime = Clock.current

/** The tool's text output for a result (Clock.currentOutput). */
export const currentTimeOutput = Clock.currentOutput

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
