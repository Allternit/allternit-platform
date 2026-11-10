import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import { SpinnerGlyph } from '../../components/Spinner/SpinnerGlyph'
import { editPromptInEditor } from '../../utils/promptEditor'
import type { LocalJSXCommandCall } from '../../types/command'
import { approveAndSpawn, newMission, planMission } from '@/cli/mission/actions'
import { servingEngine } from '@/cli/mission/orchestrator'
import { badge, label, markdownToPlan, planProblems, planToMarkdown, type Mission, type PlanInput } from '@/cli/mission/state'
import { listMissions, logFile, projectRoot } from '@/cli/mission/store'
import { workerBrain } from '../swarm/swarm'

export interface MissionArgs {
  goal: string
  /** Undefined when not given: the mission runs on the default CLI brain (see workerBrain). */
  brain?: string
  on?: string
  maxWorkers?: number
  maxHours?: number
  maxAttempts?: number
  /** A flag that was given a bad value; the mission isn't started. */
  problem?: string
}

const NUMBER_FLAGS = { '--max-workers': 'maxWorkers', '--max-hours': 'maxHours', '--max-attempts': 'maxAttempts' } as const

/**
 * `<goal> [--brain <brain>] [--on <computer>] [--max-workers N] [--max-hours N] [--max-attempts N]`
 * → its parts: the same flags as `gizzi mission start`, as `--flag value` or
 * `--flag=value`, anywhere in the line. Quotes around the goal are optional.
 */
