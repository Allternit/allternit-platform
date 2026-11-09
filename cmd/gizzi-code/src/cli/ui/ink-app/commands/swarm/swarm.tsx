import type { LocalJSXCommandCall } from '../../types/command.js'
import { startSwarm } from '@/cli/factory/swarm'

export const DEFAULT_BRAIN = 'claude-cli/claude-sonnet-5'

/** `<task> [-n|--workers N] [--brain <brain>]` → its parts. Quotes around the task are optional. */
export function parseSwarmArgs(raw: string): { task: string; workers: number; brain: string } {
  const words = raw.trim().split(/\s+/).filter(Boolean)
  let workers = 2
  let brain = DEFAULT_BRAIN
  const rest: string[] = []
  for (let i = 0; i < words.length; i++) {
    const w = words[i]!
    if ((w === '-n' || w === '--workers') && words[i + 1]) workers = Number(words[++i])
    else if (w.startsWith('--workers=')) workers = Number(w.slice(10))
    else if (w === '--brain' && words[i + 1]) brain = words[++i]!
    else if (w.startsWith('--brain=')) brain = w.slice(8)
    else rest.push(w)
  }
  return { task: rest.join(' ').replace(/^["']|["']$/g, '').trim(), workers, brain }
}

/**
 * /swarm <task> — start a real swarm through the Factory engine and open the
 * board at once; workers appear on it as their panes start. With no task it
 * just opens the board.
 */
export const call: LocalJSXCommandCall = async (onDone, context, args) => {
  const { task, workers, brain } = parseSwarmArgs(args ?? '')
  if (context.options?.isNonInteractiveSession) {
    onDone('Use `gizzi swarm "<task>" --workers 2 --brain claude-cli/<model>` outside the TUI.', { display: 'system' })
    return null
  }
  if (!task) {
    context.setAppState(prev => ({ ...prev, screen: 'board' }))
    onDone('Usage: /swarm <task> [-n 2] [--brain claude-cli/claude-sonnet-5]. Opened the board.', { display: 'system' })
    return null
  }
  const started = startSwarm({ task, workers, brain, cwd: process.cwd() })
  // Fail fast on usage errors (no git repo, bad brain) before switching screens.
  const early = await Promise.race([started.then(() => null, (e: unknown) => e), new Promise(r => setTimeout(() => r(null), 300))])
  if (early) {
    const e = early as { fact?: string; action?: string | null; message?: string }
    onDone(`Couldn't start the swarm: ${e.fact ?? e.message}${e.action ? ` — ${e.action}` : ''}`, { display: 'system' })
    return null
  }
  started.catch(() => {
    // Per-worker failures are on the board; a late whole-swarm failure has its reason in the record.
  })
  context.setAppState(prev => ({ ...prev, screen: 'board' }))
  onDone(`Starting a ${workers}-worker swarm (${brain}): ${task}`, { display: 'system' })
  return null
}
