import { describe, expect, test } from 'bun:test'
import alwaysApprove from '../../src/cli/ui/ink-app/commands/always-approve/index.ts'
import agents from '../../src/cli/ui/ink-app/commands/agents/index.ts'
import cd from '../../src/cli/ui/ink-app/commands/cd/index.ts'
import config from '../../src/cli/ui/ink-app/commands/config/index.ts'
import dash from '../../src/cli/ui/ink-app/commands/dash/index.ts'
import dashboard from '../../src/cli/ui/ink-app/commands/dashboard/index.ts'
import sessions from '../../src/cli/ui/ink-app/commands/sessions/index.ts'
import help from '../../src/cli/ui/ink-app/commands/help/index.ts'
import model from '../../src/cli/ui/ink-app/commands/model/index.ts'
import multiline from '../../src/cli/ui/ink-app/commands/multiline/index.ts'
import queue from '../../src/cli/ui/ink-app/commands/queue/index.ts'
import recap from '../../src/cli/ui/ink-app/commands/recap/index.ts'
import rewind from '../../src/cli/ui/ink-app/commands/rewind/index.ts'
import sessionInfo from '../../src/cli/ui/ink-app/commands/session-info/index.ts'
import status from '../../src/cli/ui/ink-app/commands/status/index.ts'
import theme from '../../src/cli/ui/ink-app/commands/theme/index.ts'
import transcript from '../../src/cli/ui/ink-app/commands/transcript/index.ts'
import viewPlan from '../../src/cli/ui/ink-app/commands/view-plan/index.ts'
import {
  compareCommandsForSlashMenu,
  getCommandArgumentHint,
  getCommandCategory,
  getCommandSourceTag,
} from '../../src/cli/ui/ink-app/utils/suggestions/commandSource.ts'
import type { Command } from '../../src/cli/ui/ink-app/types/command.ts'

function localCommand(name: string, extra: Partial<Command> = {}): Command {
  return {
    type: 'local',
    name,
    description: `${name} description`,
    supportsNonInteractive: true,
    load: async () => ({ call: async () => ({ type: 'text', value: '' }) }),
    ...extra,
  } as Command
}

describe('Grok-style slash menu presentation', () => {
  test('built-ins are tagged built-in', () => {
    expect(getCommandSourceTag(localCommand('compact'))).toBe('built-in')
  })

  test('project skills are tagged skill · local', () => {
    const cmd = {
      type: 'prompt',
      name: 'commit',
      description: 'Commit',
      source: 'projectSettings',
      progressMessage: 'running',
      contentLength: 10,
      getPromptForCommand: async () => [],
    } as Command
    expect(getCommandSourceTag(cmd)).toBe('skill · local')
  })

  test('user skills are tagged skill · user', () => {
    const cmd = {
      type: 'prompt',
      name: 'commit',
      description: 'Commit',
      source: 'userSettings',
      progressMessage: 'running',
      contentLength: 10,
      getPromptForCommand: async () => [],
    } as Command
    expect(getCommandSourceTag(cmd)).toBe('skill · user')
  })

  test('plugin skills are tagged skill · <plugin>', () => {
    const cmd = {
      type: 'prompt',
      name: 'login',
      description: 'Login skill',
      source: 'plugin',
      pluginInfo: { pluginManifest: { name: 'acme' }, repository: 'acme' },
      progressMessage: 'running',
      contentLength: 10,
      getPromptForCommand: async () => [],
    } as Command
    expect(getCommandSourceTag(cmd)).toBe('skill · acme')
  })

  test('argument hints come from argumentHint or prompt argNames', () => {
    expect(
      getCommandArgumentHint(localCommand('compact', { argumentHint: '[context]' })),
    ).toBe('[context]')
    const promptCmd = {
      type: 'prompt',
      name: 'commit',
      description: 'Commit',
      source: 'userSettings',
      argNames: ['message'],
      progressMessage: 'running',
      contentLength: 10,
      getPromptForCommand: async () => [],
    } as Command
    expect(getCommandArgumentHint(promptCmd)).toBe('<message>')
  })

  test('empty slash menu sorts Session commands before Settings', () => {
    const configCmd = localCommand('theme')
    const sessionCmd = localCommand('compact')
    expect(getCommandCategory(sessionCmd)).toBe('Session')
    expect(getCommandCategory(configCmd)).toBe('Settings')
    expect(compareCommandsForSlashMenu(sessionCmd, configCmd)).toBeLessThan(0)
  })
})

describe('Grok muscle-memory aliases', () => {
  test('session and mode aliases match Grok names', () => {
    expect(rewind.aliases).toContain('undo')
    // /sessions is its own command now; /dash and /dashboard are hidden
    // names that open it. No other command may claim 'sessions' — regression
    // guard for the alias collision that hijacked /dashboard resolution.
    expect(sessions.name).toBe('sessions')
    expect(dash.name).toBe('dash')
    expect(dashboard.name).toBe('dashboard')
    expect(dashboard.aliases ?? []).not.toContain('sessions')
    expect(dashboard.aliases).toContain('agents-dashboard')
    expect((dash as Command).isHidden).toBe(true)
    expect((dashboard as Command).isHidden).toBe(true)
    expect((dash as Command).aliases ?? []).not.toContain('dashboard')
    expect((dash as Command).aliases ?? []).not.toContain('sessions')
    // session-info is now its own hidden alias command (opens the Status tab).
    expect(status.aliases).not.toContain('session-info')
    expect(status.aliases).toContain('info')
    expect(model.aliases).toContain('m')
    expect(theme.aliases).toContain('t')
    expect(config.aliases).toContain('settings')
    expect(config.aliases).toContain('preferences')
    expect(help.aliases).toContain('docs')
    expect(help.aliases).toContain('tutorial')
    expect(agents.aliases).toContain('config-agents')
    expect(agents.aliases).toContain('personas')
    expect(viewPlan.aliases).toContain('show-plan')
    expect(viewPlan.aliases).toContain('plan-view')
  })

  test('new Grok-parity commands are registered by name', () => {
    expect(alwaysApprove.name).toBe('always-approve')
    expect(viewPlan.name).toBe('view-plan')
  })

  test('Grok session utility commands are registered with expected types', () => {
    expect(sessionInfo.name).toBe('session-info')
    expect(sessionInfo.type).toBe('local-jsx')
    expect(sessionInfo.immediate).toBe(true)
    expect(recap.name).toBe('recap')
    expect(recap.type).toBe('prompt')
    expect(queue.name).toBe('queue')
    expect(queue.type).toBe('local')
    expect(queue.immediate).toBe(true)
    expect(transcript.name).toBe('transcript')
    expect(multiline.name).toBe('multiline')
    expect(multiline.aliases).toContain('ml')
    expect(cd.name).toBe('cd')
  })
})
