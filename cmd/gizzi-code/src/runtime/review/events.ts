import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"

/** Review progress on the Bus, so the app can show "Review · round 2". */
export namespace ReviewEvent {
  export const Started = BusEvent.define(
    "review.started",
    z.object({
      sessionID: z.string().optional(),
      round: z.number().int().positive(),
      maxRounds: z.number().int().positive(),
      files: z.number().int().nonnegative(),
    }),
  )
  export const Finished = BusEvent.define(
    "review.finished",
    z.object({
      sessionID: z.string().optional(),
      round: z.number().int().positive(),
      maxRounds: z.number().int().positive(),
      verdict: z.enum(["PASS", "FAIL", "PARTIAL"]).nullable(),
      reviewID: z.string().optional(),
      findings: z.number().int().nonnegative(),
    }),
  )
  /** The fix turn sent back to the coder between rounds. */
  export const Fixing = BusEvent.define(
    "review.fixing",
    z.object({ sessionID: z.string(), round: z.number().int().positive(), maxRounds: z.number().int().positive() }),
  )
  export const LoopFinished = BusEvent.define(
    "review.loop.finished",
    z.object({
      sessionID: z.string(),
      rounds: z.number().int().nonnegative(),
      stopped: z.enum(["pass", "limit", "error", "abort", "nothing"]),
      verdict: z.enum(["PASS", "FAIL", "PARTIAL"]).nullable(),
      error: z.string().optional(),
    }),
  )
}
