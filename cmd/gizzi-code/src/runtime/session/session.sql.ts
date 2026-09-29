import { sqliteTable, text, integer, index, primaryKey } from "drizzle-orm/sqlite-core"
import { ProjectTable } from "@/runtime/context/project/project.sql"
import type { MessageV2 } from "@/runtime/session/message-v2"
import type { Snapshot } from "@/runtime/session/snapshot"
import type { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Timestamps } from "@/runtime/session/storage/schema.sql"

type PartData = Omit<MessageV2.Part, "id" | "sessionID" | "messageID">
type InfoData = Omit<MessageV2.Info, "id" | "sessionID">

export const SessionTable = sqliteTable(
  "session",
  {
    id: text().primaryKey(),
    project_id: text()
      .notNull()
      .references(() => ProjectTable.id, { onDelete: "cascade" }),
    parent_id: text(),
    slug: text().notNull(),
    directory: text().notNull(),
    title: text().notNull(),
    version: text().notNull(),
    share_url: text(),
    summary_additions: integer(),
    summary_deletions: integer(),
    summary_files: integer(),
    summary_diffs: text({ mode: "json" }).$type<Snapshot.FileDiff[]>(),
    revert: text({ mode: "json" }).$type<{ messageID: string; partID?: string; snapshot?: string; diff?: string }>(),
    permission: text({ mode: "json" }).$type<PermissionNext.Ruleset>(),
    permission_mode: text(),
    agent_id: text(),
    surface: text(),
    harness: text({ mode: "json" }).$type<{
      mode: "byok" | "cloud" | "local" | "subprocess"
      byok?: {
        anthropic?: { apiKey: string; baseURL?: string }
        openai?: { apiKey: string; baseURL?: string }
        google?: { apiKey: string; baseURL?: string }
      }
      cloud?: { baseURL: string; accessToken: string; refreshToken?: string }
      local?: { baseURL: string }
      subprocess?: { command: string; cwd?: string; env?: Record<string, string> }
    }>(),
    pinned: integer(),
    default_model: text({ mode: "json" }).$type<{ providerID: string; modelID: string; authProfileId?: string; source?: "user" | "auto" }>(),
    source_harness: text(),
    source_session_id: text(),
    source_path: text(),
    source_snapshot_hash: text(),
    source_snapshot_at: integer(),
    source_event_id: text(),
    source_native_hash: text(),
    source_fetched_hash: text(),
    source_export: text({ mode: "json" }).$type<{
      harness: string
      sessionId: string
      path: string
      resumeHint: string
      at: number
    }>(),
    // Session lineage (context handoff): the session this one continues, and
    // on the predecessor, where it handed off to with the checkpoint baton.
    continues_from: text(),
    handoff: text({ mode: "json" }).$type<{
      sessionID: string
      reason: string
      at: number
      baton?: unknown
    }>(),
    // Landed before a usage limit (P3.17): resumes on its own at `until`.
    paused: text({ mode: "json" }).$type<{
      until: number
      limit: string
      providerID?: string
      reason: "quota" | "rate_limit" | "limit_hit" | "budget" | "budget-check-failed"
      at: number
      suggest?: { providerID: string; modelID: string; label: string; headroom?: number }
    }>(),
    // Where the session stands against its model's usage limit: approaching,
    // wrapping up mid-turn, wrapped up / paused. Cleared on resume or reset.
    limit_state: text({ mode: "json" }).$type<{
      state: "ok" | "approaching" | "wrapping_up" | "wrapped" | "paused"
      providerID: string
      windowID: string
      label: string
      usedRatio: number
      resetAt?: number
      at: number
    }>(),
    ...Timestamps,
    time_compacting: integer(),
    time_archived: integer(),
  },
  (table) => [
    index("session_project_idx").on(table.project_id),
    index("session_parent_idx").on(table.parent_id),
    index("session_continues_from_idx").on(table.continues_from),
    index("session_agent_idx").on(table.agent_id),
    index("session_surface_idx").on(table.surface),
    index("session_pinned_idx").on(table.pinned),
    index("session_source_idx").on(table.source_harness, table.source_session_id),
  ],
)

export const MessageTable = sqliteTable(
  "message",
  {
    id: text().primaryKey(),
    session_id: text()
      .notNull()
      .references(() => SessionTable.id, { onDelete: "cascade" }),
    ...Timestamps,
    data: text({ mode: "json" }).notNull().$type<InfoData>(),
  },
  (table) => [index("message_session_idx").on(table.session_id)],
)

export const PartTable = sqliteTable(
  "part",
  {
    id: text().primaryKey(),
    message_id: text()
      .notNull()
      .references(() => MessageTable.id, { onDelete: "cascade" }),
    session_id: text().notNull(),
    ...Timestamps,
    data: text({ mode: "json" }).notNull().$type<PartData>(),
  },
  (table) => [index("part_message_idx").on(table.message_id), index("part_session_idx").on(table.session_id)],
)

