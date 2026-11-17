// @ts-nocheck
/**
 * The TUI's own settings, the device scope of the `settings` tool in Gizzi
 * Code: gizzi.tui_theme, gizzi.model, gizzi.verbose (keys from
 * contracts/settings/definitions.json, class safe). The TUI reads and writes
 * them here, in its own config; every other key goes through the shared core
 * (runtime/primitives/settings.ts) and the server's risk gate.
 */
import { feature } from 'bun:bundle'
import type { Settings } from '@/runtime/primitives/settings'
import { getGlobalConfig, saveGlobalConfig } from '../../utils/config.js'
import { getModelOptions } from '../../utils/model/modelOptions.js'
import { validateModel } from '../../utils/model/validateModel.js'
import { getInitialSettings, updateSettingsForSource } from '../../utils/settings/settings.js'
import { THEME_NAMES, THEME_SETTINGS } from '../../utils/theme.js'

export const TUI_THEME = 'gizzi.tui_theme'
export const TUI_MODEL = 'gizzi.model'
export const TUI_VERBOSE = 'gizzi.verbose'
const LOCAL_KEYS = [TUI_THEME, TUI_MODEL, TUI_VERBOSE]

/** The device-scope key the core may hand back as apply_on_device. */
export const APPEARANCE_THEME = 'appearance.theme'

/** setAppState from the tool context. */
type SetAppState = (f: (prev: any) => any) => void

export function isLocal(key: string): boolean {
  return LOCAL_KEYS.includes(key)
}

function themeValues(): string[] {
  return [...(feature('AUTO_THEME') ? THEME_SETTINGS : THEME_NAMES)]
}

function modelValues(): string[] {
  try {
    return ['default', ...getModelOptions().filter(o => o.value !== null).map(o => String(o.value))]
  } catch {
    return ['default', 'sonnet', 'opus', 'haiku']
  }
}

function currentValue(key: string): unknown {
  if (key === TUI_THEME) return getGlobalConfig().theme
  if (key === TUI_VERBOSE) return Boolean(getGlobalConfig().verbose)
  return getInitialSettings().model ?? 'default'
}

export function readLocal(key: string): Settings.ReadEntry {
  if (key === TUI_THEME) {
    return { key, class: 'safe', label: 'Gizzi Code theme', scope: 'runtime', type: 'enum', values: themeValues(), value: currentValue(key) }
  }
  if (key === TUI_VERBOSE) {
    return { key, class: 'safe', label: 'Gizzi Code verbose output', scope: 'runtime', type: 'boolean', value: currentValue(key) }
  }
  return { key, class: 'safe', label: 'Gizzi Code model', scope: 'runtime', type: 'string', values: modelValues(), value: currentValue(key) }
}

export function readAllLocal(): Settings.ReadEntry[] {
  return LOCAL_KEYS.map(readLocal)
}

function invalid(key: string, reason: string): Settings.ChangeResult {
  return { key, outcome: 'invalid', reason }
}

function toBoolean(value: unknown): boolean | undefined {
  if (typeof value === 'boolean') return value
  if (typeof value === 'string') {
    const v = value.trim().toLowerCase()
    if (v === 'true' || v === 'on') return true
    if (v === 'false' || v === 'off') return false
  }
  return undefined
}

/** Save a TUI theme. The running terminal keeps its theme until restart; /theme switches now. */
export function saveTheme(key: string, theme: string): Settings.ChangeResult {
  const values = themeValues()
  if (!values.includes(theme)) return invalid(key, `Theme "${theme}" is not one of: ${values.join(', ')}`)
  const previous = getGlobalConfig().theme
  saveGlobalConfig(prev => (prev.theme === theme ? prev : { ...prev, theme }))
  return {
    key,
    outcome: 'applied',
    previous,
    value: theme,
    reason: 'Saved. Gizzi Code shows it the next time it starts; /theme switches it now.',
  }
}

/** appearance.theme (system/light/dark) as a TUI theme. */
export function tuiThemeFor(appearance: unknown): string | undefined {
  if (appearance === 'light' || appearance === 'dark') return appearance
  if (appearance === 'system') return themeValues().includes('auto') ? 'auto' : undefined
  return undefined
}

export async function writeLocal(change: Settings.Change, setAppState: SetAppState): Promise<Settings.ChangeResult> {
  const { key, value } = change
  try {
    if (key === TUI_THEME) return saveTheme(key, String(value))

    if (key === TUI_VERBOSE) {
      const next = toBoolean(value)
      if (next === undefined) return invalid(key, 'gizzi.verbose takes true or false.')
      const previous = currentValue(key)
      saveGlobalConfig(prev => (prev.verbose === next ? prev : { ...prev, verbose: next }))
      setAppState(prev => (prev.verbose === next ? prev : { ...prev, verbose: next }))
      return { key, outcome: 'applied', previous, value: next }
    }

    // gizzi.model: "default" clears the override.
    const model = String(value ?? '').trim()
    const previous = currentValue(key)
    if (model.toLowerCase() === 'default') {
      const r = updateSettingsForSource('userSettings', { model: undefined })
      if (r.error) return invalid(key, r.error.message)
      setAppState(prev => (prev.mainLoopModel === null ? prev : { ...prev, mainLoopModel: null }))
      return { key, outcome: 'applied', previous, value: 'default' }
    }
    const check = await validateModel(model)
    if (!check.valid) return invalid(key, check.error ?? `Model "${model}" is not available.`)
    const r = updateSettingsForSource('userSettings', { model })
    if (r.error) return invalid(key, r.error.message)
    setAppState(prev => (prev.mainLoopModel === model ? prev : { ...prev, mainLoopModel: model }))
    return { key, outcome: 'applied', previous, value: model }
  } catch (error) {
    return invalid(key, `Not changed: ${error instanceof Error ? error.message : String(error)}`)
  }
}
