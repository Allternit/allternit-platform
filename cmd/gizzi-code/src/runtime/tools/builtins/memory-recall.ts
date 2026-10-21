import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { MemoryService } from "@/runtime/memory/memory-service"
import { MemoryKernelAdapter } from "@/runtime/memory/kernel-adapter"

const DESCRIPTION = `Search the user's Memory Drive (personal plus any mounted drives).

Use this before making assumptions about user preferences, project conventions,
or past feedback, and to find a memory's id before updating or deleting it with
memory_write. Returns matching one-line memories with their drive, topic file,
id, source and date.`

export const MemoryRecallTool = Tool.define("memory_recall", {
  description: DESCRIPTION,
  parameters: z.object({
    query: z.string().describe("Words to look for. Leave empty to list everything (bounded)."),
  }),
  async execute(params, _ctx) {
    const query = params.query.trim()
    const hits = await MemoryService.searchEntries(query, 60)
    // Server recall over the indexed drive adds paraphrase matches.
    const seen = new Set(hits.map((h) => h.entry?.id).filter(Boolean))
    const extra = query ? (await MemoryKernelAdapter.search(query)).filter((k) => !seen.has(k.external_id ?? "")) : []
    const count = hits.length + extra.length
    if (count === 0) {
      return {
        title: "No memories found",
        output: query ? `No memories matching "${query}".` : "No memories saved yet.",
        metadata: { count, ids: [] as string[] },
      }
    }
    const lines = hits.map((h) => {
      const e = h.entry
      const meta = e ? ` (id: ${e.id}; source: ${e.source}; added: ${e.added}${e.metadata.memory_type ? `; type: ${e.metadata.memory_type}` : ""})` : ""
      return `- [${h.ref}:${h.path}] ${h.text}${meta}`
    })
    if (extra.length > 0) {
      lines.push("", "Also related (server recall):", ...extra.map((k) => `- ${k.text.split("\n")[0]}`))
    }
    return {
      title: `Recalled ${count} memor${count === 1 ? "y" : "ies"}`,
      output: lines.join("\n"),
      metadata: { count, ids: hits.map((h) => h.entry?.id).filter((x): x is string => !!x) },
    }
  },
})
