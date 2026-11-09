import * as React from 'react'
import { useEffect, useState } from 'react'
import { getCwdState } from '../../bootstrap/state.js'
import { Text } from '../../ink'
import { checkCounts, countDecisions, currentBranchPR, type BranchPR } from '@/cli/work/api'

/**
 * Footer chips for work waiting outside this turn: `⚑ 3` when agents in any
 * session wait on a decision (/decisions), and `PR #123 ✓4 ✗1` for this
 * branch's pull request (/pr). Both stay hidden when there is nothing to show
 * or the source (engine, gh) isn't there.
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
  const pr = usePoll<BranchPR | null>(() => currentBranchPR(getCwdState()), 60_000)
  const n = pr ? checkCounts(pr) : undefined
  if (!waiting && !pr) return null
  return (
    <Text>
      {waiting ? <Text color="warning">⚑ {waiting}  </Text> : null}
      {pr && n ? (
        <Text dimColor>
          PR #{pr.number}
          {n.pass ? <Text color="success"> ✓{n.pass}</Text> : null}
          {n.fail ? <Text color="error"> ✗{n.fail}</Text> : null}
          {n.pending ? <Text color="warning"> ●{n.pending}</Text> : null}
          {'  '}
        </Text>
      ) : null}
    </Text>
  )
}
