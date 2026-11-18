/**
 * Context notice (Primitives Wave B, lane L4): a temporary tail part on the
 * latest user message telling the model how much context is left. Fires once
 * at 25% left, once at 10% left, and after each compaction.
 *
 * SessionPrompt pushes the returned text onto the step's variable tail (after
 * history), so it never busts the cached system prefix and is not stored.
 *
 * "Left" is measured against the room before auto-compaction (window minus
 * SessionCompaction.bufferTokens), the same budget compaction itself uses:
 * with compaction on, a window-based 10% would never be reached because the
 * session compacts first. With compaction off the budget is the whole window.
 *
 * Fired thresholds are kept per session in Storage (["context_notice", id]),
 * so a restart never repeats a notice. Each compaction fires once, keyed by
 * its summary message, and only on the first request after it.
 */
import type { Provider } from "@/runtime/providers/provider"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { SessionCompaction } from "@/runtime/session/compaction"
import { Storage } from "@/runtime/session/storage/storage"
import { ContextRemaining } from "@/runtime/primitives/context-remaining"
import { CONTEXT_REMAINING_TOOL_ID, HISTORY_RECALL_TOOL_ID, type ContextNoticeTrigger } from "@/runtime/primitives/wave-b-types"

export namespace ContextNotice {
  type Fired = { thresholds: ContextNoticeTrigger[]; compactions: string[] }

  const key = (sessionID: string) => ["context_notice", sessionID]
  const cache = new Map<string, Fired>()
  /** Compaction summary ids kept per session; old ones can never fire again anyway. */
  const MAX_COMPACTION_IDS = 50

  /** Thresholds in the order they are reached, with the fraction of room left that trips each. */
  const THRESHOLDS: { trigger: Exclude<ContextNoticeTrigger, "compacted">; left: number }[] = [
    { trigger: "remaining_25", left: 0.25 },
    { trigger: "remaining_10", left: 0.1 },
  ]

  async function load(sessionID: string): Promise<Fired> {
    const live = cache.get(sessionID)
    if (live) return live
    const saved = await Storage.read<Fired>(key(sessionID)).catch(() => undefined)
    const fired: Fired = {
      thresholds: Array.isArray(saved?.thresholds) ? saved.thresholds : [],
      compactions: Array.isArray(saved?.compactions) ? saved.compactions : [],
    }
    cache.set(sessionID, fired)
    return fired
  }

  async function save(sessionID: string, fired: Fired) {
    cache.set(sessionID, fired)
    await Storage.write(key(sessionID), fired).catch(() => {
      // Best effort: the in-memory copy still keeps this process from repeating.
    })
  }

  /** Drop the in-memory copy (what a restart does); the stored record stays. */
  export function forget(sessionID: string) {
    cache.delete(sessionID)
  }

  function occupied(tokens: MessageV2.Assistant["tokens"]) {
    return tokens.total || tokens.input + tokens.output + tokens.cache.read + tokens.cache.write
  }

  /**
   * From the step's messages (already cut at the latest compaction): the
   * newest finished compaction summary, whether a real reply came after it,
   * and the tokens the newest real reply used.
   */
  function scan(messages: MessageV2.WithParts[]) {
    let summaryID: string | undefined
    let used: number | undefined
    let replyAfterSummary = false
    for (let i = messages.length - 1; i >= 0; i--) {
      const info = messages[i].info
      if (info.role !== "assistant") continue
      if (info.summary) {
        if (info.finish && !summaryID) summaryID = info.id
        if (summaryID) break
        continue
      }
      const count = occupied(info.tokens)
      if (count <= 0) continue
      if (used === undefined) used = count
      replyAfterSummary = true
    }
    return { summaryID, used, replyAfterSummary }
  }

  function percent(n: number) {
    return `${Math.max(0, Math.round(n * 100))}%`
  }

  function text(input: { triggers: ContextNoticeTrigger[]; left?: number; untilCompaction?: number }) {
    const lines: string[] = ["<system-reminder>"]
    if (input.triggers.includes("compacted")) {
      lines.push(
        "Your context was just compacted: older messages were replaced by the summary above.",
        `If you need an exact detail from before (an error message, a file path, a decision), use ${HISTORY_RECALL_TOOL_ID} to read it instead of guessing.`,
      )
    }
    const threshold = input.triggers.find((t) => t !== "compacted")
    if (threshold && input.left !== undefined) {
      const tokens = input.untilCompaction !== undefined ? ` (about ${input.untilCompaction.toLocaleString("en-US")} tokens)` : ""
      lines.push(
        `About ${percent(input.left)} of your context is left before older messages are compacted${tokens}.`,
        threshold === "remaining_10"
          ? "Finish the current step, write down anything you will need later (decisions, paths, what is left to do), and avoid reading large files or long outputs."
          : "Plan the rest of the work to fit: read only what you need, prefer targeted searches, and hand big pieces to an agent.",
        `${CONTEXT_REMAINING_TOOL_ID} gives the exact numbers.`,
      )
    }
    lines.push("</system-reminder>")
    return lines.join("\n")
  }

  /** The notice text for this step, or undefined. Never throws. */
  export async function forTurn(input: {
    sessionID: string
    model: Provider.Model
    messages: MessageV2.WithParts[]
  }): Promise<string | undefined> {
    try {
      const window = input.model.limit?.context ?? 0
      const { summaryID, used, replyAfterSummary } = scan(input.messages)
      const fired = await load(input.sessionID)
      const triggers: ContextNoticeTrigger[] = []
      const next: Fired = { thresholds: [...fired.thresholds], compactions: [...fired.compactions] }

      // Right after a compaction: the first request that follows it.
      if (summaryID && !replyAfterSummary && !fired.compactions.includes(summaryID)) {
        triggers.push("compacted")
        next.compactions = [...next.compactions, summaryID].slice(-MAX_COMPACTION_IDS)
      }

      let left: number | undefined
      let untilCompaction: number | undefined
      if (window > 0 && used !== undefined && replyAfterSummary) {
        const buffer = await SessionCompaction.bufferTokens(input.model)
        const result = ContextRemaining.compute({ window, used, buffer, compactions: 0 })
        const budget = Math.max(1, window - (buffer > 0 ? buffer : 0))
        left = result.until_compaction / budget
        untilCompaction = result.until_compaction
        // The deepest threshold crossed speaks; the ones above it count as fired too.
        const crossed = THRESHOLDS.filter((t) => left! <= t.left)
        const deepest = crossed[crossed.length - 1]
        if (deepest && !fired.thresholds.includes(deepest.trigger)) {
          triggers.push(deepest.trigger)
          for (const t of crossed) if (!next.thresholds.includes(t.trigger)) next.thresholds.push(t.trigger)
        }
      }

      if (triggers.length === 0) return undefined
      await save(input.sessionID, next)
      return text({ triggers, left, untilCompaction })
    } catch {
      return undefined
    }
  }
}