export function parseMissionArgs(raw: string): MissionArgs {
  const words = raw.trim().split(/\s+/).filter(Boolean)
  const out: MissionArgs = { goal: '' }
  const rest: string[] = []
  for (let i = 0; i < words.length; i++) {
    const w = words[i]!
    const eq = w.startsWith('--') ? w.indexOf('=') : -1
    const flag = eq > 0 ? w.slice(0, eq) : w
    const known = flag === '--brain' || flag === '-m' || flag === '--on' || flag in NUMBER_FLAGS
    if (!known) {
      rest.push(w)
      continue
    }
    const value = eq > 0 ? w.slice(eq + 1) : words[++i]
    if (!value) {
      out.problem = `${flag} needs a value`
      continue
    }
    if (flag === '--brain' || flag === '-m') out.brain = value
    else if (flag === '--on') out.on = value
    else {
      const n = Number(value)
      const whole = flag !== '--max-hours'
      // --max-attempts 0 means "no fix rounds", like the CLI; the others must be positive.
      const min = flag === '--max-attempts' ? 0 : whole ? 1 : Number.MIN_VALUE
      if (!Number.isFinite(n) || n < min || (whole && !Number.isInteger(n))) out.problem = `${flag} takes a ${whole ? 'whole number' : 'number of hours'}, not "${value}"`
      else out[NUMBER_FLAGS[flag as keyof typeof NUMBER_FLAGS]] = n
    }
  }
  out.goal = rest.join(' ').replace(/^["']|["']$/g, '').trim()
  return out
}

const USAGE = 'Usage: /mission <goal> [--brain <provider/model>] [--on <computer>] [--max-workers 3] [--max-hours N] [--max-attempts 2]. Plans the goal as milestones, you approve, it runs in the background. Check it with `gizzi mission status`.'

function statusText(cwd: string): string {
  let root: string
  try {
    root = projectRoot(cwd)
  } catch {
    return USAGE
  }
  const ms = listMissions(root)
  const m = ms.find(x => x.status === 'running' || x.status === 'needs_you') ?? ms[0]
  if (!m) return `No mission in this project.\n${USAGE}`
  const lines = [`${label(m)}  (${m.id}, ${m.status})`, `Goal: ${m.goal}`]
  m.milestones.forEach((x, i) => lines.push(`  ${i === m.current && (m.status === 'running' || m.status === 'needs_you') ? '>' : ' '} ${x.id}: ${x.title}  [${x.status}]`))
  if (m.note) lines.push(`Note: ${m.note}`)
  lines.push(`Log: ${logFile(m)}`)
  return lines.join('\n')
}

type Phase =
  | { kind: 'planning' }
  | { kind: 'review'; mission: Mission; plan: PlanInput; problems: string[]; notice?: string }
  | { kind: 'error'; text: string }
  | { kind: 'working' }

function MissionPlanner({ mission0, engine, onDone }: { mission0: Mission; engine: string | undefined; onDone: (text: string) => void }) {
  const [phase, setPhase] = React.useState<Phase>({ kind: 'planning' })
  const [frame, setFrame] = React.useState(0)
  const missionRef = React.useRef(mission0)
  const alive = React.useRef(true)

  React.useEffect(() => {
    const t = setInterval(() => setFrame(f => f + 1), 120)
    return () => clearInterval(t)
  }, [])
  React.useEffect(() => () => { alive.current = false }, [])

  const plan = React.useCallback(async (retry?: string[]) => {
    setPhase({ kind: 'planning' })
    try {
      const out = await planMission(missionRef.current, { engine, retry })
      missionRef.current = out.mission
      if (!alive.current) return
      if (out.error) setPhase({ kind: 'error', text: out.error })
      else if (!out.plan) setPhase({ kind: 'error', text: out.problems.join('; ') || 'no plan came back' })
      else setPhase({ kind: 'review', mission: out.mission, plan: out.plan, problems: out.problems })
    } catch (e) {
      if (alive.current) setPhase({ kind: 'error', text: e instanceof Error ? e.message : String(e) })
    }
  }, [engine])

  React.useEffect(() => { void plan() }, [plan])

  useInput((input, key) => {
    if (phase.kind === 'working') return
    if (key.escape || input === 'q') {
      onDone(`Mission ${missionRef.current.id} not started (cancelled). Re-plan later with \`gizzi mission start\`.`)
      return
    }
    if (phase.kind === 'error') {
      if (input === 'r') void plan()
      return
    }
    if (phase.kind !== 'review') return
    if (input === 'r') {
      void plan(phase.problems)
    } else if (input === 'e') {
      const edited = editPromptInEditor(planToMarkdown(missionRef.current.goal, phase.plan))
      if (edited.error) {
        setPhase({ ...phase, notice: edited.error })
      } else if (edited.content !== null) {
        const next = markdownToPlan(edited.content)
        setPhase({ ...phase, plan: next, problems: planProblems(next), notice: 'Plan edited.' })
      }
    } else if (key.return) {
      if (phase.problems.length) {
        setPhase({ ...phase, notice: 'Fix the problems first (e to edit, r to re-plan).' })
        return
      }
      setPhase({ kind: 'working' })
      void (async () => {
        try {
          const r = await Promise.resolve(approveAndSpawn(missionRef.current, { plan: phase.plan }))
          if (r.error) onDone(`Couldn't start mission ${r.mission.id}: ${r.error}`)
          else onDone(`Mission ${r.mission.id} started (${badge(r.mission)}). Check it with \`gizzi mission status\`. Log: ${r.log}`)
        } catch (e) {
          onDone(`Couldn't start the mission: ${e instanceof Error ? e.message : String(e)}`)
        }
      })()
    }
  })

  if (phase.kind === 'planning' || phase.kind === 'working') {
    return (
      <Box>
        <SpinnerGlyph frame={frame} messageColor="gizzi" />
        <Text> {phase.kind === 'planning' ? 'Planning…' : 'Starting the mission…'}</Text>
      </Box>
    )
  }
  if (phase.kind === 'error') {
    return (
      <Box flexDirection="column">
        <Text color="error">Planning failed: {phase.text}</Text>
        <Text dimColor>r re-plan · Esc cancel</Text>
      </Box>
    )
  }
  return (
    <Box flexDirection="column">
      <Text bold>Mission: {phase.mission.goal}</Text>
      {phase.plan.milestones.map((m, i) => (
        <Box key={i} flexDirection="column" marginTop={1}>
          <Text bold>M{i + 1}: {m.title}</Text>
          {m.tracks.map((t, j) => <Text key={j}>  - {t.title}: {t.task}</Text>)}
          <Text dimColor>  validate: {m.validate}</Text>
        </Box>
      ))}
      {phase.problems.length > 0 && (
        <Box flexDirection="column" marginTop={1}>
          <Text color="warning">Problems with this plan:</Text>
          {phase.problems.map((p, i) => <Text key={i} color="warning">  - {p}</Text>)}
        </Box>
      )}
      {phase.notice && <Text color="warning">{phase.notice}</Text>}
      <Box marginTop={1}>
        <Text dimColor>Enter approve and start · e edit · r re-plan · Esc cancel</Text>
      </Box>
    </Box>
  )
}

/**
 * /mission <goal> — plan, review, approve, spawn the background runner.
 * With no goal it shows the project's mission status.
 */
export const call: LocalJSXCommandCall = async (onDone, context, args) => {
  const parsed = parseMissionArgs(args ?? '')
  const { goal, brain } = parsed
  const cwd = process.cwd()
  if (context.options?.isNonInteractiveSession) {
    onDone('Use `gizzi mission start "<goal>"` outside the TUI.', { display: 'system' })
    return null
  }
  if (parsed.problem) {
    onDone(`${parsed.problem}.\n${USAGE}`, { display: 'system' })
    return null
  }
  if (!goal) {
    onDone(statusText(cwd), { display: 'system' })
    return null
  }
  const picked = await workerBrain(brain)
  if ('error' in picked) {
    onDone(`Couldn't start the mission: ${picked.error}`, { display: 'system' })
    return null
  }
  const created = newMission({ goal, cwd, brain: picked.brain, on: parsed.on, maxWorkers: parsed.maxWorkers, maxHours: parsed.maxHours, maxAttempts: parsed.maxAttempts })
  if ('error' in created) {
    onDone(`Couldn't start the mission: ${created.error}`, { display: 'system' })
    return null
  }
  const engine = await servingEngine()
  return <MissionPlanner mission0={created.mission} engine={engine} onDone={text => onDone(text, { display: 'system' })} />
}
