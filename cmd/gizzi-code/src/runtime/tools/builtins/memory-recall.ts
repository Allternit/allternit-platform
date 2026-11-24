import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { MemoryDrive } from "@/runtime/memory/drive/drive"
import { MemoryKernelAdapter } from "@/runtime/memory/kernel-adapter"
import { Question } from "@/runtime/integrations/question"
import { askDisputes, needsUser, relatedLines, resolveDispute, type DisputeNote } from "@/runtime/memory/sleep/dispute-note"
import { Flag } from "@/runtime/context/flag/flag"
import { Session } from "@/runtime/session"
import * as BotTurn from "@/runtime/bots/bot-turn"
import DESCRIPTION from "@/runtime/tools/builtins/memory-recall.txt"

export const MemoryRecallTool = Tool.define("memory_recall", {
  description: DESCRIPTION,
  parameters: z.object({
    query: z.string().describe("Words to look for. Leave empty to list everything (bounded)."),
  }),
  async execute(params, ctx) {
    const query = params.query.trim()
    // Drive hits carry the entry version (needed for memory_write if_version).
    const hits = await MemoryDrive.search(query, 60)
    // Server recall over the indexed drive adds paraphrase matches.
    const seen = new Set(hits.map((h) => h.entry?.id).filter(Boolean))
    const extra = query ? (await MemoryKernelAdapter.search(query)).filter((k) => !seen.has(k.external_id ?? "")) : []
    const count = hits.length + extra.length
    if (count === 0) {
      return {
        title: "No memories found",
        output: query ? `No memories matching "${query}".` : "No memories saved yet.",
        metadata: { count, ids: [] as string[], versions: {} as Record<string, string> },
      }
    }
    MemoryKernelAdapter.report(
      "recalled",
      `Used ${count} memor${count === 1 ? "y" : "ies"} from Memory Drive`,
      [...hits.map((h) => ({ text: h.text, source: h.entry?.source })), ...extra.map((k) => ({ text: k.text.split("\n")[0], source: k.source ?? "" }))],
      ctx.sessionID,
    )
    const versions: Record<string, string> = {}
    const lines = hits.map((h) => {
      const e = h.entry
      if (e && h.version) versions[e.id] = h.version
      const meta = e
        ? ` (id: ${e.id}; version: ${h.version}; source: ${e.source}; added: ${e.added}${e.metadata.memory_type ? `; type: ${e.metadata.memory_type}` : ""}${e.metadata.conflict_of ? `; needs a look: second version of ${e.metadata.conflict_of}` : ""})`
        : ""
      return `- [${h.ref}:${h.path}] ${h.text}${meta}`
    })
    // Sleep memory conflicts: show both sides + the pick; ask the person once when it needs them.
    let disputes: DisputeNote[] = []
    if (extra.length > 0) {
      const related = relatedLines(extra)
      disputes = related.disputes
      lines.push("", "Also related (server recall; find the entry with memory_list before changing it):", ...related.lines)
    }
    if (disputes.length > 0) {
      if (await canAskUser(ctx.sessionID)) {
        // The question tool's own mechanism: Question.ask raises question.asked and waits for question.replied.
        const notes = await askDisputes(ctx.sessionID, disputes, { ask: (input) => Question.ask(input), resolve: resolveDispute })
        lines.push(...notes.map((n) => `- ${n}`))
      } else {
        // Headless clients, bot turns and subagents have nobody to answer: never block on a question there.
        for (const d of disputes) {
          if (needsUser(d))
            lines.push(`- Memory conflict ${d.id} needs the user, who cannot be asked here; do not rely on either side.`)
        }
      }
    }
    return {
      title: `Recalled ${count} memor${count === 1 ? "y" : "ies"}`,
      output: lines.join("\n"),
      metadata: { count, ids: hits.map((h) => h.entry?.id).filter((x): x is string => !!x), versions },
    }
  },
})

/** Ask only where the question tool exists (same rule as registry.ts) and a person is there:
 * not in a bot's turn and not in a subagent session (no parent). */
async function canAskUser(sessionID: string): Promise<boolean> {
  const client = ["app", "cli", "desktop"].includes(Flag.GIZZI_CLIENT) || Flag.GIZZI_ENABLE_QUESTION_TOOL
  if (!client || BotTurn.get(sessionID)) return false
  const session = await Session.get(sessionID).catch(() => undefined)
  return !!session && !session.parentID
}
