/**
 * Steer signal (Primitives Wave B, lane L1): wakes a `wait_agents` call when
 * the user sends a message to the waiting session.
 *
 * SessionPrompt calls `notify` where a message queues behind the running turn
 * (the queuedTurns bump in runLoop). `wait_agents` races `wait` against its
 * children settling and its timeout.
 *
 * Contract stub (B1): notify is a no-op so the prompt path is unchanged until
 * L1 lands; wait throws.
 */
import { notImplemented } from "@/runtime/primitives/wave-b-types"

export namespace SteerSignal {
  /** A message was queued for `sessionID` while its turn runs. Never throws. */
  export function notify(_sessionID: string): void {}

  /** Resolves on the next `notify(sessionID)` after the call; rejects when `abort` fires. */
  export function wait(_sessionID: string, _abort: AbortSignal): Promise<void> {
    return notImplemented("SteerSignal.wait")
  }
}
