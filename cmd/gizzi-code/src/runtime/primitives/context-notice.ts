/**
 * Context notice (Primitives Wave B, lane L4): a temporary tail part on the
 * latest user message telling the model how much context is left. Fires once
 * at 25% left, once at 10% left, and after each compaction.
 *
 * SessionPrompt pushes the returned text onto the step's variable tail (after
 * history), so it never busts the cached system prefix and is not stored.
 *
 * Contract stub (B1): returns undefined (no notice) until L4 lands.
 */
import type { Provider } from "@/runtime/providers/provider"
import type { MessageV2 } from "@/runtime/session/message-v2"

export namespace ContextNotice {
  /** The notice text for this step, or undefined. Never throws. */
  export async function forTurn(_input: {
    sessionID: string
    model: Provider.Model
    messages: MessageV2.WithParts[]
  }): Promise<string | undefined> {
    return undefined
  }
}
