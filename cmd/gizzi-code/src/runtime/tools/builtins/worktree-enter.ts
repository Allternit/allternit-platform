/**
 * worktree_enter (prompt-gap contract stub; built in G5 (PR1)). Creates a git worktree and moves the session into it.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { WORKTREE_ENTER_TOOL_ID, WorktreeEnterInput, type WorktreeEnterMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/worktree-enter.txt"

export const available: boolean = false

export const WorktreeEnterTool = Tool.define<typeof WorktreeEnterInput, WorktreeEnterMetadata>(WORKTREE_ENTER_TOOL_ID, {
  description: DESCRIPTION,
  parameters: WorktreeEnterInput,
  async execute(_params) {
    return {
      title: "worktree_enter is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false },
    }
  },
})
