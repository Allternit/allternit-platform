import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import z from "zod/v4"
import { fn } from "@/shared/util/fn"
import { Global } from "@/runtime/context/global"
import path from "path"
import { Filesystem } from "@/shared/util/filesystem"
import { Log } from "@/shared/util/log"
import { Provider } from "@/runtime/providers/provider"

export namespace SessionUsage {
  const log = Log.create({ service: "session.usage" })
  const USAGE_FILE = path.join(Global.Path.cache, "usage.json")

  export const Event = {
    Updated: BusEvent.define(
      "session.usage.updated",
      z.object({
        sessionID: z.string(),
        usage: z.object({
          tokens: z.number(),
          cost: z.number(),
        }),
      }),
    ),
  }

  export interface UsageEntry {
    timestamp: number
    sessionID: string
    messageID: string
    providerID: string
    modelID: string
    tokens: {
      input: number
      output: number
      reasoning: number
      cache: {
        read: number
        write: number
      }
    }
    cost: number
    /** O5/O8: the call type this usage came from ("answer" when unspecified). Absent on pre-O8 rows. */
    callType?: string
  }

  /**
   * O8 cache-hit counter. `prompt` is every input token the provider saw
   * (uncached input + cache reads + cache writes; gizzi's `tokens.input`
   * already excludes cached tokens), `read` is the cached share, and
   * `hitRate = read / prompt` (0 when nothing was sent).
   */
  export interface CacheStats {
    read: number
    write: number
    prompt: number
    hitRate: number
  }

  export function emptyCacheStats(): CacheStats {
    return { read: 0, write: 0, prompt: 0, hitRate: 0 }
  }

  export function addCacheStats(stats: CacheStats, tokens: UsageEntry["tokens"]): CacheStats {
    stats.read += tokens.cache.read
    stats.write += tokens.cache.write
    stats.prompt += tokens.input + tokens.cache.read + tokens.cache.write
    stats.hitRate = stats.prompt > 0 ? stats.read / stats.prompt : 0
    return stats
  }

  /** Cache hit rate over a set of usage entries, overall and per call type. */
  export function cacheHitRate(entries: Pick<UsageEntry, "tokens" | "callType">[]) {
    const total = emptyCacheStats()
    const byCallType: Record<string, CacheStats> = {}
    for (const entry of entries) {
      addCacheStats(total, entry.tokens)
      const key = entry.callType ?? "answer"
      addCacheStats((byCallType[key] ??= emptyCacheStats()), entry.tokens)
    }
    return { total, byCallType }
  }

  export interface DailyUsage {
    date: string // YYYY-MM-DD
    sessions: string[]
    providers: Record<string, { tokens: number; cost: number }>
    models: Record<string, { tokens: number; cost: number }>
    total: {
      tokens: number
      cost: number
      messages: number
    }
  }

  export interface SessionUsageSummary {
    sessionID: string
    startTime: number
    endTime?: number
    messageCount: number
    total: {
      tokens: number
      cost: number
    }
    byModel: Record<string, { tokens: number; cost: number }>
    byProvider: Record<string, { tokens: number; cost: number }>
  }

  export interface UsageSummary {
    entries: UsageEntry[]
    daily: DailyUsage[]
    sessions: SessionUsageSummary[]
    grandTotal: {
      tokens: number
      cost: number
      messages: number
      sessions: number
    }
    /** O8: prompt-cache hit rate over the filtered entries, overall and per call type. */
    cache: ReturnType<typeof cacheHitRate>
  }

  // In-memory cache
  let cache: UsageEntry[] | undefined

  async function load(): Promise<UsageEntry[]> {
    if (cache) return cache
    try {
      const data = await Filesystem.readJson(USAGE_FILE)
      cache = (data as UsageEntry[]) ?? []
      return cache
    } catch {
      cache = []
      return cache
    }
  }

  async function save(entries: UsageEntry[]) {
    cache = entries
    await Filesystem.write(USAGE_FILE, JSON.stringify(entries, null, 2))
  }

  export const record = fn(
    z.object({
      sessionID: z.string(),
      messageID: z.string(),
      providerID: z.string(),
      modelID: z.string(),
      tokens: z.object({
        input: z.number(),
        output: z.number(),
        reasoning: z.number(),
        cache: z.object({
          read: z.number(),
          write: z.number(),
        }),
      }),
      cost: z.number(),
      callType: z.string().optional(),
    }),
    async (input) => {
      const entries = await load()
      const entry: UsageEntry = {
        timestamp: Date.now(),
        sessionID: input.sessionID,
        messageID: input.messageID,
        providerID: input.providerID,
        modelID: input.modelID,
        tokens: input.tokens,
        cost: input.cost,
        ...(input.callType ? { callType: input.callType } : {}),
      }
      entries.push(entry)
      
      // Keep only last 90 days to prevent file bloat
      const cutoff = Date.now() - 90 * 24 * 60 * 60 * 1000
      const filtered = entries.filter(e => e.timestamp >= cutoff)
      
      await save(filtered)
      
      const totalTokens = 
        entry.tokens.input + 
        entry.tokens.output + 
        entry.tokens.cache.read + 
        entry.tokens.cache.write
      
      Bus.publish(Event.Updated, {
        sessionID: input.sessionID,
        usage: {
          tokens: totalTokens,
          cost: entry.cost,
        },
      })
    },
  )

