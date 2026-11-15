/**
 * Footer badge while a mission runs: "◆ M2/5" (plus " · needs you" when it is
 * waiting on the person). Polls the mission records every 3s; hidden when there
 * is no mission or no `.allternit/missions` directory. Never throws.
 */
import * as React from 'react'
import fs from 'node:fs'
import { badge, type Mission } from '@/cli/mission/state'
import { listMissions, missionsDir, projectRoot } from '@/cli/mission/store'

/** Badge text for a mission, or null unless it is running or needs the person. */
export function missionBadgeText(m: Mission | null | undefined): string | null {
  if (!m) return null
  if (m.status === 'running') return badge(m)
  if (m.status === 'needs_you') return `${badge(m)} · needs you`
  return null
}

function readBadge(root: string): string | null {
  try {
    const dir = missionsDir(root)
    if (!fs.existsSync(dir)) return null
    const ms = listMissions(root)
    return missionBadgeText(ms.find(m => m.status === 'running') ?? ms.find(m => m.status === 'needs_you'))
  } catch {
    return null
  }
}

export function useMissionBadge(cwd: string): { text: string; needsYou: boolean } | null {
  const [text, setText] = React.useState<string | null>(null)
  React.useEffect(() => {
    let root: string
    try {
      root = projectRoot(cwd)
    } catch {
      return
    }
    const tick = () => setText(prev => {
      const next = readBadge(root)
      return next === prev ? prev : next
    })
    tick()
    const t = setInterval(tick, 3000)
    return () => clearInterval(t)
  }, [cwd])
  return text ? { text, needsYou: text.includes('needs you') } : null
}
