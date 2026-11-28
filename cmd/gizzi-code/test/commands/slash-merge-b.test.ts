/**
 * Gizzi overhaul plan D4 (steps 26+27): /remote and /memory own their old
 * names, /commit and /commit-push-pr are listed, /models opens /model, and
 * /help groups commands by job with a Workspace group.
 */
import { describe, expect, test } from 'bun:test'
import remote from '../../src/cli/ui/ink-app/commands/remote/index.ts'
import session from '../../src/cli/ui/ink-app/commands/session/index.ts'
import desktop from '../../src/cli/ui/ink-app/commands/desktop/index.ts'
import mobile from '../../src/cli/ui/ink-app/commands/mobile/index.ts'
import native from '../../src/cli/ui/ink-app/commands/native/index.ts'
import memory from '../../src/cli/ui/ink-app/commands/memory/index.ts'
import memorySearch from '../../src/cli/ui/ink-app/commands/memory-search/index.ts'
import remember from '../../src/cli/ui/ink-app/commands/remember/index.ts'
import model from '../../src/cli/ui/ink-app/commands/model/index.ts'
import commit from '../../src/cli/ui/ink-app/commands/commit.ts'
import commitPushPr, { getPromptContent } from '../../src/cli/ui/ink-app/commands/commit-push-pr.ts'
import { routines } from '../../src/cli/ui/ink-app/commands/work/index.ts'
import { remoteSubcommand } from '../../src/cli/ui/ink-app/commands/remote/subcommand.ts'
import { memoryRoute } from '../../src/cli/ui/ink-app/commands/memory/subcommand.ts'
import {
  getCommandCategory,
  groupCommandsForHelp,
  HIDDEN_ALIAS_OWNERS,
  isHelpVisible,
  isSlashMenuVisible,
  SLASH_CATEGORY_ORDER,
} from '../../src/cli/ui/ink-app/utils/suggestions/commandSource.ts'
import { findCommand, INTERNAL_ONLY_COMMANDS } from '../../src/cli/ui/ink-app/commands.ts'
import { generateCommandSuggestions } from '../../src/cli/ui/ink-app/utils/suggestions/commandSuggestions.ts'
import type { Command } from '../../src/cli/ui/ink-app/types/command.ts'

const hiddenAliases: Command[] = [session, desktop, mobile, native, memorySearch, remember]

/** A Workspace pane as its own branch registers it: hidden on that branch. */
function hiddenPane(name: string): Command {
  return {
    type: 'local-jsx',
    name,
    description: `${name} view`,
    isHidden: true,
    load: async () => ({ call: async () => null }),
  } as Command
}
const workspacePanes = ['sessions', 'pods', 'connections', 'sleep'].map(hiddenPane)

const registry: Command[] = [remote, memory, model, commit, commitPushPr, routines, ...hiddenAliases, ...workspacePanes]

describe('/remote owns session, desktop, mobile, native', () => {
  test('each old name is a /remote view', () => {
    expect(remoteSubcommand('session')).toEqual({ view: 'session', rest: '' })
    expect(remoteSubcommand('desktop')).toEqual({ view: 'desktop', rest: '' })
    expect(remoteSubcommand(' Mobile ')).toEqual({ view: 'mobile', rest: '' })
    expect(remoteSubcommand('native pickup codex abc')).toEqual({ view: 'native', rest: 'pickup codex abc' })
    expect(remoteSubcommand('')).toBeUndefined()
    expect(remoteSubcommand('phone')).toBeUndefined()
  })

  test('the old names keep their own aliases', () => {
    expect(findCommand('ios', registry)).toBe(mobile)
    expect(findCommand('app', registry)).toBe(desktop)
    expect(findCommand('cli-session', registry)).toBe(native)
  })
})

describe('/memory owns memory-search and remember, and has a Sleep entry', () => {
  test('search, add and sleep route', () => {
    expect(memoryRoute('search deploy notes')).toEqual({ kind: 'search', rest: 'deploy notes' })
    expect(memoryRoute('add use pnpm here')).toEqual({ kind: 'add', rest: 'use pnpm here' })
    expect(memoryRoute('remember --project use pnpm')).toEqual({ kind: 'add', rest: '--project use pnpm' })
    expect(memoryRoute('sleep')).toEqual({ kind: 'sleep-view' })
    // on/off stays the text switch inside memory.tsx
    expect(memoryRoute('sleep off')).toBeUndefined()
    expect(memoryRoute('view preferences.md')).toBeUndefined()
    expect(memoryRoute('')).toBeUndefined()
  })
})

