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
import { BackgroundTask } from "@/runtime/session/background-task"
import { Todo } from "@/runtime/session/todo"
import { ContextRemaining } from "@/runtime/primitives/context-remaining"
import { CONTEXT_REMAINING_TOOL_ID, HISTORY_RECALL_TOOL_ID, type ContextNoticeTrigger } from "@/runtime/primitives/wave-b-types"

export namespace ContextNotice {
  type Fired = {
    thresholds: ContextNoticeTrigger[]
    compactions: string[]
    /** Background task id -> its last-output time when the quiet check-in fired (once per quiet period). */
    quiet: Record<string, number>
    /** Id of the newest assistant message when the todo nudge last fired (starts the next 10-turn window). */
    todoAfter?: string
  }

  /** A background command with no output this long gets one check-in. */
  export const QUIET_MS = 10 * 60 * 1000
  /** Assistant turns with open todos and no todowrite before the nudge. */
  export const TODO_NUDGE_TURNS = 10

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
      quiet: saved?.quiet && typeof saved.quiet === "object" ? { ...saved.quiet } : {},
      todoAfter: typeof saved?.todoAfter === "string" ? saved.todoAfter : undefined,
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

  /** One line per background shell command from this session that is running and has been silent for QUIET_MS. */
  async function quietLines(sessionID: string, fired: Fired, next: Fired): Promise<string[]> {
    const tasks = await BackgroundTask.list(sessionID, true)
    const now = Date.now()
    const live = new Set(tasks.map((t) => t.id))
    for (const id of Object.keys(next.quiet)) if (!live.has(id)) delete next.quiet[id]
    const lines: string[] = []
    for (const task of tasks) {
      if (task.kind !== "shell" || task.status !== "running") continue
      // time.updated moves only when the command writes output (BackgroundTask.progress).
      const last = task.time.updated
      if (now - last < QUIET_MS) continue
      if (fired.quiet[task.id] === last) continue
      next.quiet[task.id] = last
      const minutes = Math.floor((now - last) / 60_000)
      lines.push(
        `Background command "${task.description}" (${task.id}) has printed nothing for ${minutes} minutes and is still running. Check it with task_output, or stop it if it is stuck.`,
      )
    }
    return lines
  }

  /** The todo nudge line, or undefined. Turns are assistant messages since the last todowrite or nudge. */
  function todoLine(sessionID: string, messages: MessageV2.WithParts[], fired: Fired, next: Fired): string | undefined {
    const open = Todo.get(sessionID).some((t) => t.status === "pending" || t.status === "in_progress")
    if (!open) return undefined
    let start = -1
    if (fired.todoAfter) start = messages.findIndex((m) => m.info.id === fired.todoAfter)
    for (let i = messages.length - 1; i > start; i--) {
      const m = messages[i]
      if (m.info.role === "assistant" && m.parts.some((p) => p.type === "tool" && p.tool === "todowrite")) {
        start = i
        break
      }
    }
    let turns = 0
    let newest: string | undefined
    for (let i = start + 1; i < messages.length; i++) {
      if (messages[i].info.role !== "assistant") continue
      turns++
      newest = messages[i].info.id
    }
    if (turns < TODO_NUDGE_TURNS || !newest) return undefined
    next.todoAfter = newest
    return `Your todo list has open items and has not been updated in ${turns} turns. Update it, or mark done what is finished.`
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
      const next: Fired = {
        thresholds: [...fired.thresholds],
        compactions: [...fired.compactions],
        quiet: { ...fired.quiet },
        todoAfter: fired.todoAfter,
      }

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

      const extra: string[] = []
      try {
        extra.push(...(await quietLines(input.sessionID, fired, next)))
      } catch {
        // Background lookup is best effort; never block the other notices.
      }
      try {
        const nudge = todoLine(input.sessionID, input.messages, fired, next)
        if (nudge) extra.push(nudge)
      } catch {
        // Same: best effort.
      }

      if (triggers.length === 0 && extra.length === 0) return undefined
      await save(input.sessionID, next)
      const parts: string[] = []
      if (triggers.length > 0) parts.push(text({ triggers, left, untilCompaction }))
      if (extra.length > 0) parts.push(["<system-reminder>", ...extra, "</system-reminder>"].join("\n"))
      return parts.join("\n")
    } catch {
      return undefined
    }
  }
}
