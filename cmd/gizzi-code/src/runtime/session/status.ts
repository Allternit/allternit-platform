import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Instance } from "@/runtime/context/project/instance"
import z from "zod/v4"
import { SleepTriggers } from "@/runtime/memory/sleep/triggers"

export namespace SessionStatus {
  export const Info = z
    .union([
      z.object({
        type: z.literal("idle"),
      }),
      z.object({
        type: z.literal("retry"),
        attempt: z.number(),
        message: z.string(),
        next: z.number(),
      }),
      z.object({
        type: z.literal("busy"),
        /** Set by the status_update tool: short phase label and one-line note. */
        phase: z.string().optional(),
        note: z.string().optional(),
      }),
    ])
    
  export type Info = z.infer<typeof Info>

  export const Event = {
    Status: BusEvent.define(
      "session.status",
      z.object({
        sessionID: z.string(),
        status: Info,
      }),
    ),
    // deprecated
    Idle: BusEvent.define(
      "session.idle",
      z.object({
        sessionID: z.string(),
      }),
    ),
  }

  const state = Instance.state(() => {
    const data: Record<string, Info> = {}
    return data
  })

  export function get(sessionID: string) {
    return (
      state()[sessionID] ?? {
        type: "idle",
      }
    )
  }

  export function list() {
    return state()
  }

  export function all() {
    return state()
  }

  export function set(sessionID: string, status: Info) {
    // Plain busy re-sets keep the phase the agent last reported.
    if (status.type === "busy" && status.phase === undefined) {
      const prev = state()[sessionID]
      if (prev?.type === "busy" && prev.phase) status = { ...status, phase: prev.phase, note: prev.note }
    }
    Bus.publish(Event.Status, {
      sessionID,
      status,
    })
    if (status.type === "idle") {
      // deprecated
      Bus.publish(Event.Idle, {
        sessionID,
      })
      delete state()[sessionID]
      // Allternit Sleep: a quiet session starts its idle timer (cache TTL).
      // No-op for sessions without a top-level prompt here (subagents).
      SleepTriggers.sessionIdle(sessionID)
      return
    }
    state()[sessionID] = status
  }
}
