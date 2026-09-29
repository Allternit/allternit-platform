import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"

/**
 * Live progress of a long provider-side turn (a Subscription Fabric task can
 * run 5–25 minutes: deep research, image and document generation). It is a
 * status line, not content: it is not stored on the message and not replayed
 * into history. Chat bridges relay it as a `progress` frame; the chat shows it
 * as the live line of the turn's process block.
 *
 * Why not reasoning: a progress label ("Searching the web") is the provider
 * UI's status, not the model's thought — as reasoning it would pile up in the
 * transcript and be sent back to other models as the assistant's thinking.
 */
export namespace SessionProgress {
  export const Event = {
    Updated: BusEvent.define(
      "session.progress",
      z.object({
        sessionID: z.string(),
        messageID: z.string().optional(),
        /** What the provider is doing now, in plain words. */
        label: z.string().optional(),
        /** 0..1 when the provider reports it. */
        fraction: z.number().optional(),
        /** Seconds since the task started (heartbeat: still working). */
        elapsedS: z.number().optional(),
      }),
    ),
  }
}
