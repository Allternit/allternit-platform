/**
 * voice_control in the Gizzi TUI (G7, PR9). Same id, input schema and
 * description as the runtime builtin (runtime/tools/builtins/voice-control.ts),
 * enabled while that module's `available` is true and voice dictation is on
 * (`/voice`), since TUI voice is push-to-talk dictation only.
 *
 * Only end_call applies here: it stops a recording in progress. The TUI has no
 * spoken output, language or speed, so reply mode, language and speed are
 * applied in the apps only and the result says so.
 */
import { available } from '@/runtime/tools/builtins/voice-control'
import { VOICE_CONTROL_TOOL_ID, VoiceControlInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/voice-control.txt'
import { buildTool, type ToolDef } from '../../Tool.js'
import { getInitialSettings } from '../../utils/settings/settings.js'

export const VOICE_CONTROL_TOOL_NAME = VOICE_CONTROL_TOOL_ID

type InputSchema = typeof VoiceControlInput
type Output = { ok: boolean; message: string }

const APPS_ONLY: Record<string, string> = {
  set_language: 'Language is applied in the apps only: terminal voice has no spoken output.',
  set_speed: 'Speed is applied in the apps only: terminal voice has no spoken output.',
  set_reply_mode: 'Reply mode is applied in the apps only: terminal replies are always text.',
}

export const VoiceControlTool = buildTool({
  name: VOICE_CONTROL_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'Voice',
  get inputSchema(): InputSchema {
    return VoiceControlInput
  },
  isEnabled() {
    return available && getInitialSettings().voiceEnabled === true
  },
  async description() {
    return DESCRIPTION_TXT
  },
  async prompt() {
    return DESCRIPTION_TXT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.message }
  },
  renderToolUseMessage() {
    return ''
  },
  async call(input) {
    if (input.action !== 'end_call') {
      return { data: { ok: false, message: APPS_ONLY[input.action] ?? 'Applied in the apps only.' } }
    }
    // Loaded on use: the voice service pulls in the recorder.
    const { stopRecording } = await import('../../services/voice.js')
    stopRecording()
    return { data: { ok: true, message: 'Stopped the voice recording. Reply in text from here on.' } }
  },
} satisfies ToolDef<InputSchema, Output>)
