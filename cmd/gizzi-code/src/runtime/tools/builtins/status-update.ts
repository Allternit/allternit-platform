/**
 * status_update: short progress for long work; publishes session.status with a phase and note.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { STATUS_UPDATE_TOOL_ID, StatusUpdateInput, type StatusUpdateMetadata } from "@/runtime/primitives/gap-types"
import { SessionStatus } from "@/runtime/session/status"
import DESCRIPTION from "@/runtime/tools/builtins/status-update.txt"

export const available: boolean = true

export const StatusUpdateTool = Tool.define<typeof StatusUpdateInput, StatusUpdateMetadata>(STATUS_UPDATE_TOOL_ID, {
  description: DESCRIPTION,
  parameters: StatusUpdateInput,
  async execute(params, ctx) {
    const phase = params.phase.trim()
    const note = params.note?.replace(/\s+/g, " ").trim() || undefined
    SessionStatus.set(ctx.sessionID, { type: "busy", phase, note })
    return {
      title: phase,
      output: note ? `Status set: ${phase} - ${note}` : `Status set: ${phase}`,
      metadata: { ok: true, phase, note, at: Date.now() },
    }
  },
})
