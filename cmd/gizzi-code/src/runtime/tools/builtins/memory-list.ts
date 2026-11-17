/**
 * memory_list (Primitives Wave D, D1 recall): the Memory Drive's topics with
 * counts, or one topic's entries `{id, version, text, added}`, 200 per page.
 * Every client; permission allow (loop/agent.ts).
 *
 * W1 stub: final id and parameters; lane M fills the execute body (versions
 * come from the entry-version helper in memory/drive/format.ts).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/memory-list.txt"

export const MEMORY_LIST_PAGE_SIZE = 200

export const MemoryListTool = Tool.define("memory_list", {
  description: DESCRIPTION,
  parameters: z.object({
    drive: z.string().optional().describe("A mounted drive's name; leave it out for the personal drive"),
    topic: z.string().optional().describe("A topic file; leave it out to list topics with counts"),
    after: z.string().optional().describe("The last entry id of the previous page"),
  }),
  async execute(_params, _ctx) {
    return {
      title: "Memory list unavailable",
      output: "memory_list is not available in this build yet.",
      metadata: { count: 0, ids: [] as string[] },
    }
  },
})
