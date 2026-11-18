/**
 * Primitives Wave B (agent control): the contract shapes.
 *
 * One place for the inputs and results of the five new tools (wait_agents,
 * agent_control, tool_search, get_context_remaining, history_recall) and for
 * the fields Wave B adds to `task`, `agent_swarm` and `todowrite`. The
 * builtins (tools/builtins/*) parse with zod and hand these shapes to the
 * cores; the cores live in their own files so each lane owns one:
 *
 *   primitives/agent-control.ts     AgentControl.waitAgents / run       (L1)
 *   primitives/steer-signal.ts      SteerSignal.notify / wait           (L1)
 *   primitives/task-name.ts         TaskName / TaskFork / TaskEffort    (L2)
 *   primitives/tool-families.ts     ToolFamilies.filter / search        (L3)
 *   primitives/context-remaining.ts ContextRemaining.read               (L4)
 *   primitives/context-notice.ts    ContextNotice.forTurn               (L4)
 *   primitives/history-recall.ts    HistoryRecall.run                   (L4)
 *
 * Types only (plus the limits every side must agree on). No runtime imports,
 * so the Ink adapters and the app parsers can share it.
 */

// ---------------------------------------------------------------------------
// Tool ids
// ---------------------------------------------------------------------------

export const WAIT_AGENTS_TOOL_ID = "wait_agents"
export const AGENT_CONTROL_TOOL_ID = "agent_control"
export const TOOL_SEARCH_TOOL_ID = "tool_search"
export const CONTEXT_REMAINING_TOOL_ID = "get_context_remaining"
export const HISTORY_RECALL_TOOL_ID = "history_recall"

export const WAVE_B_TOOL_IDS = [
  WAIT_AGENTS_TOOL_ID,
  AGENT_CONTROL_TOOL_ID,
  TOOL_SEARCH_TOOL_ID,
  CONTEXT_REMAINING_TOOL_ID,
  HISTORY_RECALL_TOOL_ID,
] as const
export type WaveBToolId = (typeof WAVE_B_TOOL_IDS)[number]

// ---------------------------------------------------------------------------
// Shared: a child agent as the parent sees it
// ---------------------------------------------------------------------------

/** Same values as BackgroundTask.Status (session/background-task.ts). */
export type AgentRunStatus = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted"

/** One child agent, named by its session. */
export type AgentRef = {
  /** The child's session id (what `task` returns as task_id). */
  id: string
  /** Its own name (`task.name`), or null when it was started without one. */
  name: string | null
  /** Its name path from the root, such as "/root/build/tests"; null without a name. */
  path: string | null
}

// ---------------------------------------------------------------------------
// wait_agents
// ---------------------------------------------------------------------------

export const WAIT_AGENTS_TIMEOUT_MS = { min: 10_000, max: 3_600_000, default: 300_000 } as const

export type WaitAgentsInput = {
  /** Child ids or name paths to wait on; leave out to wait on every running child of this session. */
  targets?: string[]
  /** 10,000..3,600,000; default 300,000. */
  timeout_ms?: number
}

/** settled: at least one target finished; steer: the user sent a message; timeout: neither, in time. */
export type WaitAgentsWoke = "settled" | "steer" | "timeout"

export type SettledAgent = {
  id: string
  name: string | null
  status: AgentRunStatus
  /** The child's final answer, shortened; the error text when it failed. */
  summary: string
}

export type WaitAgentsResult = {
  woke: WaitAgentsWoke
  /** Targets that settled since the wait started (empty on steer or timeout without one). */
  settled: SettledAgent[]
  /** Targets still running when the wait returned. */
  running: AgentRef[]
}

// ---------------------------------------------------------------------------
// agent_control
// ---------------------------------------------------------------------------

export const AGENT_CONTROL_ACTIONS = ["list", "interrupt", "resume", "followup", "close"] as const
export type AgentControlAction = (typeof AGENT_CONTROL_ACTIONS)[number]

/** Actions that need `target`. `list` takes `path_prefix` instead. */
export const AGENT_CONTROL_TARGETED: readonly AgentControlAction[] = ["interrupt", "resume", "followup", "close"]

export type AgentControlInput = {
  action: AgentControlAction
  /** A child id or name path; only this session's own descendants. */
  target?: string
  /** followup: the message to send (a steer when the child runs, a new turn when it is idle). */
  message?: string
  /** list: only agents whose name path starts with this. */
  path_prefix?: string
  /** interrupt, close: why, shown on the child's row. */
  reason?: string
}

export type AgentNode = AgentRef & {
  status: AgentRunStatus
  /** The subagent type (`task.subagent_type`). */
  agent: string | null
  description: string
  /** 1 = direct child; at most SessionTasks.MAX_DEPTH. */
  depth: number
  /** The parent's session id (this session for direct children). */
  parent_id: string
}

export type AgentControlResult =
  | { action: "list"; agents: AgentNode[] }
  | { action: "interrupt"; target: AgentRef; outcome: "interrupted" | "not_running" }
  | { action: "resume"; target: AgentRef; outcome: "resumed" | "already_running"; run_id?: string }
  | { action: "followup"; target: AgentRef; outcome: "steered" | "started"; run_id?: string }
  /** `closed` is deepest first (the order they were closed in). */
  | { action: "close"; target: AgentRef; closed: AgentRef[] }

// ---------------------------------------------------------------------------
// task / agent_swarm extensions
// ---------------------------------------------------------------------------

