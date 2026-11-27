import type { Command } from '../../types/command.js'
import { getCommandName } from '../../types/command.js'

/**
 * Grok-style source badge shown in the slash menu.
 * Built-ins stay `built-in`. Skills are `skill · <scope>` so collisions
 * (same name from a plugin vs a builtin) are visible in the picker.
 */
export function getCommandSourceTag(cmd: Command): string {
  if (cmd.type === 'prompt' && cmd.kind === 'workflow') {
    return 'workflow'
  }

  if (cmd.type === 'local' || cmd.type === 'local-jsx') {
    return 'built-in'
  }

  if (cmd.type !== 'prompt') {
    return 'built-in'
  }

  if (cmd.source === 'plugin') {
    const pluginName = cmd.pluginInfo?.pluginManifest?.name
    return pluginName ? `skill · ${pluginName}` : 'skill · plugin'
  }

  if (cmd.source === 'mcp' || cmd.loadedFrom === 'mcp' || cmd.isMcp) {
    return 'mcp'
  }

  if (cmd.source === 'bundled' || cmd.loadedFrom === 'bundled') {
    return 'bundled'
  }

  if (cmd.source === 'builtin') {
    return 'built-in'
  }

  if (cmd.source === 'projectSettings' || cmd.source === 'localSettings') {
    return 'skill · local'
  }

  if (cmd.source === 'userSettings') {
    return 'skill · user'
  }

  if (cmd.source === 'policySettings') {
    return 'skill · managed'
  }

  if (cmd.loadedFrom === 'skills') {
    return 'skill · user'
  }

  return 'skill'
}

export function getCommandArgumentHint(cmd: Command): string | undefined {
  if (cmd.argumentHint && cmd.argumentHint.trim()) {
    return cmd.argumentHint.trim()
  }
  if (cmd.type === 'prompt' && cmd.argNames?.length) {
    return cmd.argNames.map(name => `<${name}>`).join(' ')
  }
  return undefined
}

/** Grok user-guide category order. Unknown names fall through to Other. */
export const SLASH_CATEGORY_ORDER = [
  'Session',
  'Model and Mode',
  'Memory',
  'Extensions',
  'Workflows',
  'Agents',
  'Account',
  'Configuration',
  'Other',
] as const

export type SlashCategory = (typeof SLASH_CATEGORY_ORDER)[number]

const COMMAND_CATEGORIES: Record<string, SlashCategory> = {
  clear: 'Session',
  resume: 'Session',
  compact: 'Session',
  context: 'Session',
  copy: 'Session',
  export: 'Session',
  exit: 'Session',
  rename: 'Session',
  rewind: 'Session',
  status: 'Session',
  dash: 'Session',
  session: 'Session',
  tag: 'Session',
  branch: 'Session',
  fork: 'Session',
  btw: 'Session',
  history: 'Session',
  'edit-prompt': 'Session',
  files: 'Session',
  diff: 'Session',
  share: 'Session',
  model: 'Model and Mode',
  effort: 'Model and Mode',
  'always-approve': 'Model and Mode',
  auto: 'Model and Mode',
  vim: 'Model and Mode',
  plan: 'Model and Mode',
  think: 'Model and Mode',
  fast: 'Model and Mode',
  permissions: 'Model and Mode',
  sandbox: 'Model and Mode',
  passes: 'Model and Mode',
  memory: 'Memory',
  'memory-search': 'Memory',
  remember: 'Memory',
  plugin: 'Extensions',
  skills: 'Extensions',
  hooks: 'Extensions',
  mcp: 'Extensions',
  'reload-plugins': 'Extensions',
  'install-github-app': 'Extensions',
  ide: 'Extensions',
  loops: 'Workflows',
  goal: 'Workflows',
  mission: 'Workflows',
  tasks: 'Workflows',
  workflows: 'Workflows',
  workflow: 'Workflows',
  agents: 'Agents',
  swarm: 'Agents',
  pet: 'Agents',
  login: 'Account',
  logout: 'Account',
  'privacy-settings': 'Account',
  config: 'Configuration',
  theme: 'Configuration',
  keybindings: 'Configuration',
  statusline: 'Configuration',
  color: 'Configuration',
  timestamps: 'Configuration',
  doctor: 'Configuration',
  'terminal-setup': 'Configuration',
  'release-notes': 'Configuration',
  help: 'Configuration',
  feedback: 'Configuration',
  'import-claude': 'Configuration',
  init: 'Configuration',
}

/**
 * Merged commands (overhaul plan D4, Eoj E3): each old name still runs, as a
 * hidden alias of its owner for one release, and is never offered in the
 * slash menu, /help or typeahead. The commands carry `isHidden: true`; this
 * map is the single list of them and the owner each one opens.
 */
export const HIDDEN_ALIAS_OWNERS: Readonly<Record<string, string>> = {
  stats: 'status',
  'session-info': 'status',
  'extra-usage': 'status',
  usage: 'status',
  cost: 'status',
  'resume-now': 'resume',
  'keep-going': 'resume',
  'view-plan': 'plan',
  routines: 'loops',
}

/** True for an old command name that only survives as a hidden alias. */
export function isHiddenAlias(cmd: Command): boolean {
  // Built-ins only: a user skill that happens to be named `stats` stays listed.
  if (cmd.type !== 'local' && cmd.type !== 'local-jsx') return false
  return Object.hasOwn(HIDDEN_ALIAS_OWNERS, getCommandName(cmd))
}

/** Commands the slash menu may offer: not hidden, not a merged old name. */
export function isSlashMenuVisible(cmd: Command): boolean {
  return !cmd.isHidden && !isHiddenAlias(cmd)
}

export function getCommandCategory(cmd: Command): SlashCategory {
  const name = getCommandName(cmd)
  return COMMAND_CATEGORIES[name] ?? 'Other'
}

export function compareCommandsForSlashMenu(a: Command, b: Command): number {
  const aCat = SLASH_CATEGORY_ORDER.indexOf(getCommandCategory(a))
  const bCat = SLASH_CATEGORY_ORDER.indexOf(getCommandCategory(b))
  if (aCat !== bCat) return aCat - bCat
  return getCommandName(a).localeCompare(getCommandName(b))
}
