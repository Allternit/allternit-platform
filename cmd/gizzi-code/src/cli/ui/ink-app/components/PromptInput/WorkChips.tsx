import * as React from 'react'
import { useEffect, useState } from 'react'
import { Text } from '../../ink'
import { countDecisions } from '@/cli/work/api'

/**
 * `⚑ 3` when agents in any session wait on a decision (/decisions); hidden
 * when nothing waits or the engine isn't there. This branch's PR checks
 * (`PR #123 ✓4 ✗1`) ride on the existing PR badge (usePrStatus / PrBadge).
 */
function usePoll<T>(load: () => Promise<T>, ms: number): T | undefined {
  const [value, setValue] = useState<T>()
  useEffect(() => {
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const run = async () => {
      const next = await load().catch(() => undefined)
      if (stopped) return
      setValue(next)
      timer = setTimeout(run, ms)
    }
    void run()
    return () => {
      stopped = true
      if (timer) clearTimeout(timer)
    }
  }, [ms])
  return value
}

export function WorkChips(): React.ReactNode {
  const waiting = usePoll(countDecisions, 10_000)
  if (!waiting) return null
  return <Text color="warning">⚑ {waiting}  </Text>
}
