/**
 * worktree_exit (prompt-gap contract stub; built in G5 (PR1)). Leaves the session's worktree; keeps or removes it.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { WORKTREE_EXIT_TOOL_ID, WorktreeExitInput, type WorktreeExitMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/worktree-exit.txt"

export const available: boolean = false

export const WorktreeExitTool = Tool.define<typeof WorktreeExitInput, WorktreeExitMetadata>(WORKTREE_EXIT_TOOL_ID, {
  description: DESCRIPTION,
  parameters: WorktreeExitInput,
  async execute(params) {
    return {
      title: "worktree_exit is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false, action: params.action },
    }
  },
})
