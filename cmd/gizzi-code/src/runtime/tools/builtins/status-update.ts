/**
 * status_update (prompt-gap contract stub; built in G4 (PR8b)). Short progress for long work; publishes session.status.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { STATUS_UPDATE_TOOL_ID, StatusUpdateInput, type StatusUpdateMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/status-update.txt"

export const available: boolean = false

export const StatusUpdateTool = Tool.define<typeof StatusUpdateInput, StatusUpdateMetadata>(STATUS_UPDATE_TOOL_ID, {
  description: DESCRIPTION,
  parameters: StatusUpdateInput,
  async execute(params) {
    return {
      title: "status_update is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false, phase: params.phase },
    }
  },
})
