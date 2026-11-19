/**
 * voice_control (prompt-gap contract stub; built in G7 (PR9)). Voice session language, speed, reply mode and end call.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { VOICE_CONTROL_TOOL_ID, VoiceControlInput, type VoiceControlMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/voice-control.txt"

export const available: boolean = false

export const VoiceControlTool = Tool.define<typeof VoiceControlInput, VoiceControlMetadata>(VOICE_CONTROL_TOOL_ID, {
  description: DESCRIPTION,
  parameters: VoiceControlInput,
  async execute(params) {
    return {
      title: "voice_control is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false, action: params.action },
    }
  },
})
