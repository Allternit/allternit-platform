/**
 * Gizzi overhaul plan D4 (steps 24+25): one owner per job. Old names keep
 * working as hidden aliases for one release and never show in the slash menu.
 */
import { describe, expect, test } from 'bun:test'
import status from '../../src/cli/ui/ink-app/commands/status/index.ts'
import usage, { usageTab } from '../../src/cli/ui/ink-app/commands/usage/index.ts'
import cost, { costTab } from '../../src/cli/ui/ink-app/commands/cost/index.ts'
import stats from '../../src/cli/ui/ink-app/commands/stats/index.ts'
import sessionInfo from '../../src/cli/ui/ink-app/commands/session-info/index.ts'
import { extraUsage, extraUsageNonInteractive } from '../../src/cli/ui/ink-app/commands/extra-usage/index.ts'
import resume from '../../src/cli/ui/ink-app/commands/resume/index.ts'
import resumeNow from '../../src/cli/ui/ink-app/commands/resume-now/index.ts'
import keepGoing from '../../src/cli/ui/ink-app/commands/keep-going/index.ts'
import plan from '../../src/cli/ui/ink-app/commands/plan/index.ts'
import viewPlan from '../../src/cli/ui/ink-app/commands/view-plan/index.ts'
import goal from '../../src/cli/ui/ink-app/commands/goal.ts'
import mission from '../../src/cli/ui/ink-app/commands/mission/index.ts'
import loops from '../../src/cli/ui/ink-app/commands/loops.ts'
import remote from '../../src/cli/ui/ink-app/commands/remote/index.ts'
import memory from '../../src/cli/ui/ink-app/commands/memory/index.ts'
import { routines } from '../../src/cli/ui/ink-app/commands/work/index.ts'
import { statusTabFromArgs, STATUS_ALIAS_TABS } from '../../src/cli/ui/ink-app/commands/status/tabs.ts'
import { resumeSubcommand } from '../../src/cli/ui/ink-app/commands/resume/subcommand.ts'
import { isPlanView } from '../../src/cli/ui/ink-app/commands/plan/subcommand.ts'
import { HIDDEN_ALIAS_OWNERS, isSlashMenuVisible } from '../../src/cli/ui/ink-app/utils/suggestions/commandSource.ts'
import { findCommand } from '../../src/cli/ui/ink-app/commands.ts'
import { generateCommandSuggestions } from '../../src/cli/ui/ink-app/utils/suggestions/commandSuggestions.ts'
import type { Command } from '../../src/cli/ui/ink-app/types/command.ts'

const hiddenAliases: Command[] = [
  usage, usageTab, cost, costTab, stats, sessionInfo, extraUsage, extraUsageNonInteractive,
  resumeNow, keepGoing, viewPlan, routines,
]
// remote and memory own the step 26 aliases (slash-merge-b.test.ts).
const owners: Command[] = [status, resume, plan, goal, mission, loops, remote, memory]
const registry: Command[] = [...owners, ...hiddenAliases]

async function openedTab(cmd: Command): Promise<unknown> {
  if (cmd.type !== 'local-jsx') throw new Error(`${cmd.name} is not a terminal view`)
  const mod = await cmd.load()
  const el = (await mod.call(() => {}, {} as never, '')) as { props: { defaultTab: unknown } }
  return el.props.defaultTab
}

describe('/status owns status, usage, cost, stats, session-info, extra-usage', () => {
  test('each old name opens its /status tab', async () => {
    expect(await openedTab(usageTab)).toBe('Usage')
    expect(await openedTab(costTab)).toBe('Cost')
    expect(await openedTab(stats)).toBe('Stats')
    expect(await openedTab(sessionInfo)).toBe('Status')
    expect(STATUS_ALIAS_TABS['extra-usage']).toBe('Usage')
  })

  test('/status <tab> opens that tab, anything else opens Status', () => {
    expect(statusTabFromArgs('usage')).toBe('Usage')
    expect(statusTabFromArgs(' Cost ')).toBe('Cost')
    expect(statusTabFromArgs('stats')).toBe('Stats')
    expect(statusTabFromArgs('')).toBe('Status')
    expect(statusTabFromArgs('nonsense')).toBe('Status')
  })

  test('the terminal UI resolves /usage and /cost to the tab, not the text form', () => {
    const terminal = [status, usageTab, usage, costTab, cost]
    expect(findCommand('usage', terminal)).toBe(usageTab)
    expect(findCommand('tokens', terminal)).toBe(usageTab)
    expect(findCommand('cost', terminal)).toBe(costTab)
    // print mode drops local-jsx commands, so the text form still answers there
    const printMode = terminal.filter(c => c.type !== 'local-jsx')
    expect(findCommand('usage', printMode)).toBe(usage)
    expect(findCommand('cost', printMode)).toBe(cost)
  })
})

describe('/resume, /plan, /goal and /loops own their old names', () => {
  test('/resume now and /resume keep-going route to the old commands', () => {
    expect(resumeSubcommand('now')).toBe('now')
    expect(resumeSubcommand(' keep-going ')).toBe('keep-going')
    expect(resumeSubcommand('my old chat')).toBeUndefined()
    expect(resumeSubcommand(undefined)).toBeUndefined()
  })

  test('/plan view shows the plan instead of entering plan mode with "view"', () => {
    expect(isPlanView('view')).toBe(true)
    expect(isPlanView('show')).toBe(true)
    expect(isPlanView('open')).toBe(false)
    expect(isPlanView('build the login page')).toBe(false)
  })

  test('/mission stays a listed command next to /goal', () => {
    expect(isSlashMenuVisible(mission)).toBe(true)
    expect(isSlashMenuVisible(goal)).toBe(true)
  })
})

describe('hidden aliases work but are never offered', () => {
  test('every merged old name is hidden and mapped to a listed owner', () => {
    for (const cmd of hiddenAliases) {
      expect(cmd.isHidden).toBe(true)
      expect(isSlashMenuVisible(cmd)).toBe(false)
      expect(HIDDEN_ALIAS_OWNERS[cmd.name]).toBeDefined()
    }
    for (const cmd of owners) expect(isSlashMenuVisible(cmd)).toBe(true)
    for (const owner of new Set(Object.values(HIDDEN_ALIAS_OWNERS))) {
      expect(owners.some(c => c.name === owner)).toBe(true)
    }
  })

  test('the bare slash menu lists owners only', () => {
    const names = generateCommandSuggestions('/', registry).map(s => s.displayText)
    for (const owner of owners) expect(names.some(n => n.includes(`/${owner.name}`))).toBe(true)
    for (const name of Object.keys(HIDDEN_ALIAS_OWNERS)) {
      expect(names.some(n => n === `/${name}` || n.startsWith(`/${name} `))).toBe(false)
    }
  })

  test('typing an old name exactly still finds it, so it keeps working', () => {
    expect(findCommand('resume-now', registry)).toBe(resumeNow)
    expect(findCommand('view-plan', registry)).toBe(viewPlan)
    expect(findCommand('routines', registry)).toBe(routines)
  })
})
