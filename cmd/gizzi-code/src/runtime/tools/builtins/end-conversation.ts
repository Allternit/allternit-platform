/**
 * end_conversation (G7, PR11g). Ends one of Gizzi's own chats: for sustained
 * abuse after an earlier warning, or when the person asked and confirmed.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * - Gizzi's own chats only (decision D5): SessionPrompt.resolveTools drops the
 *   tool on bot turns (customer channels included) and child sessions; the
 *   checks here repeat that for a call that arrives anyway.
 * - The end is recorded as `Session.Info.ended` ({ at, reason }, the ended
 *   registry). SessionPrompt.prompt then refuses new prompts with
 *   Session.ENDED_MESSAGE and clients offer a new chat.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { END_CONVERSATION_TOOL_ID, EndConversationInput, type EndConversationMetadata } from "@/runtime/primitives/gap-types"
import { Session } from "@/runtime/session"
import * as BotTurn from "@/runtime/bots/bot-turn"
import DESCRIPTION from "@/runtime/tools/builtins/end-conversation.txt"

export const available: boolean = true

function refuse(output: string) {
  return { title: "Chat not ended", output, metadata: { ok: false } as EndConversationMetadata }
}

export const EndConversationTool = Tool.define<typeof EndConversationInput, EndConversationMetadata>(END_CONVERSATION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: EndConversationInput,
  async execute(params, ctx) {
    if (BotTurn.get(ctx.sessionID)) return refuse("end_conversation works only in Gizzi's own chats, not in a bot's chat.")
    const session = await Session.get(ctx.sessionID)
    if (session.parentID) return refuse("A subagent cannot end the parent's chat.")
    if (session.ended) {
      return {
        title: "Chat already ended",
        output: Session.ENDED_MESSAGE,
        metadata: { ok: true, reason: session.ended.reason, ended: true },
      }
    }
    // A warning or a confirmation has to sit in an earlier reply of this chat.
    const earlierReply = ctx.messages.some((m) => m.info.role === "assistant" && m.info.id !== ctx.messageID)
    if (params.reason === "sustained_abuse" && (!params.warned || !earlierReply)) {
      return refuse("Not ended. Warn the person plainly first and try to steer the chat back; end it only if the abuse continues after that.")
    }
    if (params.reason === "user_requested" && !earlierReply) {
      return refuse("Not ended. Ask the person to confirm they want to end this chat, and end it after they say yes.")
    }
    const info = await Session.setEnded({ sessionID: ctx.sessionID, reason: params.reason })
    return {
      title: "Chat ended",
      output: `The chat has ended and takes no new messages; the person can start a new chat. Close with one short, neutral line and nothing else.`,
      metadata: { ok: true, reason: info.ended?.reason ?? params.reason, ended: true },
    }
  },
})
