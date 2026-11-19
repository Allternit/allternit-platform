/**
 * voice_control (G7, PR9). Changes the voice session: spoken language, speed,
 * reply mode, or ending the call. Shapes in runtime/primitives/gap-types.ts.
 *
 * The tool changes nothing itself. It returns the requested change in its
 * metadata; allternit-api turns a finished call into a `control` event
 * (voice_turn_stream.rs) or `call.control` on a phone call (voice_calls.rs),
 * and the client applies it. `end_call` hangs up after the current speech.
 *
 * Voice turns only: the latest user message must carry `reply_mode: "voice"`
 * (allternit-api stamps it on voice turns). On any other turn the call is
 * refused here, so no shared registry needs a session gate.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Session } from "@/runtime/session"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { VOICE_CONTROL_TOOL_ID, VoiceControlInput, type VoiceControlMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/voice-control.txt"

export const available: boolean = true

function lastUserIsVoice(messages: MessageV2.WithParts[]): boolean {
  for (let i = messages.length - 1; i >= 0; i--) {
    const info = messages[i].info
    if (info.role === "user") return info.metadata?.["reply_mode"] === "voice"
  }
  return false
}

/** True when this turn's reply is spoken. A bridged call has no messages on ctx, so read the session. */
export async function isVoiceTurn(ctx: Pick<Tool.Context, "sessionID" | "messages">): Promise<boolean> {
  if (ctx.messages?.length) return lastUserIsVoice(ctx.messages)
  const recent = await Session.messages({ sessionID: ctx.sessionID, limit: 10 }).catch(() => [])
  return lastUserIsVoice(recent)
}

function refuse(action: VoiceControlMetadata["action"], title: string, output: string) {
  return { title, output, metadata: { ok: false, action } satisfies VoiceControlMetadata }
}

export const VoiceControlTool = Tool.define<typeof VoiceControlInput, VoiceControlMetadata>(VOICE_CONTROL_TOOL_ID, {
  description: DESCRIPTION,
  parameters: VoiceControlInput,
  async execute(params, ctx) {
    const { action } = params
    if (!(await isVoiceTurn(ctx))) {
      return refuse(action, "Not a voice session", "This turn is not a voice session, so there is nothing to change. Answer in text.")
    }
    switch (action) {
      case "set_language": {
        const language = params.language?.trim()
        if (!language) return refuse(action, "Language missing", "Give `language` as a BCP 47 tag such as \"es\" or \"pt-BR\".")
        return {
          title: `Voice language: ${language}`,
          output: `The app now speaks and listens in ${language}. Reply in that language from here on.`,
          metadata: { ok: true, action, language } satisfies VoiceControlMetadata,
        }
      }
      case "set_speed": {
        if (params.speed === undefined) return refuse(action, "Speed missing", "Give `speed` from 0.5 to 2 (1 is normal).")
        return {
          title: `Voice speed: ${params.speed}x`,
          output: `The app now speaks at ${params.speed}x.`,
          metadata: { ok: true, action, speed: params.speed } satisfies VoiceControlMetadata,
        }
      }
      case "set_reply_mode": {
        if (!params.reply_mode) return refuse(action, "Reply mode missing", "Give `reply_mode`: voice, text or both.")
        return {
          title: `Replies: ${params.reply_mode}`,
          output: `Replies now come back as ${params.reply_mode}.`,
          metadata: { ok: true, action, reply_mode: params.reply_mode } satisfies VoiceControlMetadata,
        }
      }
      case "end_call":
        return {
          title: "Ending the call",
          output: "The call ends after your current reply is spoken. Keep the goodbye to one short sentence.",
          metadata: { ok: true, action } satisfies VoiceControlMetadata,
        }
    }
  },
})