describe('hidden aliases work but are never offered', () => {
  test('every new hidden alias is hidden and owned by a listed command', () => {
    for (const cmd of hiddenAliases) {
      expect(cmd.isHidden).toBe(true)
      expect(isSlashMenuVisible(cmd)).toBe(false)
      expect(isHelpVisible(cmd)).toBe(false)
    }
    expect(HIDDEN_ALIAS_OWNERS.session).toBe('remote')
    expect(HIDDEN_ALIAS_OWNERS.desktop).toBe('remote')
    expect(HIDDEN_ALIAS_OWNERS.mobile).toBe('remote')
    expect(HIDDEN_ALIAS_OWNERS.native).toBe('remote')
    expect(HIDDEN_ALIAS_OWNERS['memory-search']).toBe('memory')
    expect(HIDDEN_ALIAS_OWNERS.remember).toBe('memory')
    expect(isSlashMenuVisible(remote)).toBe(true)
    expect(isSlashMenuVisible(memory)).toBe(true)
  })

  test('the bare slash menu does not suggest them', () => {
    const names = generateCommandSuggestions('/', registry).map(s => s.displayText)
    for (const cmd of hiddenAliases) {
      expect(names.some(n => n === `/${cmd.name}` || n.startsWith(`/${cmd.name} `))).toBe(false)
    }
  })

  test('typing an old name exactly still finds it', () => {
    for (const cmd of hiddenAliases) expect(findCommand(cmd.name, registry)).toBe(cmd)
  })
})

describe('/commit, /commit-push-pr and /models', () => {
  test('commit commands are no longer internal-only', () => {
    expect(INTERNAL_ONLY_COMMANDS).not.toContain(commit)
    expect(INTERNAL_ONLY_COMMANDS).not.toContain(commitPushPr)
    expect(isSlashMenuVisible(commit)).toBe(true)
    expect(isSlashMenuVisible(commitPushPr)).toBe(true)
    expect(getCommandCategory(commit)).toBe('Code')
    expect(getCommandCategory(commitPushPr)).toBe('Code')
  })

  test('commit-push-pr asks for no upstream reviewer and never merges', () => {
    const text = JSON.stringify(commitPushPr.allowedTools) + getPromptContent('main', '')
    expect(text).not.toContain('anthropics/claude-code')
    expect(text).not.toContain('gh pr merge')
  })

  test('/models resolves to /model', () => {
    expect(model.aliases).toContain('models')
    expect(findCommand('models', registry)).toBe(model)
  })
})

describe('/help groups by job with a Workspace group', () => {
  test('group order is Session, Code, Workspace, Account, Settings', () => {
    expect(SLASH_CATEGORY_ORDER.slice(0, 5)).toEqual(['Session', 'Code', 'Workspace', 'Account', 'Settings'])
  })

  test('Workspace lists sessions, pods, connections, sleep even though their panes are hidden', () => {
    for (const pane of workspacePanes) {
      expect(isSlashMenuVisible(pane)).toBe(true)
      expect(getCommandCategory(pane)).toBe('Workspace')
    }
    const names = generateCommandSuggestions('/', registry).map(s => s.displayText)
    for (const pane of workspacePanes) expect(names.some(n => n.includes(`/${pane.name}`))).toBe(true)
  })

  test('/routines is named in /help under Workspace but stays out of the slash menu', () => {
    expect(isSlashMenuVisible(routines)).toBe(false)
    expect(isHelpVisible(routines)).toBe(true)
    const groups = groupCommandsForHelp(registry.filter(isHelpVisible))
    const workspace = groups.find(g => g.group === 'Workspace')!
    expect(workspace.commands.map(c => c.name)).toEqual(['connections', 'memory', 'pods', 'routines', 'sessions', 'sleep'])
    expect(groups.map(g => g.group)).toEqual(['Session', 'Code', 'Workspace'])
    expect(groups.find(g => g.group === 'Code')!.commands.map(c => c.name)).toEqual(['commit', 'commit-push-pr'])
  })
})
