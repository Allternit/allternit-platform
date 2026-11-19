/**
 * end_conversation (prompt-gap contract stub; built in G7 (PR11g)). Ends one of Gizzi's own chats after a warning, for sustained abuse.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { END_CONVERSATION_TOOL_ID, EndConversationInput, type EndConversationMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/end-conversation.txt"

export const available: boolean = false

export const EndConversationTool = Tool.define<typeof EndConversationInput, EndConversationMetadata>(END_CONVERSATION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: EndConversationInput,
  async execute(_params) {
    return {
      title: "end_conversation is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false },
    }
  },
})
