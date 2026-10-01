// Shadow S1 permission GATE (Q27). After the policy floor and the configured
// rules have decided (PermissionNext.evaluatePolicy), each decision is also
// asked of the shared S1 decision runtime (`/v1/decision`) and lands in the one
// shadow ledger. The person's approve/reject reply on an ask card is reported
// as its outcome label.
//
// Shadow only: nothing here is awaited by the permission path and nothing here
// changes its action. When S1 is ever combined with a permission decision it
// goes through `tighten()`, which can never loosen the incumbent (Q26).
import {
  reportOutcome,
  shadowGate,
  tighten,
  type DecisionClientOptions,
  type Friction,
} from "../../../../../../../tools/system-one-local/src/decision/client"

export namespace PermissionS1 {
  export const BANK = "bank.permission_gate"
  export const PRIMITIVE = "permission.gizzi"
  export const QUESTION = "may_proceed"

  let override: DecisionClientOptions | undefined
  /** Tests: route calls to a fake runtime (and force-enable under `bun test`). */
  export function _setClient(opts: DecisionClientOptions | undefined) {
    override = opts
  }

  function opts(): DecisionClientOptions | undefined {
    if (override) return override
    if (process.env.ALLTERNIT_S1_SHADOW_GATES === "0") return undefined
    // Unit tests never reach a real runtime unless they opt in.
    if (process.env.NODE_ENV === "test" && process.env.ALLTERNIT_S1_SHADOW_GATES !== "1") return undefined
    return {}
  }

  export const subjectForRequest = (requestID: string) => `gizzi-permission:${requestID}`
  export const subjectForCall = (callID: string | undefined, permission: string, pattern: string) =>
    callID ? `gizzi-call:${callID}:${permission}:${pattern}` : undefined

  export interface ShadowInput {
    permission: string
    pattern: string
    mode: string | undefined
    incumbent: Friction
    sessionID: string
    subject_ref?: string
  }

  /** Fire-and-forget shadow GATE. Returns the pending promise for tests; callers never await it. */
  export function shadow(input: ShadowInput): Promise<unknown> | undefined {
    const o = opts()
    if (!o) return undefined
    const state = [`permission: ${input.permission}`, `pattern: ${input.pattern}`, `mode: ${input.mode ?? "default"}`].join("\n")
    return shadowGate(
      {
        producer: "gizzi-permission",
        decision_bank_id: BANK,
        question_id: QUESTION,
        primitive_id: PRIMITIVE,
        motif: "GATE",
        subject_ref: input.subject_ref,
        ids: { session_id: input.sessionID },
        instructions:
          "Should this tool permission proceed without asking the person first? Answer true only if it is clearly safe and routine.",
        extensions: { "x-incumbent_action": input.incumbent },
      },
      state,
      o,
    ).catch(() => null)
  }

  /** The person's reply to an ask card: approve ("once"/"always") = true, reject = false. */
  export function outcome(requestID: string, approved: boolean): Promise<boolean> | undefined {
    const o = opts()
    if (!o) return undefined
    return reportOutcome(
      { subject_ref: subjectForRequest(requestID), question_id: QUESTION, truth: approved ? "true" : "false", source: "gizzi.permission_reply" },
      o,
    ).catch(() => false)
  }

  /** The only sanctioned way to combine S1 with a permission decision: tighten-only. */
  export const combine = tighten
}
