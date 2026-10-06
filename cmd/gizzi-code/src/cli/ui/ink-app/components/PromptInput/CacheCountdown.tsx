import * as React from 'react'
import { useEffect, useState } from 'react'
import { Text } from '../../ink'
import type { Message } from '../../types/message'
import { getTokenUsage } from '../../utils/tokens'
import { getPromptCache1hAllowlist, getPromptCache1hEligible } from '../../bootstrap/state'
import { cacheCountdown } from '../../../../../shared/util/cache-countdown'

/** Separate row preserves user status-line output and updates without running a command. */
export function CacheCountdown({ messages, model }: { messages: Message[]; model: string }) {
  const [now, setNow] = useState(Date.now)
  const boundary = messages.findLastIndex(m => m.type === 'system' && m.subtype === 'compact_boundary')
  const last = messages.slice(boundary + 1).findLast(m => getTokenUsage(m) !== undefined)
  const usage = last ? getTokenUsage(last) : undefined
  // Omit stale model data immediately after a switch. Unknown providers have no TTL.
  const responseModel = last?.type === 'assistant' ? last.message.model : undefined
  const cached = (usage?.cache_read_input_tokens ?? 0) + (usage?.cache_creation_input_tokens ?? 0)
  const at = last?.timestamp ? new Date(last.timestamp).getTime() : NaN
  const eligible = getPromptCache1hEligible() === true
  const allowlist = getPromptCache1hAllowlist() ?? []
  const source = 'repl_main_thread'
  const oneHour = eligible && allowlist.some(p => p.endsWith('*') ? source.startsWith(p.slice(0, -1)) : p === source)
  const ttl = oneHour ? 3600 : 300
  const available = cached > 0 && Number.isFinite(at) && responseModel === model.split('/').pop() && model.includes('claude')
  const status = available ? cacheCountdown({
    cacheTtlSeconds: ttl, cacheExpiresAt: at + ttl * 1000,
    cacheRecacheTokens: (usage?.input_tokens ?? 0) + cached,
    inputTokens: usage?.input_tokens, cacheReadTokens: usage?.cache_read_input_tokens,
    cacheWriteTokens: usage?.cache_creation_input_tokens,
  }, now) : undefined
  useEffect(() => {
    if (!available) return
    setNow(Date.now())
    const timer = setInterval(() => setNow(Date.now()), 30_000)
    return () => clearInterval(timer)
  }, [available, at])
  if (!status) return null
  return <Text color={status.state === 'cold' ? 'red' : status.state === 'expiring' ? 'yellow' : 'green'} wrap="truncate-end">{status.label}</Text>
}
