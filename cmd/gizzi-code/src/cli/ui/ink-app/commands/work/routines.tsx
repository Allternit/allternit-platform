import * as React from 'react'
import { useState } from 'react'
import { Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { listRoutines, setRoutinePaused, type Routine } from '@/cli/work/api'
import { relTime, useLoaded, WorkPane } from './WorkPane'

/**
 * /routines — the account's routines (Desktop's Automation Tasks,
 * allternit-api `/api/v1/automation/routines`) sorted by next run; p pauses
 * or resumes the selected one the way Desktop's toggle does.
 */
function schedule(r: Routine): string {
  return [r.schedule_type, r.schedule_expression].filter(Boolean).join(' ')
}

export function RoutinesPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const { data, error, reload } = useLoaded(listRoutines, 30_000)
  const [notice, setNotice] = useState<string>()
  const [failure, setFailure] = useState<string>()
  return (
    <WorkPane
      title="Routines"
      subtitle={data ? `${data.filter(r => r.status === 'active').length} active` : undefined}
      items={data}
      error={failure ?? (error && `Couldn't load routines: ${error}`)}
      empty="No routines yet. Make one in Allternit Desktop (Automation Tasks) or from a bot."
      notice={notice}
      hint="↑/↓ choose · p pause/resume"
      onClose={onDone}
      onKey={(input, r) => {
        if (input !== 'p' || !r) return
        const pause = r.status !== 'paused'
        setRoutinePaused(r.id, pause).then(
          () => {
            setNotice(`${pause ? 'Paused' : 'Resumed'} ${r.name}`)
            setFailure(undefined)
            reload()
          },
          e => setFailure(e instanceof Error ? e.message : String(e)),
        )
      }}
      renderRow={r => (
        <Text wrap="truncate-end">
          <Text color={r.status === 'active' ? 'success' : 'inactive'}>{r.status === 'active' ? '▶' : '⏸'}</Text>{' '}
          {r.name}
          <Text dimColor>
            {' '}
            · next {r.status === 'active' ? relTime(r.next_run_at) : 'paused'} · last {relTime(r.last_run_at)} · {schedule(r)}
          </Text>
        </Text>
      )}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const list = await listRoutines().catch((e: Error) => e)
    onDone(
      list instanceof Error
        ? `Couldn't load routines: ${list.message}`
        : list.map(r => `${r.status} · ${r.name} · next ${r.status === 'active' ? relTime(r.next_run_at) : '—'} · ${schedule(r)}`).join('\n') ||
            'No routines yet.',
      { display: 'system' },
    )
    return null
  }
  return <RoutinesPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
