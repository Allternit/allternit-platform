import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../commands.js'
import { getDirectConnectServerUrl, getSessionId } from '../bootstrap/state.js'

type GoalAction = 'pause' | 'resume' | 'complete' | 'block' | 'status'
let activeGoalId: string | undefined

interface GoalRecord {
  id: string
  agent_id?: string | null
  objective: string
  state: string
  progress: number
  time_updated: number
  queue_position?: number | null
  budget?: { turnBudget: number | null; tokenBudget: number | null; wallClockBudgetMs: number | null }
  usage?: { turnsUsed: number; tokensUsed: number; wallClockMs: number }
  terminal_reason?: string | null
}

function goalBaseUrl(): string {
  return getDirectConnectServerUrl() ?? 'http://localhost:4096'
}

async function resolveActiveGoal(sessionID?: string): Promise<GoalRecord | undefined> {
  const response = await fetch(new URL('/v1/automations/goals', goalBaseUrl()), { headers: { Accept: 'application/json' } })
  if (!response.ok) return undefined
  const goals = await response.json() as GoalRecord[]
  const candidates = goals
    .filter((goal) => (
      (goal.id === activeGoalId && (sessionID === undefined || goal.agent_id === sessionID))
      || ((sessionID === undefined || goal.agent_id === sessionID) && !['completed', 'cancelled'].includes(goal.state))
    ))
    .sort((a, b) => b.time_updated - a.time_updated)
  return candidates.find((goal) => goal.id === activeGoalId)
    ?? candidates.find((goal) => goal.state === 'in_progress')
    ?? candidates[0]
}

async function handleGoalAction(action: GoalAction, sessionID?: string): Promise<string> {
  const goal = await resolveActiveGoal(sessionID)
  if (!goal) return 'No active persistent goal was found.'
  activeGoalId = goal.id
  if (action === 'status') {
    const usage = goal.usage
      ? `${goal.usage.turnsUsed} turns, ${goal.usage.tokensUsed} tokens, ${Math.round(goal.usage.wallClockMs / 1000)}s`
      : 'usage unavailable'
    const budget = goal.budget
      ? `turns=${goal.budget.turnBudget ?? '∞'}, tokens=${goal.budget.tokenBudget ?? '∞'}, time=${goal.budget.wallClockBudgetMs ?? '∞'}ms`
      : 'unbounded'
    return `Goal ${goal.id}: ${goal.state}, ${goal.progress}% — ${goal.objective}\nUsage: ${usage}\nBudget: ${budget}${goal.terminal_reason ? `\nReason: ${goal.terminal_reason}` : ''}`
  }
  const endpoint = action === 'resume' ? 'run' : action
  const response = await fetch(new URL(`/v1/automations/goals/${encodeURIComponent(goal.id)}/${endpoint}`, goalBaseUrl()), { method: 'POST', headers: { Accept: 'application/json' } })
  if (!response.ok) return `Goal ${action} failed (${response.status} ${response.statusText}).`
  const nextState = action === 'resume' ? 'in progress' : action === 'complete' ? 'completed' : action === 'pause' ? 'paused' : 'blocked'
  return `Goal ${goal.id} is now ${nextState}. Do not continue autonomous work unless the goal is in progress.`
}

const GOAL_PROMPT = (id: string, args: string, persisted: boolean) => `
You are in Goal Mode (Goal ID: ${id}). Your high-level objective is:
"${args}"

Durable goal record: ${persisted ? 'connected' : 'unavailable; continue in this session and report that persistence is degraded'}.

Follow this strict loop engineering process:
1. Define a clear plan with milestones. Write the plan to the console or file.
2. Break down the current milestone into concrete tasks.
3. Execute the tasks one by one.
4. After each milestone, run only the validations authorized for this workspace and record their real output.
5. If a test or validation fails, analyze the failure, modify the files to fix the issue, and rerun validation. Do not stop until all validations pass.
6. Refine and update the plan if you discover new constraints or bugs.
7. Complete the goal only when the requested outcome is achieved and the available evidence supports completion. Otherwise report it blocked or incomplete.

Begin working on the goal immediately by creating a plan and starting the first task.
`

const goal: Command = {
  type: 'prompt',
  name: 'goal',
  argumentHint: '<objective|status|pause|resume|clear>',
  // Owns /mission (overhaul plan D4); /mission stays listed because it shipped 11-10.
  description: 'Run a task continuously in an autonomous goal-directed loop until complete (/mission plans it as milestones first)',
  progressMessage: 'executing goal loop',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args, context): Promise<ContentBlockParam[]> {
    if (!args.trim()) return [{ type: 'text', text: 'Provide an objective: /goal <objective>' }]
    const normalized = args.trim().toLowerCase()
    if (['pause', 'resume', 'complete', 'block', 'status'].includes(normalized)) {
      try {
        return [{ type: 'text', text: await handleGoalAction(normalized as GoalAction, getSessionId()) }]
      } catch {
        return [{ type: 'text', text: `Unable to ${normalized} the persistent goal because the goal service is unavailable.` }]
      }
    }
    const queueMatch = /^queue\s+(.+)$/is.exec(args.trim())
    const replaceMatch = /^replace\s+(.+)$/is.exec(args.trim())
    const objective = queueMatch?.[1]?.trim() ?? replaceMatch?.[1]?.trim() ?? args.trim()
    let id = `goal-${Date.now()}`
    let persisted = false
    try {
      const response = await fetch(new URL('/v1/automations/goals', goalBaseUrl()), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          id,
          agent_id: getSessionId(),
          objective,
          enqueue: Boolean(queueMatch),
          replace: Boolean(replaceMatch),
        }),
      })
      if (response.ok) {
        const created = await response.json() as { id?: string; state?: string }
        id = created.id ?? id
        activeGoalId = id
        persisted = true
        if (created.state === 'queued') {
          return [{ type: 'text', text: `Goal ${id} was queued and will become active only after the current goal completes.` }]
        }
      } else if (response.status === 409) {
        const conflict = await response.json().catch(() => ({})) as { error?: string }
        return [{
          type: 'text',
          text: `${conflict.error ?? 'A goal is already active.'} Use /goal queue <objective> or /goal replace <objective>.`,
        }]
      }
    } catch {
      // The prompt explicitly reports degraded persistence.
    }
    return [{ type: 'text', text: GOAL_PROMPT(id, objective, persisted) }]
  },
}

export default goal
