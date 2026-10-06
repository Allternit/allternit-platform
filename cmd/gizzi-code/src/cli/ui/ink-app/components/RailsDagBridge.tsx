/**
 * Factory DAG Bridge
 *
 * Polls the Factory DAG view (startRailsDagListener) and mirrors each
 * update into the AppState railsDag slice, which the RailsTaskList panel
 * renders. Null-rendering sibling of RailsInboxBridge; mounts only when
 * Factory peer mode is on.
 */

import { useEffect } from 'react'
import { useSetAppState } from '../state/AppState'
import {
  isRailsPeerMode,
  startRailsDagListener,
} from '@/runtime/gizzi-core/services/railsDag'
import { Log } from '@/shared/util/log'

export function RailsDagBridge(): null {
  const setAppState = useSetAppState()

  useEffect(() => {
    if (!isRailsPeerMode()) return

    Log.Default.info('tui: Factory dag bridge mounted')

    const stop = startRailsDagListener(dto => {
      setAppState(prev => ({
        ...prev,
        railsDag: {
          ...prev.railsDag,
          dags: dto.dags,
          activeWihs: dto.active_wihs,
          updatedAt: Date.now(),
        },
      }))
    })

    return () => {
      stop()
    }
  }, [setAppState])

  return null
}