  export async function getSummary(options?: {
    sessionID?: string
    startDate?: Date
    endDate?: Date
    limit?: number
  }): Promise<UsageSummary> {
    const entries = await load()
    
    let filtered = entries
    
    if (options?.sessionID) {
      filtered = filtered.filter(e => e.sessionID === options.sessionID)
    }
    
    if (options?.startDate) {
      filtered = filtered.filter(e => e.timestamp >= options.startDate!.getTime())
    }
    
    if (options?.endDate) {
      filtered = filtered.filter(e => e.timestamp <= options.endDate!.getTime())
    }
    
    if (options?.limit) {
      filtered = filtered.slice(-options.limit)
    }

    // Group by day
    const dailyMap = new Map<string, DailyUsage>()
    const sessionMap = new Map<string, SessionUsageSummary>()
    
    for (const entry of filtered) {
      const date = new Date(entry.timestamp).toISOString().split('T')[0]
      
      // Daily aggregation
      if (!dailyMap.has(date)) {
        dailyMap.set(date, {
          date,
          sessions: [],
          providers: {},
          models: {},
          total: { tokens: 0, cost: 0, messages: 0 },
        })
      }
      const day = dailyMap.get(date)!
      if (!day.sessions.includes(entry.sessionID)) {
        day.sessions.push(entry.sessionID)
      }
      
      const totalTokens = 
        entry.tokens.input + 
        entry.tokens.output + 
        entry.tokens.cache.read + 
        entry.tokens.cache.write
      
      if (!day.providers[entry.providerID]) {
        day.providers[entry.providerID] = { tokens: 0, cost: 0 }
      }
      day.providers[entry.providerID].tokens += totalTokens
      day.providers[entry.providerID].cost += entry.cost
      
      if (!day.models[entry.modelID]) {
        day.models[entry.modelID] = { tokens: 0, cost: 0 }
      }
      day.models[entry.modelID].tokens += totalTokens
      day.models[entry.modelID].cost += entry.cost
      
      day.total.tokens += totalTokens
      day.total.cost += entry.cost
      day.total.messages++
      
      // Session aggregation
      if (!sessionMap.has(entry.sessionID)) {
        sessionMap.set(entry.sessionID, {
          sessionID: entry.sessionID,
          startTime: entry.timestamp,
          messageCount: 0,
          total: { tokens: 0, cost: 0 },
          byModel: {},
          byProvider: {},
        })
      }
      const session = sessionMap.get(entry.sessionID)!
      session.endTime = entry.timestamp
      session.messageCount++
      session.total.tokens += totalTokens
      session.total.cost += entry.cost
      
      if (!session.byModel[entry.modelID]) {
        session.byModel[entry.modelID] = { tokens: 0, cost: 0 }
      }
      session.byModel[entry.modelID].tokens += totalTokens
      session.byModel[entry.modelID].cost += entry.cost
      
      if (!session.byProvider[entry.providerID]) {
        session.byProvider[entry.providerID] = { tokens: 0, cost: 0 }
      }
      session.byProvider[entry.providerID].tokens += totalTokens
      session.byProvider[entry.providerID].cost += entry.cost
    }
    
    const daily = Array.from(dailyMap.values()).sort((a, b) => b.date.localeCompare(a.date))
    const sessions = Array.from(sessionMap.values()).sort((a, b) => b.startTime - a.startTime)
    
    const grandTotal = sessions.reduce(
      (acc, s) => ({
        tokens: acc.tokens + s.total.tokens,
        cost: acc.cost + s.total.cost,
        messages: acc.messages + s.messageCount,
        sessions: acc.sessions + 1,
      }),
      { tokens: 0, cost: 0, messages: 0, sessions: 0 },
    )
    
    return {
      entries: filtered,
      daily,
      sessions,
      grandTotal,
      cache: cacheHitRate(filtered),
    }
  }

  export async function getSessionUsage(sessionID: string): Promise<SessionUsageSummary | undefined> {
    const summary = await getSummary({ sessionID })
    return summary.sessions[0]
  }

  export function formatCost(cost: number): string {
    if (cost < 0.01) return `$${cost.toFixed(4)}`
    if (cost < 1) return `$${cost.toFixed(2)}`
    return `$${cost.toFixed(2)}`
  }

  export function formatTokens(tokens: number): string {
    if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`
    if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`
    return tokens.toString()
  }

  export async function clear() {
    cache = []
    await Filesystem.write(USAGE_FILE, "[]")
  }
}
