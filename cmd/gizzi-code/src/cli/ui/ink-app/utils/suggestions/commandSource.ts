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

/**
 * Job groups for /help and the slash menu (overhaul plan step 27), in display
 * order. Commands with no group fall through to Other.
 */
export const SLASH_CATEGORY_ORDER = [
  'Session',
  'Code',
  'Workspace',
  'Account',
  'Settings',
  'Other',
] as const

export type SlashCategory = (typeof SLASH_CATEGORY_ORDER)[number]

const COMMAND_CATEGORIES: Record<string, SlashCategory> = {
  // Session: this conversation, its model and mode, and where it runs.
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
  tag: 'Session',
  branch: 'Session',
  fork: 'Session',
  btw: 'Session',
  history: 'Session',
  'edit-prompt': 'Session',
  share: 'Session',
  transcript: 'Session',
  recap: 'Session',
  queue: 'Session',
  answer: 'Session',
  edit: 'Session',
  runtime: 'Session',
  remote: 'Session',
  model: 'Session',
  effort: 'Session',
  plan: 'Session',
  think: 'Session',
  fast: 'Session',
  goal: 'Session',
  mission: 'Session',
  loops: 'Session',
  tasks: 'Session',
  // Code: the repository in front of you.
  commit: 'Code',
  'commit-push-pr': 'Code',
  diff: 'Code',
  files: 'Code',
  review: 'Code',
  'security-review': 'Code',
  'pr-comments': 'Code',
  init: 'Code',
  'add-dir': 'Code',
  cd: 'Code',
  find: 'Code',
  grep: 'Code',
  fix: 'Code',
  ide: 'Code',
  'install-github-app': 'Code',
  // Workspace: everything beyond this one chat.
  sessions: 'Workspace',
  pods: 'Workspace',
  connections: 'Workspace',
  sleep: 'Workspace',
  routines: 'Workspace',
  memory: 'Workspace',
  knowledge: 'Workspace',
  agents: 'Workspace',
  swarm: 'Workspace',
  workflows: 'Workspace',
  workflow: 'Workspace',
  artifact: 'Workspace',
  computers: 'Workspace',
  projects: 'Workspace',
  decisions: 'Workspace',
  pr: 'Workspace',
  bots: 'Workspace',
  autopilot: 'Workspace',
  away: 'Workspace',
  factory: 'Workspace',
  board: 'Workspace',
  dashboard: 'Workspace',
  design: 'Workspace',
  image: 'Workspace',
  // Account
  login: 'Account',
  logout: 'Account',
  'privacy-settings': 'Account',
  passes: 'Account',
  // Settings: how Gizzi behaves and looks, and what it can reach.
  config: 'Settings',
  theme: 'Settings',
  keybindings: 'Settings',
  statusline: 'Settings',
  color: 'Settings',
  timestamps: 'Settings',
  doctor: 'Settings',
  'terminal-setup': 'Settings',
  'release-notes': 'Settings',
  help: 'Settings',
  feedback: 'Settings',
  'import-claude': 'Settings',
  vim: 'Settings',
  'always-approve': 'Settings',
  auto: 'Settings',
  permissions: 'Settings',
  sandbox: 'Settings',
  plugin: 'Settings',
  skills: 'Settings',
  hooks: 'Settings',
  mcp: 'Settings',
  'reload-plugins': 'Settings',
  pet: 'Settings',
  multiline: 'Settings',
  voice: 'Settings',
  customize: 'Settings',
  advisor: 'Settings',
  'remote-env': 'Settings',
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
  session: 'remote',
  desktop: 'remote',
  mobile: 'remote',
  native: 'remote',
  'memory-search': 'memory',
  remember: 'memory',
}

/**
 * Workspace views listed by name (overhaul S2b). Their commands set isHidden
 * on their own branches (#3235 sleep, #3238 pods and connections, #3242
 * sessions); this list makes them visible in the slash menu and /help without
 * importing them, so a build without one of them simply has nothing to list.
 */
export const WORKSPACE_LISTED: ReadonlySet<string> = new Set([
  'sessions',
  'pods',
  'connections',
  'sleep',
])

/** Hidden aliases that /help still names under their group (not the slash menu). */
export const HELP_LISTED_ALIASES: ReadonlySet<string> = new Set(['routines'])

/** True for an old command name that only survives as a hidden alias. */
export function isHiddenAlias(cmd: Command): boolean {
  // Built-ins only: a user skill that happens to be named `stats` stays listed.
  if (cmd.type !== 'local' && cmd.type !== 'local-jsx') return false
  return Object.hasOwn(HIDDEN_ALIAS_OWNERS, getCommandName(cmd))
}

function isBuiltIn(cmd: Command): boolean {
  return cmd.type === 'local' || cmd.type === 'local-jsx'
}

/** Commands the slash menu may offer: not hidden, not a merged old name. */
export function isSlashMenuVisible(cmd: Command): boolean {
  if (isHiddenAlias(cmd)) return false
  if (isBuiltIn(cmd) && WORKSPACE_LISTED.has(getCommandName(cmd))) return true
  return !cmd.isHidden
}

/** Commands /help lists: the slash-menu set plus the few aliases it names. */
export function isHelpVisible(cmd: Command): boolean {
  if (isBuiltIn(cmd) && HELP_LISTED_ALIASES.has(getCommandName(cmd))) return true
  return isSlashMenuVisible(cmd)
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

export type CommandGroup = { group: SlashCategory; commands: Command[] }

/**
 * /help grouped by job: one entry per non-empty group in
 * SLASH_CATEGORY_ORDER, each sorted by name, one row per command name.
 */
export function groupCommandsForHelp(commands: readonly Command[]): CommandGroup[] {
  const seen = new Set<string>()
  const byGroup = new Map<SlashCategory, Command[]>()
  for (const cmd of commands) {
    const name = getCommandName(cmd)
    if (seen.has(name)) continue
    seen.add(name)
    const group = getCommandCategory(cmd)
    const list = byGroup.get(group) ?? []
    list.push(cmd)
    byGroup.set(group, list)
  }
  return SLASH_CATEGORY_ORDER.filter(group => byGroup.has(group)).map(group => ({
    group,
    commands: byGroup.get(group)!.sort((a, b) => getCommandName(a).localeCompare(getCommandName(b))),
  }))
}
