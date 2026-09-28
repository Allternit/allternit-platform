import { Database, sql } from "@/runtime/session/storage/db"
import { Session } from "@/runtime/session"
import { SessionHandoff } from "@/runtime/session/handoff"

/**
 * Spend limits (spec P8.1): a bot's monthly budget and a thread's budget
 * from its plan. gizzi is where every turn runs, so it's where the limit
 * holds: a session over budget pauses (SessionPause, reason "budget") until
 * the period resets or the limit is raised — no surprise bills.
 *
 *   scope "agent"   — every session of a bot (session.agent_id), per month
 *   scope "session" — one thread's whole lineage, in total
 */
export namespace Budget {
  export type Scope = "agent" | "session"

  export interface Info {
    scope: Scope
    targetID: string
    limitUsd: number
    period: "month" | "total"
    spentUsd: number
    /** When the period resets (month budgets). */
    resetsAt?: number
  }

  export function monthStart(now = Date.now()): number {
    const d = new Date(now)
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1)
  }

  export function nextMonthStart(now = Date.now()): number {
    const d = new Date(now)
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)
  }

  export function set(scope: Scope, targetID: string, limitUsd: number | null) {
    Database.use((db) => {
      if (limitUsd === null) {
        db.run(sql`DELETE FROM budget WHERE scope = ${scope} AND target_id = ${targetID}`)
        return
      }
      const period = scope === "agent" ? "month" : "total"
      db.run(sql`INSERT INTO budget (scope, target_id, limit_usd, period, time_updated)
        VALUES (${scope}, ${targetID}, ${limitUsd}, ${period}, ${Date.now()})
        ON CONFLICT(scope, target_id) DO UPDATE SET limit_usd = excluded.limit_usd, time_updated = excluded.time_updated`)
    })
  }

  function limitOf(scope: Scope, targetID: string): number | undefined {
    const row = Database.use((db) =>
      db.get<{ limit_usd: number }>(sql`SELECT limit_usd FROM budget WHERE scope = ${scope} AND target_id = ${targetID}`),
    )
    return row?.limit_usd
  }

  /** USD spent by a bot's sessions since `since` (ms). */
  export function agentSpend(agentID: string, since: number): number {
    const row = Database.use((db) =>
      db.get<{ total: number | null }>(sql`SELECT SUM(json_extract(m.data, '$.cost')) AS total FROM message m
        JOIN session s ON s.id = m.session_id
        WHERE s.agent_id = ${agentID} AND m.time_created >= ${since} AND json_extract(m.data, '$.role') = 'assistant'`),
    )
    return row?.total ?? 0
  }

  /** USD spent across a thread's lineage (every window of it). */
  export async function lineageSpend(sessionID: string): Promise<number> {
    const chain = await SessionHandoff.lineage(sessionID).catch(() => [])
    const ids = chain.length ? chain.map((s) => s.id) : [sessionID]
    let total = 0
    for (const id of ids) {
      const row = Database.use((db) =>
        db.get<{ total: number | null }>(sql`SELECT SUM(json_extract(data, '$.cost')) AS total FROM message
          WHERE session_id = ${id} AND json_extract(data, '$.role') = 'assistant'`),
      )
      total += row?.total ?? 0
    }
    return total
  }

  export async function info(scope: Scope, targetID: string, now = Date.now()): Promise<Info | undefined> {
    const limit = limitOf(scope, targetID)
    if (limit === undefined) return
    if (scope === "agent") {
      return { scope, targetID, limitUsd: limit, period: "month", spentUsd: agentSpend(targetID, monthStart(now)), resetsAt: nextMonthStart(now) }
    }
    return { scope, targetID, limitUsd: limit, period: "total", spentUsd: await lineageSpend(targetID) }
  }

  /**
   * Over budget for this turn? Returns the pause to apply: a bot's monthly
   * budget resets on the 1st; a thread budget holds until it's raised.
   */
  export async function exceeded(session: Session.Info, now = Date.now()): Promise<{ until: number; limit: string } | undefined> {
    if (session.agentID) {
      const bot = await info("agent", session.agentID, now)
      if (bot && bot.spentUsd >= bot.limitUsd) {
        return { until: bot.resetsAt!, limit: `monthly budget ($${bot.limitUsd.toFixed(2)})` }
      }
    }
    const root = (await SessionHandoff.lineage(session.id).catch(() => []))[0]?.id ?? session.id
    const thread = await info("session", root, now)
    if (thread && thread.spentUsd >= thread.limitUsd) {
      // Holds until someone raises it; re-checked daily.
      return { until: now + 24 * 3600_000, limit: `thread budget ($${thread.limitUsd.toFixed(2)})` }
    }
    return
  }
}
