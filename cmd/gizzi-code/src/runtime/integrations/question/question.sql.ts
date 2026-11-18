import { sqliteTable, text, integer, index } from "drizzle-orm/sqlite-core"

/**
 * Questions asked with `ask_user_later`: the turn does not wait, so the ask
 * lives here until the person answers (a new turn in the root session), it
 * expires after 7 days, or it is dismissed. Blocking `question` asks stay in
 * memory (integrations/question/index.ts).
 *
 * `session_id` is the session that asked (maybe a subagent); `root_session_id`
 * is the thread the ask belongs to, where the card shows and the answer lands.
 * No foreign key: deleting a session dismisses its asks (Session.remove).
 */
export const QuestionAsyncTable = sqliteTable(
  "question_async",
  {
    id: text().primaryKey(),
    session_id: text().notNull(),
    root_session_id: text().notNull(),
    // Question.Info[] (no plan)
    questions: text({ mode: "json" }).notNull().$type<unknown[]>(),
    assumption: text().notNull(),
    status: text().notNull().$type<"pending" | "answered" | "expired" | "dismissed">(),
    created_at: integer().notNull(),
    answered_at: integer(),
    // { answers: string[][] }
    answer: text({ mode: "json" }).$type<{ answers: string[][] }>(),
  },
  (table) => [
    index("question_async_root_status_idx").on(table.root_session_id, table.status),
    index("question_async_status_created_idx").on(table.status, table.created_at),
  ],
)
