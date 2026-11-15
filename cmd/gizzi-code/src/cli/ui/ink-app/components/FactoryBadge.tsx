/**
 * Footer badge while Factory bots run: "⚙ 3 bots · 1 waiting". Hidden when
 * nothing is working or waiting. Fed by one shared subscription to the
 * engine's event stream (cli/factory/live.ts), not by polling; /board or
 * ctrl+x f opens the full view.
 */
import * as React from 'react'
import { acquireSharedLive, type LiveView } from '@/cli/factory/live'
import { badgeText, buildBoard, allRows } from '@/cli/factory/board'
import { drainReady, observeSwarm, openSwarms } from '@/cli/factory/swarm'

export function useFactoryBadge(): { text: string; waiting: boolean } | null {
  const [view, setView] = React.useState<LiveView | null>(null)
  React.useEffect(() => {
    const { live, release } = acquireSharedLive()
    const off = live.subscribe(setView)
    return () => {
      off()
      release()
    }
  }, [])
  // Deliver queued tasks/steers to swarm workers whose panes are free, even when no board is open.
  React.useEffect(() => {
    if (view?.connected) for (const s of openSwarms()) drainReady(s, view.agents)
  }, [view])
  return React.useMemo(() => {
    if (!view?.connected) return null
    const swarms = openSwarms().map(s => observeSwarm(s, view.agents))
    const rows = allRows(buildBoard({ agents: view.agents, swarms, lastLine: {}, since: view.since }))
    const text = badgeText(rows)
    return text ? { text, waiting: rows.some(r => r.state === 'waiting') } : null
  }, [view])
}