/** Append-only replay ledger. Mutable message/part tables remain the current-state projection. */
export const SessionTraceTable = sqliteTable(
  "session_trace",
  {
    sequence: integer().primaryKey({ autoIncrement: true }),
    session_id: text()
      .notNull()
      .references(() => SessionTable.id, { onDelete: "cascade" }),
    kind: text().notNull(),
    message_id: text(),
    part_id: text(),
    data: text({ mode: "json" }).notNull().$type<unknown>(),
    time_created: integer().notNull(),
  },
  (table) => [
    index("session_trace_cursor_idx").on(table.session_id, table.sequence),
    index("session_trace_message_idx").on(table.message_id),
  ],
)

export const TodoTable = sqliteTable(
  "todo",
  {
    session_id: text()
      .notNull()
      .references(() => SessionTable.id, { onDelete: "cascade" }),
    content: text().notNull(),
    status: text().notNull(),
    priority: text().notNull(),
    position: integer().notNull(),
    ...Timestamps,
  },
  (table) => [
    primaryKey({ columns: [table.session_id, table.position] }),
    index("todo_session_idx").on(table.session_id),
  ],
)

export const PermissionTable = sqliteTable("permission", {
  project_id: text()
    .primaryKey()
    .references(() => ProjectTable.id, { onDelete: "cascade" }),
  ...Timestamps,
  data: text({ mode: "json" }).notNull().$type<PermissionNext.Ruleset>(),
})

export const BackgroundTaskTable = sqliteTable(
  "background_task",
  {
    id: text().primaryKey(),
    parent_session_id: text()
      .notNull()
      .references(() => SessionTable.id, { onDelete: "cascade" }),
    child_session_id: text().references(() => SessionTable.id, { onDelete: "set null" }),
    kind: text().notNull(),
    status: text().notNull(),
    description: text().notNull(),
    output: text(),
    error: text(),
    time_finished: integer(),
    ...Timestamps,
  },
  (table) => [
    index("background_task_parent_idx").on(table.parent_session_id, table.status),
    index("background_task_child_idx").on(table.child_session_id),
  ],
)

export const RoutineTable = sqliteTable(
  "routine",
  {
    id: text().primaryKey(),
    agent_id: text(),
    name: text().notNull(),
    steps: text({ mode: "json" }).notNull().$type<Array<{ command: string; status: 'pending' | 'running' | 'done' | 'failed' }>>(),
    trigger: text(),
    schedule: text(),
    state: text().notNull().default("defined"),
    ...Timestamps,
  },
  (table) => [
    index("routine_agent_idx").on(table.agent_id),
  ]
)

export const LoopTable = sqliteTable(
  "loop",
  {
    id: text().primaryKey(),
    agent_id: text(),
    command: text().notNull(),
    exit_condition: text(),
    max_iterations: integer().notNull().default(10),
    iteration_log: text({ mode: "json" }).notNull().$type<Array<{ iteration: number; output: string; exitCode: number; timestamp: string }>>(),
    state: text().notNull().default("running"),
    ...Timestamps,
  },
  (table) => [
    index("loop_agent_idx").on(table.agent_id),
  ]
)

export const GoalTable = sqliteTable(
  "goal",
  {
    id: text().primaryKey(),
    agent_id: text(),
    objective: text().notNull(),
    completion_criterion: text(),
    milestones: text({ mode: "json" }).notNull().$type<Array<{ name: string; status: string; completedAt?: string }>>(),
    validations: text({ mode: "json" }).notNull().$type<Array<{ testName: string; status: string; output?: string }>>(),
    budget: text({ mode: "json" }).notNull().$type<{
      turnBudget: number | null
      tokenBudget: number | null
      wallClockBudgetMs: number | null
    }>(),
    usage: text({ mode: "json" }).notNull().$type<{
      turnsUsed: number
      tokensUsed: number
      wallClockMs: number
      lastStartedAt: number | null
    }>(),
    blocked_audit: text({ mode: "json" }).notNull().$type<{
      fingerprint: string | null
      consecutiveTurns: number
    }>(),
    terminal_reason: text(),
    queue_position: integer(),
    revision: integer().notNull().default(1),
    state: text().notNull().default("planning"),
    progress: integer().notNull().default(0),
    time_started: integer(),
    time_finished: integer(),
    ...Timestamps,
  },
  (table) => [
    index("goal_agent_idx").on(table.agent_id),
    index("goal_state_queue_idx").on(table.state, table.queue_position),
  ]
)

/** Inbound native origin timeline. Fetched when the canonical CLI file moves; never written back. */
export const SessionSourceEventTable = sqliteTable(
  "session_source_event",
  {
    session_id: text()
      .notNull()
      .references(() => SessionTable.id, { onDelete: "cascade" }),
    sequence: integer().notNull(),
    event_id: text(),
    kind: text().notNull(),
    role: text(),
    text: text(),
    tool_name: text(),
    fetched_at: integer().notNull(),
  },
  (table) => [
    primaryKey({ columns: [table.session_id, table.sequence] }),
    index("session_source_event_session_idx").on(table.session_id),
  ],
)
