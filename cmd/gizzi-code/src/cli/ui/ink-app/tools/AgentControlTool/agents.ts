/**
 * The terminal's view of "my agents" for wait_agents and agent_control
 * (Primitives Wave B, lane L6). Ink keeps its background agents as
 * local_agent tasks in AppState.tasks (task id = agent id) and their names in
 * AppState.agentNameRegistry, so these helpers read that state and shape it
 * like the runtime contract (runtime/primitives/wave-b-types.ts).
 *
 * Ink names are flat (one level), so a name path is always "/root/<name>".
 * Ink has no "interrupted" task status: an interrupt kills the agent's turn
 * and keeps its transcript on disk, and this module remembers which killed
 * agents were interrupted (not closed) so they report as interrupted.
 */
import type { AppState } from '../../state/AppState.js'
import {
  isPanelAgentTask,
  type LocalAgentTaskState,
} from '../../tasks/LocalAgentTask/LocalAgentTask.js'
import {
  TASK_NAME_ROOT,
  type AgentRef,
  type AgentRunStatus,
} from '@/runtime/primitives/wave-b-types'

const SUMMARY_CHARS = 1_500

/** Agents agent_control interrupted in this process (killed, but meant to resume). */
const interrupted = new Set<string>()

export function markInterrupted(id: string): void {
  interrupted.add(id)
}

export function clearInterrupted(id: string): void {
  interrupted.delete(id)
}

/** Every background agent of this terminal session (the main-session pseudo-task is left out). */
export function agentTasks(state: AppState): LocalAgentTaskState[] {
  return Object.values(state.tasks ?? {}).filter(isPanelAgentTask)
}

function nameOf(state: AppState, id: string): string | null {
  for (const [name, agentId] of state.agentNameRegistry ?? new Map()) {
    if (agentId === id) return name
  }
  return null
}

export function agentRef(state: AppState, id: string): AgentRef {
  const name = nameOf(state, id)
  return { id, name, path: name ? `${TASK_NAME_ROOT}/${name}` : null }
}

export function agentStatus(task: LocalAgentTaskState): AgentRunStatus {
  switch (task.status) {
    case 'pending':
      return 'queued'
    case 'running':
      return 'running'
    case 'completed':
      return 'completed'
    case 'failed':
      return 'failed'
    default:
      return interrupted.has(task.id) ? 'interrupted' : 'cancelled'
  }
}

export function isAgentRunning(task: LocalAgentTaskState | undefined): boolean {
  return task?.status === 'running' || task?.status === 'pending'
}

/** A task id, a name, or a name path ("/root/tests") to the agent's task id. */
export function resolveAgent(state: AppState, target: string): string {
  const raw = target.trim()
  const tasks = state.tasks ?? {}
  if (isPanelAgentTask(tasks[raw])) return raw
  const name = raw.startsWith(`${TASK_NAME_ROOT}/`)
    ? raw.slice(TASK_NAME_ROOT.length + 1)
    : raw.replace(/^\//, '')
  const id = state.agentNameRegistry?.get(name)
  if (id && isPanelAgentTask(tasks[id])) return id
  throw new Error(
    `No agent "${target}" in this session. Use agent_control with action "list" to see your agents.`,
  )
}

function clip(text: string): string {
  const flat = text.trim()
  return flat.length <= SUMMARY_CHARS ? flat : `${flat.slice(0, SUMMARY_CHARS - 1)}…`
}

/** The agent's final answer (shortened), its error, or what it was doing when it stopped. */
export function agentSummary(task: LocalAgentTaskState): string {
  if (task.status === 'failed') return clip(task.error || 'The agent failed.')
  const answer = (task.result?.content ?? [])
    .map(block => block?.text ?? '')
    .join('\n')
  if (answer.trim()) return clip(answer)
  if (task.status === 'killed') {
    return interrupted.has(task.id)
      ? 'Interrupted. Its history is kept; resume it or send a followup to continue.'
      : 'Stopped before it finished.'
  }
  return clip(task.progress?.summary ?? '')
}

/** What a running agent is doing right now, when known. */
export function agentActivity(task: LocalAgentTaskState): string | undefined {
  return (
    task.progress?.lastActivity?.activityDescription ??
    task.progress?.summary ??
    undefined
  )
}
