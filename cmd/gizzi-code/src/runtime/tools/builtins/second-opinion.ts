/**
 * second_opinion (prompt-gap contract stub; built in G5 (PR6)). Advisory review of a plan or diff on a deeper tier.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { SECOND_OPINION_TOOL_ID, SecondOpinionInput, type SecondOpinionMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/second-opinion.txt"

export const available: boolean = false

export const SecondOpinionTool = Tool.define<typeof SecondOpinionInput, SecondOpinionMetadata>(SECOND_OPINION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: SecondOpinionInput,
  async execute(params) {
    return {
      title: "second_opinion is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false, kind: params.kind },
    }
  },
})
