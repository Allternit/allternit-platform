// @ts-nocheck
/**
 * settings in the Gizzi TUI: replaces the old Ink ConfigTool. A thin adapter
 * over the shared core (runtime/primitives/settings.ts), the same one the
 * runtime builtin uses (runtime/tools/builtins/settings.ts).
 *
 * - Scopes come from contracts/settings/definitions.json: "account" keys
 *   are applied by allternit-api; "runtime" keys (gizzi.tui_theme,
 *   gizzi.model, gizzi.verbose) are Gizzi Code's own config, read and written
 *   here (./local.ts); "device" keys belong to the Allternit apps.
 * - Every other key goes through the core; the server enforces the risk
 *   class. An apply_on_device appearance.theme is applied to the TUI theme.
 * - Outcomes: applied | apply_on_device | needs_confirmation | locked |
 *   invalid. `previous` (when known) is shown so the user can undo.
 * - The TUI has no settings card, so confirm and locked keys print what the
 *   user must do themselves; nothing here confirms on their behalf.
 * - write is refused in plan mode, as in the runtime (plan agent denies it).
 */
import { z } from 'zod/v4'
import { Settings } from '@/runtime/primitives/settings'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { getRuleByContentsForTool } from '../../utils/permissions/permissions.js'
import { APPEARANCE_THEME, isLocal, readAllLocal, readLocal, saveTheme, tuiThemeFor, writeLocal } from './local.js'
import { DESCRIPTION, SETTINGS_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => Settings.Input)
const outputSchema = lazySchema(() =>
  z.object({
    summary: z.string(),
    text: z.string(),
    /** One line per change, for the transcript. */
    lines: z.array(z.string()).optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** "settings/instructions" -> "Allternit → Settings → Instructions". */
export function settingsPlace(path?: string): string {
  const parts = (path ?? '')
    .split('/')
    .filter(Boolean)
    .filter((p, i) => !(i === 0 && p === 'settings'))
    .map(p => p.charAt(0).toUpperCase() + p.slice(1).replace(/[-_]/g, ' '))
  return ['Allternit', 'Settings', ...parts].join(' → ')
}

/** What the user must do for a change the runtime may not make. */
function userStep(r: Settings.ChangeResult): string | undefined {
  if (r.outcome === 'needs_confirmation') {
    if (r.key === 'gizzi.permission_mode') {
      return 'Change it yourself: press Shift+Tab in Gizzi Code to cycle the permission mode.'
    }
    return `Confirm it yourself: open ${settingsPlace(r.settings_path)} and make the change there.`
  }
  if (r.outcome === 'locked') {
    return `Only the user can change this, in ${settingsPlace(r.settings_path)}.`
  }
  return undefined
}

function line(r: Settings.ChangeResult): string {
  switch (r.outcome) {
    case 'applied':
      return `${r.key}: ${r.previous === undefined ? '' : `${JSON.stringify(r.previous)} → `}${JSON.stringify(r.value)}${r.reason ? ` (${r.reason})` : ''}`
    case 'apply_on_device':
      return `${r.key}: ${r.reason ?? 'applies in the Allternit app on the user’s devices'}`
    case 'invalid':
      return `${r.key}: not changed. ${r.reason ?? ''}`.trim()
    default:
      return `${r.key}: ${r.reason ?? r.outcome}`
  }
}

/** Fill the TUI's part of a server result: device theme, user steps. */
function finish(r: Settings.ChangeResult): Settings.ChangeResult {
  if (r.outcome === 'apply_on_device') {
    if (r.key === APPEARANCE_THEME) {
      const theme = tuiThemeFor(r.value)
      if (theme) {
        const applied = saveTheme(r.key, theme)
        if (applied.outcome === 'applied') return applied
      }
    }
    return { ...r, reason: r.reason ?? 'Applies in the Allternit app on the user’s devices; Gizzi Code has no such setting.' }
  }
  const step = userStep(r)
  return step ? { ...r, reason: step } : r
}

async function read(input: Settings.Input, signal?: AbortSignal) {
  const keys = input.keys
  const local = keys ? keys.filter(isLocal).map(readLocal) : readAllLocal()
  const serverKeys = keys?.filter(k => !isLocal(k))
  const entries = [...local]
  let note = ''
  if (!keys || serverKeys.length) {
    try {
      const r = await Settings.read(getSessionId(), serverKeys, signal)
      entries.push(...(r.settings ?? []).filter(e => !isLocal(e.key)))
    } catch (error) {
      if (signal?.aborted) throw error
      note = `\nAllternit account settings could not be read: ${messageOf(error)}`
    }
  }
  const result: Settings.Result = { action: 'read', settings: entries }
  return {
    summary: `Read ${entries.length} setting${entries.length === 1 ? '' : 's'}`,
    text: JSON.stringify(result) + note,
  }
}

async function write(input: Settings.Input, context) {
  const changes = input.changes ?? []
  if (!changes.length) return { summary: 'No change', text: 'write needs at least one change.' }
  if (context.getAppState().toolPermissionContext.mode === 'plan') {
    return { summary: 'Not changed (plan mode)', text: 'Settings writes are refused in plan mode. Leave plan mode first.' }
  }

  const results = new Map<number, Settings.ChangeResult>()
  const server: { index: number; change: Settings.Change }[] = []
  for (const [index, change] of changes.entries()) {
    if (isLocal(change.key)) results.set(index, await writeLocal(change, context.setAppState))
    else server.push({ index, change })
  }

  if (server.length) {
    const signal = context.abortController?.signal
    try {
      const r = await Settings.write(getSessionId(), context.toolUseId ?? '', server.map(s => s.change), signal)
      const byKey = new Map((r.changes ?? []).map(c => [c.key, c]))
      for (const { index, change } of server) {
        const c = byKey.get(change.key) ?? { key: change.key, outcome: 'invalid', reason: 'No result from Allternit.' }
        results.set(index, finish(c))
      }
    } catch (error) {
      if (signal?.aborted) throw error
      for (const { index, change } of server) {
        results.set(index, { key: change.key, outcome: 'invalid', reason: `Not changed: ${messageOf(error)}` })
      }
    }
  }

  const ordered = changes.map((_, i) => results.get(i)!)
  const applied = ordered.filter(r => r.outcome === 'applied').length
  const result: Settings.Result = { action: 'write', changes: ordered }
  return {
    summary: applied === ordered.length ? `Changed ${applied} setting${applied === 1 ? '' : 's'}` : `Changed ${applied} of ${ordered.length} settings`,
    text: JSON.stringify(result),
    lines: ordered.map(line),
  }
}

export const SettingsTool = buildTool({
  name: SETTINGS_TOOL_NAME,
  searchHint: 'read or change settings: theme, model, verbose, Allternit account',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Settings',
  get inputSchema() {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  shouldDefer: true,
  isReadOnly: input => input?.action !== 'write',
  isConcurrencySafe: input => input?.action !== 'write',
  toAutoClassifierInput: input =>
    input.action === 'write'
      ? (input.changes ?? []).map(c => `${c.key} = ${JSON.stringify(c.value)}`).join(', ')
      : `read ${(input.keys ?? []).join(' ')}`.trim(),
  async checkPermissions(input, context) {
    // Same as the runtime: allow, with the action as the rule pattern so a
    // user's deny or ask rule for settings(write) still holds.
    const permissionContext = context.getAppState().toolPermissionContext
    const ruleContent = input.action
    const deny = getRuleByContentsForTool(permissionContext, SettingsTool, 'deny').get(ruleContent)
    if (deny) {
      return {
        behavior: 'deny',
        message: `${SETTINGS_TOOL_NAME} ${ruleContent} is denied by a permission rule.`,
        decisionReason: { type: 'rule', rule: deny },
      }
    }
    const ask = getRuleByContentsForTool(permissionContext, SettingsTool, 'ask').get(ruleContent)
    if (ask) {
      return {
        behavior: 'ask',
        message: ruleContent === 'write' ? 'Gizzi wants to change your settings.' : 'Gizzi wants to read your settings.',
        decisionReason: { type: 'rule', rule: ask },
      }
    }
    return { behavior: 'allow', updatedInput: input }
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return DESCRIPTION
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.text }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    try {
      return { data: input.action === 'write' ? await write(input, context) : await read(input, context.abortController?.signal) }
    } catch (error) {
      if (context.abortController?.signal.aborted) throw error
      return { data: { summary: 'Settings unavailable', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)
