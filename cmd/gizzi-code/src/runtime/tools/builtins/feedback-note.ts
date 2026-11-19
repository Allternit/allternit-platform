/**
 * feedback_note (prompt-gap contract stub; built in G4 (PR11c)). A diagnostic note kept on the device until the person sends it.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { FEEDBACK_NOTE_TOOL_ID, FeedbackNoteInput, type FeedbackNoteMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/feedback-note.txt"

export const available: boolean = false

export const FeedbackNoteTool = Tool.define<typeof FeedbackNoteInput, FeedbackNoteMetadata>(FEEDBACK_NOTE_TOOL_ID, {
  description: DESCRIPTION,
  parameters: FeedbackNoteInput,
  async execute(_params) {
    return {
      title: "feedback_note is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false },
    }
  },
})
