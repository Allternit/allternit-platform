/**
 * Agent control core (Primitives Wave B, lane L1): `wait_agents` and
 * `agent_control` act on the calling session's own descendants only.
 *
 * - waitAgents: returns when the first target settles, the user steers
 *   (SteerSignal), or the timeout passes.
 * - run: list, interrupt (keeps the child; not reported as failed), resume and
 *   followup (open a new background_task run row on the same child), close
 *   (cascades, deepest first).
 *
 * Contract stub (B1): both throw until L1 fills them in.
 */
import {
  notImplemented,
  type AgentControlInput,
  type AgentControlResult,
  type WaitAgentsInput,
  type WaitAgentsResult,
} from "@/runtime/primitives/wave-b-types"

export namespace AgentControl {
  export interface Caller {
    /** The calling session (the parent). */
    sessionID: string
    abort: AbortSignal
  }

  export function waitAgents(_input: Caller & WaitAgentsInput): Promise<WaitAgentsResult> {
    return notImplemented("wait_agents")
  }

  /** `close` has already been approved by the builtin's permission ask when this runs. */
  export function run(_input: Caller & AgentControlInput): Promise<AgentControlResult> {
    return notImplemented("agent_control")
  }
}