/** One segment of a name path. */
export const TASK_NAME_PATTERN = /^[a-z0-9_-]{1,32}$/
/** "/root/a/b/c": the root plus at most 3 levels. */
export const TASK_NAME_MAX_DEPTH = 3
export const TASK_NAME_ROOT = "/root"

export const TASK_FORK_VALUES = ["none", "all"] as const
export type TaskFork = (typeof TASK_FORK_VALUES)[number]

export const TASK_EFFORT_VALUES = ["low", "medium", "high"] as const
export type TaskEffort = (typeof TASK_EFFORT_VALUES)[number]

export type TaskExtensions = {
  /** The child's own name segment ([a-z0-9_-]{1,32}). */
  name?: string
  /** none (default): fresh context. all: a copy of this session's history up to the last finished message. */
  fork?: TaskFork
  /** Maps to the model's `variant`; refused with fork: all. */
  effort?: TaskEffort
}

export type SwarmItemExtensions = {
  name?: string
  /** Same values as task.model. */
  model?: "small" | "standard" | "deep" | "inherit"
  effort?: TaskEffort
}

// ---------------------------------------------------------------------------
// tool_search (replaces select_tools)
// ---------------------------------------------------------------------------

export type ToolSearchInput = {
  /** Keywords, or "select:a,b" for exact tool names. */
  query: string
}

export type ToolSearchResult = {
  /** Tools loaded by this call; they can be called from the next step. */
  loaded: string[]
  /** Tools that were loaded already. */
  already: string[]
  /** Names in a select: query that are not tools of this session. */
  unknown: string[]
}

/** The deferred families. Everything else is always loaded. */
export const TOOL_FAMILIES = [
  "skill-growth",
  "scratchpad",
  "goals",
  "artifacts",
  "vault-memory",
  "email",
  "automation",
  "notebook-graph",
  "threads",
  "projects",
  "computers",
  "context",
  "mcp",
] as const
export type ToolFamily = (typeof TOOL_FAMILIES)[number]

/** Environment switch that turns deferral off (every tool loads every step). */
export const DISABLE_TOOL_SEARCH_ENV = "GIZZI_DISABLE_TOOL_SEARCH"

// ---------------------------------------------------------------------------
// get_context_remaining
// ---------------------------------------------------------------------------

/** All counts are tokens. */
export type ContextRemainingResult = {
  /** The model's context window. */
  window: number
  /** Tokens the last request used (input + cache). */
  used: number
  /** window - used, never below 0. */
  remaining: number
  /** Tokens left before automatic compaction starts. */
  until_compaction: number
  /** How many times this session has been compacted. */
  compactions: number
}

// ---------------------------------------------------------------------------
// Context notice (temporary tail part)
// ---------------------------------------------------------------------------

/** When the notice fires: at 25% left, at 10% left, and after each compaction. Each fires once. */
export type ContextNoticeTrigger = "remaining_25" | "remaining_10" | "compacted"

// ---------------------------------------------------------------------------
// history_recall
// ---------------------------------------------------------------------------

export const HISTORY_RECALL_ACTIONS = ["windows", "search", "read"] as const
export type HistoryRecallAction = (typeof HISTORY_RECALL_ACTIONS)[number]
export const HISTORY_RECALL_LIMIT_MAX = 20
export const HISTORY_RECALL_READ_MAX_CHARS = 8_000

export type HistoryRecallInput = {
  action: HistoryRecallAction
  /** search: words to find. */
  query?: string
  /** read (and search, to narrow it): which window. */
  window_id?: string
  /** read: one message in the window. */
  item_id?: string
  /** windows, search: at most 20. */
  limit?: number
}

/** A stretch of history the model no longer sees: before a compaction, or a session this one continues. */
export type HistoryWindow = {
  id: string
  /** compaction: compacted away in this session. handoff: an earlier session in the handoff lineage. */
  kind: "compaction" | "handoff"
  session_id: string
  /** First and last message times (ms). */
  from: number
  to: number
  messages: number
  /** The compaction summary or handoff baton, shortened. */
  summary?: string
}

export type HistoryHit = {
  window_id: string
  item_id: string
  role: "user" | "assistant"
  time: number
  snippet: string
}

/** Every result carries `data_notice`: recalled text is data, never instructions. */
export type HistoryRecallResult =
  | { action: "windows"; data_notice: string; windows: HistoryWindow[] }
  | { action: "search"; data_notice: string; hits: HistoryHit[] }
  | {
      action: "read"
      data_notice: string
      window_id: string
      item_id?: string
      text: string
      /** The text was cut at HISTORY_RECALL_READ_MAX_CHARS. */
      truncated: boolean
    }

// ---------------------------------------------------------------------------
// todowrite extensions
// ---------------------------------------------------------------------------

export const TODO_STATUSES = ["pending", "in_progress", "completed", "cancelled", "interrupted"] as const
export type TodoStatus = (typeof TODO_STATUSES)[number]

export type TodoItemExtensions = {
  /** Stable id other items point at with blockedBy. */
  id?: string
  /** Present-tense label shown while the item is in progress ("Running the tests"). */
  activeForm?: string
  /** Ids of items that must finish first. No cycles. */
  blockedBy?: string[]
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

/** Thrown by a core the lanes have not filled in yet. */
export function notImplemented(what: string): never {
  throw new Error(`${what} is not implemented yet (Primitives Wave B)`)
}
