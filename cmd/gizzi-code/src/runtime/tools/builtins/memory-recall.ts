import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { MemoryDrive } from "@/runtime/memory/drive/drive"
import { MemoryKernelAdapter, type PastTurn } from "@/runtime/memory/kernel-adapter"
import { Question } from "@/runtime/integrations/question"
import { askDisputes, needsUser, relatedLines, resolveDispute, type DisputeNote } from "@/runtime/memory/sleep/dispute-note"
import { Flag } from "@/runtime/context/flag/flag"
import { Session } from "@/runtime/session"
import * as BotTurn from "@/runtime/bots/bot-turn"
import DESCRIPTION from "@/runtime/tools/builtins/memory-recall.txt"

function pastLine(t: PastTurn): string {
  const text = t.text.replace(/\s+/g, " ").trim()
  return `- [${t.timestamp.slice(0, 10)} · ${t.speaker}; id: ${t.observation_id}; session: ${t.session_id}] ${text}`
}

export const MemoryRecallTool = Tool.define("memory_recall", {
  description: DESCRIPTION,
  parameters: z.object({
    query: z.string().describe("Words to look for. Leave empty to list everything (bounded)."),
    from: z.string().optional().describe("Only past chats on or after this date (ISO, e.g. 2026-11-01)."),
    to: z.string().optional().describe("Only past chats on or before this date (ISO)."),
    session: z.string().optional().describe("Only past chats from this session id."),
    skip_sessions: z.array(z.string()).optional().describe("Session ids to leave out (ones you already read)."),
    neighbors_of: z
      .string()
      .optional()
      .describe("A past-chat observation id (obs_…): return the turns around it instead of searching."),
  }),
  async execute(params, ctx) {
    const query = params.query.trim()
    if (params.neighbors_of?.trim()) {
      const turns = await MemoryKernelAdapter.neighbors(params.neighbors_of.trim())
      return {
        title: turns.length ? `${turns.length} turns around ${params.neighbors_of}` : "No neighbouring turns",
        output: turns.length ? turns.map(pastLine).join("\n") : `No turns found around ${params.neighbors_of}.`,
        metadata: { count: turns.length, ids: turns.map((t) => t.observation_id), versions: {} as Record<string, string> },
      }
    }
    // Drive hits carry the entry version (needed for memory_write if_version).
    const hits = await MemoryDrive.search(query, 60)
    // Server recall over the indexed drive adds paraphrase matches.
    const seen = new Set(hits.map((h) => h.entry?.id).filter(Boolean))
    const extra = query ? (await MemoryKernelAdapter.search(query)).filter((k) => !seen.has(k.external_id ?? "")) : []
    const past = query
      ? await MemoryKernelAdapter.searchPastChats(query, {
          from: params.from,
          to: params.to,
          session: params.session,
          skip_sessions: params.skip_sessions,
        })
      : []
    const count = hits.length + extra.length + past.length
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
    if (past.length > 0) {
      lines.push("", "Past chats (use neighbors_of with an id to read around a hit):", ...past.map(pastLine))
    }
    return {
      title: `Recalled ${count} memor${count === 1 ? "y" : "ies"}`,
      output: lines.join("\n"),
      metadata: { count, ids: [...hits.map((h) => h.entry?.id).filter((x): x is string => !!x), ...past.map((t) => t.observation_id)], versions },
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
