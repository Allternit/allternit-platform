import * as React from 'react'
import { Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { getFloorState } from '@/cli/work/pods'
import { NO_PODS, podDetail, podError, podRow, podSummary, type FloorState, type Pod } from '@/cli/work/pods-model'
import { useLoaded, WorkPane } from './WorkPane'

/**
 * Pods (Desktop's Office floor grouping): Chief, one pod per active project,
 * Team. Detail shows the selected pod's bots and state; enter opens the
 * Factory floor, the team view this terminal has.
 */
export function PodsPane({ onDone, onOpenTeam }: { onDone: () => void; onOpenTeam?: () => void }): React.ReactNode {
  const { data, error } = useLoaded<FloorState>(getFloorState, 15_000)
  const pods: Pod[] | undefined = data ? (data.pods ?? []) : undefined
  return (
    <WorkPane
      title="Pods"
      subtitle={data && pods ? `${pods.length} ${pods.length === 1 ? 'pod' : 'pods'}` : undefined}
      items={pods}
      error={error ? podError(error) : undefined}
      empty={NO_PODS}
      hint={onOpenTeam ? 'enter open team view' : '↑/↓ move'}
      onClose={onDone}
      onKey={(input, pod) => { if (input === '\r' && pod && onOpenTeam) onOpenTeam() }}
      renderRow={(p, sel) => <Text bold={sel}>{data ? podRow(data, p) : p.name}</Text>}
      detail={p => (
        <>
          <Text dimColor>{data ? podSummary(data, p) : ''}</Text>
          {data ? podDetail(data, p).map((l, i) => <Text key={i}>  {l}</Text>) : null}
        </>
      )}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    try {
      const s = await getFloorState()
      const pods = s.pods ?? []
      onDone(pods.length ? pods.map(p => podRow(s, p)).join('\n') : NO_PODS, { display: 'system' })
    } catch (e) {
      onDone(podError(e), { display: 'system' })
    }
    return null
  }
  return (
    <PodsPane
      onDone={() => onDone(undefined, { display: 'skip' })}
      onOpenTeam={() => {
        context.setAppState(prev => ({ ...prev, screen: 'factory' }))
        onDone(undefined, { display: 'skip' })
      }}
    />
  )
}
