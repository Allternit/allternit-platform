import z from "zod/v4"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { SessionCompaction } from "@/runtime/session/compaction"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { ContextBreakdownStore } from "@/runtime/session/context-breakdown-store"
import { Provider } from "@/runtime/providers/provider"
import { ProviderQuotas, type QuotaResult } from "@/runtime/providers/quota"
import { Log } from "@/shared/util/log"
import { Database, and, eq, sql } from "@/runtime/session/storage/db"
import { MessageTable } from "@/runtime/session/session.sql"

/**
 * The "Context window" panel's data for one session: what fills the window
 * (ContextBreakdown, reconciled with the provider's reported usage) and the
 * provider's plan usage limits (ProviderQuotas), in one read. It also
 * says how many times the session was compacted (Primitives Wave B).
 */
export namespace SessionContextUsage {
  const log = Log.create({ service: "session.context-usage" })

  /** How long the panel waits on a quota fetch before using what is already known. */
  const QUOTA_WAIT_MS = 2_500

  export const Result = z
    .object({
      breakdown: ContextBreakdown.Snapshot.nullable(),
      /** ProviderQuotas result for the session's provider; null when not requested. */
      limits: z.any().nullable(),
      /** How many times this session has been compacted (finished compaction summaries). */
      compactions: z.number(),
    })
    .meta({ ref: "SessionContextUsage" })
  export type Result = { breakdown: ContextBreakdown.Snapshot | null; limits: QuotaResult | null; compactions: number }

  function contextTokens(tokens: MessageV2.Assistant["tokens"]) {
    return tokens.input + tokens.cache.read + tokens.cache.write + tokens.output
  }

  async function latestAssistant(sessionID: string) {
    const messages = await Session.messages({ sessionID, limit: 50 })
    for (let i = messages.length - 1; i >= 0; i--) {
      const info = messages[i].info
      if (info.role !== "assistant" || info.summary) continue
      if (contextTokens(info.tokens) > 0) return info
    }
    return undefined
  }

  /**
   * Finished compactions in this session: assistant messages marked `summary`
   * that reached a finish (the same test MessageV2.filterCompacted uses). A
   * failed or running compaction does not count. One indexed count, no parts.
   */
  export function compactions(sessionID: string): number {
    const row = Database.use((db) =>
      db
        .select({ count: sql<number>`count(*)` })
        .from(MessageTable)
        .where(
          and(
            eq(MessageTable.session_id, sessionID),
            sql`json_extract(${MessageTable.data}, '$.role') = 'assistant'`,
            sql`json_extract(${MessageTable.data}, '$.summary') = 1`,
            sql`json_extract(${MessageTable.data}, '$.finish') IS NOT NULL`,
          ),
        )
        .get(),
    )
    return Number(row?.count ?? 0)
  }

  async function model(providerID: string, modelID: string) {
    return Provider.getModel(providerID, modelID).catch(() => undefined)
  }

  async function limitsFor(providerID: string): Promise<QuotaResult> {
    const timeout = new Promise<undefined>((resolve) => setTimeout(() => resolve(undefined), QUOTA_WAIT_MS))
    const result = await Promise.race([ProviderQuotas.get(providerID), timeout]).catch((error) => {
      log.warn("quota read failed", { providerID, error: error instanceof Error ? error.message : String(error) })
      return undefined
    })
    return result ?? ProviderQuotas.peek(providerID) ?? { status: "unsupported" }
  }

  export async function read(sessionID: string, options: { limits?: boolean } = {}): Promise<Result> {
    await Session.get(sessionID)
    const snapshot = await ContextBreakdownStore.get(sessionID)
    const assistant = await latestAssistant(sessionID)
    const providerID = snapshot?.providerID ?? assistant?.providerID
    const modelID = snapshot?.modelID ?? assistant?.modelID
    const info = providerID && modelID ? await model(providerID, modelID) : undefined
    const autoCompactBuffer = info ? await SessionCompaction.bufferTokens(info) : undefined
    // The reported usage belongs to the measured request only once that request's reply landed.
    const reported = assistant && (!snapshot || assistant.parentID === snapshot.messageID) ? assistant : undefined

    let breakdown: ContextBreakdown.Snapshot | null = null
    if (snapshot?.source === "brain") {
      // The brain measured it (its own /context or context total) after the
      // turn: its numbers stand, including its own autocompact buffer.
      breakdown = snapshot
    } else if (snapshot) {
      breakdown = ContextBreakdown.reconcile(snapshot, {
        providerContextTokens: reported ? contextTokens(reported.tokens) : undefined,
        autoCompactBuffer,
      })
    } else if (reported && info) {
      breakdown = ContextBreakdown.fromUsage({
        sessionID,
        messageID: reported.parentID,
        providerID: reported.providerID,
        modelID: reported.modelID,
        contextWindow: info.limit.context,
        providerContextTokens: contextTokens(reported.tokens),
        autoCompactBuffer: autoCompactBuffer ?? 0,
      })
    }

    if (breakdown?.source === "brain" && breakdown.brain) {
      const brain = await Provider.getProvider(breakdown.brain).catch(() => undefined)
      if (brain?.name) breakdown = { ...breakdown, brainName: brain.name }
    }

    const limits = options.limits === false || !providerID ? null : await limitsFor(providerID)
    return { breakdown, limits, compactions: compactions(sessionID) }
  }
}
