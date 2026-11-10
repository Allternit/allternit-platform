import type { LocalJSXCommandCall } from '../../types/command.js'
import { startSwarm } from '@/cli/factory/swarm'
import { defaultCliBrain } from '@/runtime/providers/default-brain'
import { retiredModelId } from '@/runtime/providers/discovery/subprocess'

export const NO_CLI_BRAIN = 'No CLI brain to run workers: sign in to a CLI brain (gizzi connect) or pass --brain <provider/model>'

/**
 * The brain workers run on: an explicit `--brain` (a retired model id mapped
 * to its current one), else the user's default CLI brain resolved now.
 * `{ error }` when neither exists, before any worker starts.
 */
export async function workerBrain(explicit: string | undefined): Promise<{ brain: string } | { error: string }> {
  const given = explicit?.trim()
  if (given) {
    const slash = given.indexOf('/')
    if (slash <= 0) return { brain: given }
    const providerID = given.slice(0, slash)
    return { brain: `${providerID}/${retiredModelId(providerID, given.slice(slash + 1))}` }
  }
  const brain = await defaultCliBrain().catch(() => null)
  return brain ? { brain } : { error: NO_CLI_BRAIN }
}

/**
 * `<task> [-n|--workers N] [--brain <brain>]` → its parts. Quotes around the
 * task are optional. `brain` is undefined when not given (see workerBrain).
 */
export function parseSwarmArgs(raw: string): { task: string; workers: number; brain?: string } {
  const words = raw.trim().split(/\s+/).filter(Boolean)
  let workers = 2
  let brain: string | undefined
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
  const { task, workers, brain: given } = parseSwarmArgs(args ?? '')
  if (context.options?.isNonInteractiveSession) {
    onDone('Use `gizzi swarm "<task>" --workers 2 [--brain <provider/model>]` outside the TUI.', { display: 'system' })
    return null
  }
  if (!task) {
    context.setAppState(prev => ({ ...prev, screen: 'board' }))
    onDone('Usage: /swarm <task> [-n 2] [--brain <provider/model>] (default: your default CLI brain). Opened the board.', { display: 'system' })
    return null
  }
  const picked = await workerBrain(given)
  if ('error' in picked) {
    onDone(`Couldn't start the swarm: ${picked.error}`, { display: 'system' })
    return null
  }
  const brain = picked.brain
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
