/**
 * sleep: a short in-turn wait (runtime/primitives/sleep.ts). Wakes early when
 * a new user message arrives for this session, or when the turn is aborted.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Sleep } from "@/runtime/primitives/sleep"
import { Bus } from "@/shared/bus"
import { MessageV2 } from "@/runtime/session/message-v2"
import DESCRIPTION from "@/runtime/tools/builtins/sleep.txt"

export const SleepTool = Tool.define("sleep", {
  description: DESCRIPTION,
  parameters: z.object({
    seconds: z.number().min(Sleep.MIN_SECONDS).max(Sleep.MAX_SECONDS).describe("How long to wait, 1 to 900 seconds"),
    reason: z.string().max(Sleep.MAX_REASON_CHARS).describe("Why you are waiting, shown to the user"),
  }),
  async execute(params, ctx) {
    await ctx.ask({
      permission: "sleep",
      patterns: ["*"],
      always: ["*"],
      metadata: { seconds: params.seconds, reason: params.reason },
    })
    const known = new Set(ctx.messages.filter((m) => m.info.role === "user").map((m) => m.info.id))
    const result = await Sleep.wait({
      seconds: params.seconds,
      abort: ctx.abort,
      watchUserInput: (wake) =>
        Bus.subscribe(MessageV2.Event.Updated, (event) => {
          const info = event.properties.info
          if (info.sessionID === ctx.sessionID && info.role === "user" && !known.has(info.id)) wake()
        }),
    })
    return {
      title: `slept ${result.slept_seconds}s (${result.woke})`,
      output: `Slept ${result.slept_seconds} seconds. Woke: ${result.woke}.`,
      metadata: { ...result, reason: params.reason },
    }
  },
})
