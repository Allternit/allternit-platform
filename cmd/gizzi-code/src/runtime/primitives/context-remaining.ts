/**
 * Context left (Primitives Wave B, lane L4): the `get_context_remaining`
 * core and the source of `/context`'s `compactions`.
 *
 * Contract stub (B1): throws until L4 fills it in.
 */
import type { Provider } from "@/runtime/providers/provider"
import { notImplemented, type ContextRemainingResult } from "@/runtime/primitives/wave-b-types"

export namespace ContextRemaining {
  /** Tokens for this session against `model`'s window; `model` defaults to the session's last model. */
  export function read(_input: { sessionID: string; model?: Provider.Model }): Promise<ContextRemainingResult> {
    return notImplemented("get_context_remaining")
  }
}
