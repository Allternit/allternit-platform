/**
 * The Factory board's row model: pure functions shared by `gizzi board`
 * (plain terminal), the TUI's `/board` view and the footer badge, so all
 * three say the same thing.
 *
 * Status conventions borrowed from Herdr (factory/pane): one glyph + word
 * per state, the states roll up into the header, a finished worker keeps
 * its `✓ done` until it's landed, and the line under each bot is the last
 * thing its pane said.
 */
import type { FactoryAgent } from "@/cli/factory/types"
import { costLabel, type SwarmRecord, type SwarmWorker } from "@/cli/factory/swarm"

export type BoardState = "starting" | "working" | "waiting" | "idle" | "done" | "failed" | "stopped"

export interface BoardRow {
  key: string
  address: string
  agentId: string | null
  agent: FactoryAgent | null
  swarm: SwarmRecord | null
  worker: SwarmWorker | null
  brain: string
  state: BoardState
  /** ms the row has been in its current run (frozen once a worker finishes). */
  elapsedMs: number
  cost: string
  branch: string
  lastLine: string
  attachable: boolean
}

export interface BoardGroup {
  title: string
  rows: BoardRow[]
}

export const STATE_GLYPH: Record<BoardState, string> = {
  starting: "◌",
  working: "●",
  waiting: "▲",
  idle: "○",
  done: "✓",
  failed: "✗",
  stopped: "■",
}

export const STATE_WORD: Record<BoardState, string> = {
  starting: "starting",
  working: "working",
  waiting: "waiting",
  idle: "idle",
  done: "done",
  failed: "failed",
  stopped: "stopped",
}

export function engineState(state: string | undefined): BoardState {
  switch (state) {
    case "working":
    case "checking":
      return "working"
    case "needs_you":
    case "blocked":
      return "waiting"
    case "done":
      return "done"
    case "failed":
      return "failed"
    default:
      return "idle"
  }
}

function workerState(w: SwarmWorker): BoardState {
  switch (w.phase) {
    case "starting":
      return "starting"
    case "working":
      return "working"
    case "waiting":
      return "waiting"
    case "done":
    case "landed":
      return "done"
    case "failed":
      return "failed"
    case "stopped":
      return "stopped"
  }
}

function harnessOf(agent: FactoryAgent | null): string | undefined {
  return agent?.binding?.harness ?? agent?.binding?.vendor ?? agent?.binding?.type
}

export interface BoardInput {
  agents: FactoryAgent[]
  swarms: SwarmRecord[]
  lastLine: Record<string, string>
  since: Record<string, number>
  now?: number
}

/**
 * Swarm workers first (grouped per swarm, landed and stopped ones dropped),
 * then every other bot the engine says is live. Offline bots are left out:
 * the board is for what is running now (`gizzi agents ps` lists all).
 */
export function buildBoard(input: BoardInput): BoardGroup[] {
  const now = input.now ?? Date.now()
  const byId = new Map(input.agents.map((a) => [a.id, a]))
  const claimed = new Set<string>()
  const groups: BoardGroup[] = []

  for (const s of input.swarms) {
    const rows: BoardRow[] = []
    for (const w of s.workers) {
      if (w.phase === "landed" || w.phase === "stopped") {
        claimed.add(w.agentId)
        continue
      }
      const agent = byId.get(w.agentId) ?? input.agents.find((a) => a.address === w.address) ?? null
      claimed.add(w.agentId)
      if (agent) claimed.add(agent.id)
      const state = workerState(w)
      rows.push({
        key: `${s.id}/${w.bot}`,
        address: w.address,
        agentId: agent?.id ?? w.agentId,
        agent,
        swarm: s,
        worker: w,
        brain: s.brain,
        state,
        elapsedMs: (w.finishedAt ?? now) - w.startedAt,
        cost: costLabel(s.harness),
        branch: w.branch,
        lastLine: input.lastLine[agent?.id ?? w.agentId] ?? w.note ?? "",
        attachable: !!agent?.pane?.attachable,
      })
    }
    if (rows.length) {
      const task = s.task.split("\n")[0]!.trim()
      groups.push({ title: `${s.id} · ${task.length > 60 ? task.slice(0, 59) + "…" : task}`, rows })
    }
  }

  const others: BoardRow[] = []
  for (const a of input.agents) {
    if (claimed.has(a.id) || a.state === "offline" || !a.state) continue
    const h = harnessOf(a)
    others.push({
      key: a.id,
      address: a.address ?? a.id,
      agentId: a.id,
      agent: a,
      swarm: null,
      worker: null,
      brain: h ?? "—",
      state: engineState(a.state),
      elapsedMs: input.since[a.id] ? now - input.since[a.id]! : 0,
      cost: costLabel(h),
      branch: a.team ?? "—",
      lastLine: input.lastLine[a.id] ?? "",
      attachable: !!a.pane?.attachable,
    })
  }
  if (others.length) groups.push({ title: "other bots", rows: others })
  return groups
}

export function allRows(groups: BoardGroup[]): BoardRow[] {
  return groups.flatMap((g) => g.rows)
}

export interface BoardSummary {
  bots: number
  working: number
  waiting: number
  done: number
  failed: number
}

export function summarize(rows: BoardRow[]): BoardSummary {
  const n = (s: BoardState) => rows.filter((r) => r.state === s).length
  return { bots: rows.length, working: n("working") + n("starting"), waiting: n("waiting"), done: n("done"), failed: n("failed") }
}

/** "4 bots · 1 waiting · 2 done" — the header roll-up. */
export function summaryText(s: BoardSummary): string {
  const parts = [`${s.bots} bot${s.bots === 1 ? "" : "s"}`]
  if (s.waiting) parts.push(`${s.waiting} waiting`)
  if (s.working) parts.push(`${s.working} working`)
  if (s.done) parts.push(`${s.done} done`)
  if (s.failed) parts.push(`${s.failed} failed`)
  return parts.join(" · ")
}

/**
 * The TUI footer badge, "⚙ 3 bots · 1 waiting", or null when nothing is
 * running or waiting (finished-only boards don't need a badge).
 */
export function badgeText(rows: BoardRow[]): string | null {
  const s = summarize(rows)
  if (!s.working && !s.waiting) return null
  return `⚙ ${s.bots} bot${s.bots === 1 ? "" : "s"}${s.waiting ? ` · ${s.waiting} waiting` : ""}`
}

export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m${String(s % 60).padStart(2, "0")}s`
  const h = Math.floor(m / 60)
  return `${h}h${String(m % 60).padStart(2, "0")}m`
}

export function fit(text: string, width: number): string {
  if (width <= 0) return ""
  const chars = [...text]
  if (chars.length <= width) return text + " ".repeat(width - chars.length)
  return chars.slice(0, Math.max(0, width - 1)).join("") + "…"
}

/** The column layout both renderers use: `#  BOT  BRAIN  STATE  AGE  COST  BRANCH`. */
export function rowColumns(r: BoardRow, i: number, width: number) {
  const brainW = Math.min(30, Math.max(10, Math.floor(width * 0.24)))
  return {
    num: fit(String(i + 1), 3),
    bot: fit(r.address, 16),
    brain: fit(r.brain, brainW),
    state: fit(`${STATE_GLYPH[r.state]} ${STATE_WORD[r.state]}`, 10),
    age: fit(formatElapsed(r.elapsedMs), 8),
    cost: fit(r.cost, 5),
    branch: r.branch,
  }
}

export const BOARD_KEYS = "↑↓ move · 1-9 jump · n next waiting · ⏎ attach · s steer · x stop · l land · q quit"
