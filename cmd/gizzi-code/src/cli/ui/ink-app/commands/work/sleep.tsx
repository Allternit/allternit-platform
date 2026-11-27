import * as React from 'react'
import { useState } from 'react'
import { Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { getSleepStatus, runSleepNow, setSleepEnabled } from '@/cli/work/api'
import { formatSleepStatus, sleepError, stateWord, type SleepStatus } from '@/cli/work/sleep-model'
import { useLoaded, WorkPane } from './WorkPane'

/**
 * Sleep tab of the work views (allternit-api `/api/v1/memory/sleep/*`): state,
 * last run, next run and the last report. n runs a cycle now, p pauses or
 * resumes; each asks "y to confirm" first, like the other work actions.
 */
type Pending = 'run' | 'toggle'

export function SleepPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const { data, error, reload } = useLoaded(getSleepStatus, 15_000)
  const [pending, setPending] = useState<Pending>()
  const [notice, setNotice] = useState<string>()
  const [failure, setFailure] = useState<string>()
  const items: SleepStatus[] | undefined = data ? [data] : undefined
  const act = (p: Pending, s: SleepStatus) => {
    const job = p === 'run' ? runSleepNow().then(() => 'Sleep started') : setSleepEnabled(!s.enabled).then(() => (s.enabled ? 'Sleep paused' : 'Sleep resumed'))
    job.then(
      m => { setNotice(m); setFailure(undefined); reload() },
      e => setFailure(sleepError(e)),
    )
  }
  return (
    <WorkPane
      title="Sleep"
      subtitle={data ? stateWord(data) : undefined}
      items={items}
      error={failure ?? (error && sleepError(error))}
      empty="No Sleep status."
      notice={pending ? `${pending === 'run' ? 'Run Sleep now' : data?.enabled ? 'Pause Sleep' : 'Resume Sleep'}? y to confirm, any other key cancels` : notice}
      hint="n run now · p pause/resume"
      onClose={onDone}
      onKey={(input, s) => {
        if (!s) return
        if (pending) {
          const p = pending
          setPending(undefined)
          if (input === 'y') act(p, s)
          return
        }
        if (input === 'n') setPending('run')
        else if (input === 'p') setPending('toggle')
      }}
      renderRow={s => <Text>{formatSleepStatus(s)}</Text>}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const s = await getSleepStatus().then(formatSleepStatus, (e: unknown) => sleepError(e))
    onDone(s, { display: 'system' })
    return null
  }
  return <SleepPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
