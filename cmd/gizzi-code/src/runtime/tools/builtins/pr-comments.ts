/**
 * pr_comments (prompt-gap contract stub; built in G5 (PR6)). Lists and replies to pull request review comments.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { PR_COMMENTS_TOOL_ID, PrCommentsInput, type PrCommentsMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/pr-comments.txt"

export const available: boolean = false

export const PrCommentsTool = Tool.define<typeof PrCommentsInput, PrCommentsMetadata>(PR_COMMENTS_TOOL_ID, {
  description: DESCRIPTION,
  parameters: PrCommentsInput,
  async execute(params) {
    return {
      title: "pr_comments is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false, action: params.action },
    }
  },
})
