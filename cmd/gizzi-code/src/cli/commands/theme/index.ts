/**
 * Theme Command
 * Production-quality theme management with custom palette editing.
 *
 * Supports three immutable built-ins (dark, light, system) plus arbitrary
 * user-defined named themes stored in ~/.config/gizzi-code/themes.json.
 */

import { log } from '../../utils/log.js'
import { loadPreferences, savePreferences, getPreference } from '../../../utils/sessionStorage.js'
import { writeFile, readFile, mkdir, access, constants } from 'fs/promises'
import { join } from 'path'
import { GlobalPaths } from '../../../runtime/context/global/paths.js'

export type BuiltinThemeName = 'dark' | 'light' | 'system'
export type ThemeName = BuiltinThemeName | string

export type ColorKey =
  | 'background'
  | 'foreground'
  | 'primary'
  | 'secondary'
  | 'success'
  | 'error'
  | 'warning'
  | 'muted'
  | 'accent'
  | 'border'
  | 'link'

export interface ThemeConfig {
  name: ThemeName
  background: string
  foreground: string
  primary: string
  secondary: string
  success: string
  error: string
  warning: string
  muted: string
  accent: string
  border: string
  link: string
}

export interface CustomThemeConfig extends ThemeConfig {
  custom: true
  base?: 'dark' | 'light'
}

const BUILTIN_NAMES: BuiltinThemeName[] = ['dark', 'light', 'system']

const DEFAULT_DARK: ThemeConfig = {
  name: 'dark',
  background: '#1a1a1a',
  foreground: '#e0e0e0',
  primary: '#6C5CE7',
  secondary: '#a29bfe',
  success: '#00b894',
  error: '#ff7675',
  warning: '#fdcb6e',
  muted: '#636e72',
  accent: '#fd79a8',
  border: '#2d3436',
  link: '#74b9ff',
}

const DEFAULT_LIGHT: ThemeConfig = {
  name: 'light',
  background: '#ffffff',
  foreground: '#2d3436',
  primary: '#6C5CE7',
  secondary: '#a29bfe',
  success: '#00b894',
  error: '#ff7675',
  warning: '#fdcb6e',
  muted: '#b2bec3',
  accent: '#e84393',
  border: '#dfe6e9',
  link: '#0984e3',
}

const DEFAULT_SYSTEM: ThemeConfig = {
  name: 'system',
  background: 'auto',
  foreground: 'auto',
  primary: '#6C5CE7',
  secondary: '#a29bfe',
  success: '#00b894',
  error: '#ff7675',
  warning: '#fdcb6e',
  muted: 'auto',
  accent: '#fd79a8',
  border: 'auto',
  link: '#74b9ff',
}

const BUILTINS: Record<BuiltinThemeName, ThemeConfig> = {
  dark: DEFAULT_DARK,
  light: DEFAULT_LIGHT,
  system: DEFAULT_SYSTEM,
}

const COLOR_KEYS: ColorKey[] = [
  'background',
  'foreground',
  'primary',
  'secondary',
  'success',
  'error',
  'warning',
  'muted',
  'accent',
  'border',
  'link',
]

function getConfigDir(): string {
  return GlobalPaths.config
}

function getThemeFilePath(): string {
  return join(getConfigDir(), 'theme.json')
}

function getCustomThemesFilePath(): string {
  return join(getConfigDir(), 'themes.json')
}

async function ensureConfigDir(): Promise<void> {
  const configDir = getConfigDir()
  try {
    await access(configDir, constants.F_OK)
  } catch {
    await mkdir(configDir, { recursive: true })
  }
}

function isBuiltin(name: ThemeName): name is BuiltinThemeName {
  return BUILTIN_NAMES.includes(name as BuiltinThemeName)
}

function isColorKey(key: string): key is ColorKey {
  return COLOR_KEYS.includes(key as ColorKey)
}

function isHexColor(value: string): boolean {
  return /^#([0-9A-Fa-f]{3}){1,2}$/.test(value)
}

/**
 * Load user-defined custom themes from disk.
 */
export async function loadCustomThemes(): Promise<Record<string, CustomThemeConfig>> {
  try {
    const data = await readFile(getCustomThemesFilePath(), 'utf8')
    const parsed = JSON.parse(data)
    if (parsed && typeof parsed === 'object') {
      return parsed as Record<string, CustomThemeConfig>
    }
  } catch {
    // File missing or corrupted; return empty set
  }
  return {}
}

/**
 * Persist user-defined custom themes to disk.
 */
async function saveCustomThemes(themes: Record<string, CustomThemeConfig>): Promise<void> {
  await ensureConfigDir()
  await writeFile(getCustomThemesFilePath(), JSON.stringify(themes, null, 2))
}

/**
 * Get a merged theme definition by name. Built-ins take precedence over custom.
 */
export async function getThemeByName(name: ThemeName): Promise<ThemeConfig | undefined> {
  if (isBuiltin(name)) {
    return BUILTINS[name]
  }
  const customs = await loadCustomThemes()
  return customs[name]
}

/**
 * Get all available theme names, with the active one marked.
 */
export async function listAllThemeNames(): Promise<{ names: ThemeName[]; active: ThemeName }> {
  const customs = await loadCustomThemes()
  const names: ThemeName[] = [...BUILTIN_NAMES, ...Object.keys(customs)]
  const active = await getCurrentTheme()
  return { names, active }
}

/**
 * Get current theme name from preferences.
 */
export async function getCurrentTheme(): Promise<ThemeName> {
  const theme = await getPreference<ThemeName>('theme', 'dark')
  if (isBuiltin(theme)) return theme
  const customs = await loadCustomThemes()
  return customs[theme] ? theme : 'dark'
}

/**
 * Get system preferred theme.
 */
function getSystemTheme(): BuiltinThemeName {
  try {
    const { execSync } = require('child_process')
    const result = execSync('defaults read -g AppleInterfaceStyle', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'ignore'],
    })
    if (result.trim() === 'Dark') return 'dark'
  } catch {
    // Not macOS or command failed
  }
  return 'light'
}

/**
 * Resolve a system/auto theme to a concrete dark or light palette.
 */
function resolveSystemTheme(): ThemeConfig {
  const systemName = getSystemTheme()
  return BUILTINS[systemName]
}

/**
 * Get effective theme (resolves 'system' to actual theme).
 */
export async function getEffectiveTheme(): Promise<ThemeConfig> {
  const current = await getCurrentTheme()

  if (current === 'system') {
    return resolveSystemTheme()
  }

  const theme = await getThemeByName(current)
  return theme ?? BUILTINS.dark
}

/**
 * Write the active theme to the legacy theme.json file so other tools can read it.
 */
async function writeActiveThemeFile(theme: ThemeConfig): Promise<void> {
  try {
    await ensureConfigDir()
    await writeFile(getThemeFilePath(), JSON.stringify(theme, null, 2))
  } catch {
    // Ignore write errors
  }
}

/**
 * Set theme by name. Accepts built-ins or custom user themes.
 */
export async function setTheme(name: ThemeName): Promise<void> {
  const theme = await getThemeByName(name)
  if (!theme) {
    throw new Error(`Invalid theme: ${name}. Available: ${(await listAllThemeNames()).names.join(', ')}`)
  }

  const prefs = await loadPreferences()
  prefs.theme = name
  await savePreferences(prefs)
  await writeActiveThemeFile(theme)
}

/**
 * Create or overwrite a custom named theme.
 */
export async function saveCustomTheme(
  name: string,
  overrides: Partial<Record<ColorKey, string>>,
  base: 'dark' | 'light' = 'dark',
): Promise<CustomThemeConfig> {
  if (isBuiltin(name as ThemeName)) {
    throw new Error(`Cannot overwrite built-in theme "${name}"`)
  }
  if (!/^[-a-zA-Z0-9_]+$/.test(name)) {
    throw new Error(
      `Theme name must contain only letters, numbers, hyphens, and underscores: ${name}`,
    )
  }

  const customs = await loadCustomThemes()
  const baseTheme = base === 'light' ? DEFAULT_LIGHT : DEFAULT_DARK
  const previous = customs[name]

  const theme: CustomThemeConfig = {
    ...(previous ?? baseTheme),
    name,
    custom: true,
    base,
  }

  for (const [key, value] of Object.entries(overrides)) {
    if (isColorKey(key) && value !== undefined) {
      const normalized = validateColorOverride(key, value)
      theme[key] = normalized[key]
    }
  }

  customs[name] = theme
  await saveCustomThemes(customs)
  return theme
}

/**
 * Delete a custom theme. Falls back to dark if it was active.
 */
export async function deleteCustomTheme(name: ThemeName): Promise<void> {
  if (isBuiltin(name)) {
    throw new Error(`Cannot delete built-in theme "${name}"`)
  }

  const customs = await loadCustomThemes()
  if (!customs[name]) {
    throw new Error(`Custom theme not found: ${name}`)
  }

  delete customs[name]
  await saveCustomThemes(customs)

  const current = await getCurrentTheme()
  if (current === name) {
    await setTheme('dark')
  }
}

/**
 * Parse a flat list of CLI args for --key value and --key=value flags.
 */
function parseFlags(args: string[]): Record<string, string> {
  const flags: Record<string, string> = {}
  let i = 0
  while (i < args.length) {
    const arg = args[i]
    if (arg.startsWith('--')) {
      const eq = arg.indexOf('=')
      if (eq !== -1) {
        flags[arg.slice(2, eq)] = arg.slice(eq + 1)
      } else {
        const next = args[i + 1]
        if (next !== undefined && !next.startsWith('--')) {
          flags[arg.slice(2)] = next
          i++
        } else {
          flags[arg.slice(2)] = ''
        }
      }
    }
    i++
  }
  return flags
}

/**
 * Show current theme info.
 */
async function showTheme(): Promise<void> {
  const current = await getCurrentTheme()
  const effective = await getEffectiveTheme()

  log('info', `Current theme: ${current}`)
  if (current === 'system') {
    log('info', `Effective theme: ${effective.name}`)
  }
  log('info', `Background: ${effective.background}`)
  log('info', `Foreground: ${effective.foreground}`)
  log('info', `Primary: ${effective.primary}`)
  log('info', `Secondary: ${effective.secondary}`)
  log('info', `Accent: ${effective.accent}`)
  log('info', `Border: ${effective.border}`)
  log('info', `Link: ${effective.link}`)
  log('info', `Success: ${effective.success}`)
  log('info', `Error: ${effective.error}`)
  log('info', `Warning: ${effective.warning}`)
  log('info', `Muted: ${effective.muted}`)
}

/**
 * List available themes.
 */
async function listThemes(): Promise<void> {
  const { names, active } = await listAllThemeNames()
  log('info', 'Available themes:')
  for (const name of names) {
    const marker = name === active ? ' (active)' : ''
    const kind = isBuiltin(name) ? 'built-in' : 'custom'
    log('info', `  • ${name} [${kind}]${marker}`)
  }
}

/**
 * Validate and normalize a single color override.
 */
function validateColorOverride(key: string, value: string): Record<ColorKey, string> {
  if (!isColorKey(key)) {
    throw new Error(
      `Invalid color key: ${key}. Valid keys: ${COLOR_KEYS.join(', ')}`,
    )
  }
  if (value.toLowerCase() === 'auto') {
    return { [key]: 'auto' } as Record<ColorKey, string>
  }
  if (!isHexColor(value)) {
    throw new Error(
      `Invalid color value for ${key}: ${value}. Use a hex code like #6C5CE7 or #fff.`,
    )
  }
  // Normalize short hex to full hex for consistency.
  const normalized =
    value.length === 4
      ? `#${value[1]}${value[1]}${value[2]}${value[2]}${value[3]}${value[3]}`
      : value.toLowerCase()
  return { [key]: normalized } as Record<ColorKey, string>
}

/**
 * Create or update a custom theme.
 */
async function createCustomTheme(args: string[]): Promise<void> {
  const name = args[0]
  if (!name) {
    log('error', 'Please specify a custom theme name')
    log('info', 'Usage: theme custom <name> [--base dark|light] [--<color> #hex] ...')
    log('info', `Colors: ${COLOR_KEYS.join(', ')}`)
    return
  }

  const flags = parseFlags(args.slice(1))
  const base = flags.base === 'light' ? 'light' : 'dark'
  delete flags.base

  const overrides: Partial<Record<ColorKey, string>> = {}
  for (const [key, value] of Object.entries(flags)) {
    const override = validateColorOverride(key, value)
    Object.assign(overrides, override)
  }

  const theme = await saveCustomTheme(name, overrides, base)
  log('success', `Custom theme saved: ${theme.name}`)
  log('info', `Use "theme set ${theme.name}" to activate it.`)
}

/**
 * Edit a single color in an existing custom theme.
 */
async function editPalette(args: string[]): Promise<void> {
  const name = args[0]
  if (!name) {
    log('error', 'Please specify a custom theme name')
    log('info', 'Usage: theme palette <name> --key <color> --value <hex>')
    return
  }

  const flags = parseFlags(args.slice(1))
  const colorKey = flags.key
  const colorValue = flags.value

  if (!colorKey || !colorValue) {
    log('error', 'Both --key and --value are required')
    log('info', 'Usage: theme palette <name> --key <color> --value <hex>')
    log('info', `Colors: ${COLOR_KEYS.join(', ')}`)
    return
  }

  if (isBuiltin(name as ThemeName)) {
    log('error', `Cannot edit built-in theme "${name}". Create a custom theme first.`)
    return
  }

  const customs = await loadCustomThemes()
  if (!customs[name]) {
    log('error', `Custom theme not found: ${name}`)
    return
  }

  const override = validateColorOverride(colorKey, colorValue)
  const theme = await saveCustomTheme(name, override, customs[name].base ?? 'dark')
  log('success', `Updated ${colorKey} for theme "${name}": ${theme[colorKey as ColorKey]}`)
}

/**
 * Remove a custom theme.
 */
async function removeTheme(args: string[]): Promise<void> {
  const name = args[0]
  if (!name) {
    log('error', 'Please specify a theme name to delete')
    return
  }

  try {
    await deleteCustomTheme(name)
    log('success', `Deleted custom theme: ${name}`)
  } catch (error) {
    if (error instanceof Error) {
      log('error', error.message)
    } else {
      log('error', 'Failed to delete theme')
    }
  }
}

/**
 * Execute theme command.
 */
export default async function themeCommand(args: string[]): Promise<void> {
  try {
    const subcommand = args[0] || 'show'

    switch (subcommand) {
      case 'show':
      case 'current':
        await showTheme()
        break

      case 'set': {
        const themeName = args[1]
        if (!themeName) {
          log('error', 'Please specify a theme name')
          await listThemes()
          return
        }
        await setTheme(themeName)
        log('success', `Theme set to: ${themeName}`)
        break
      }

      case 'list':
      case 'ls':
        await listThemes()
        break

      case 'custom':
        await createCustomTheme(args.slice(1))
        break

      case 'palette':
        await editPalette(args.slice(1))
        break

      case 'delete':
      case 'rm':
        await removeTheme(args.slice(1))
        break

      case 'dark':
        await setTheme('dark')
        log('success', 'Theme set to: dark')
        break

      case 'light':
        await setTheme('light')
        log('success', 'Theme set to: light')
        break

      case 'system':
        await setTheme('system')
        log('success', 'Theme set to: system')
        break

      default:
        // Try to use as theme name
        if (await getThemeByName(subcommand)) {
          await setTheme(subcommand)
          log('success', `Theme set to: ${subcommand}`)
        } else {
          log('error', `Unknown theme: ${subcommand}`)
          await listThemes()
        }
    }
  } catch (error) {
    if (error instanceof Error) {
      log('error', `Theme command failed: ${error.message}`)
    } else {
      log('error', 'Theme command failed with unknown error')
    }
  }
}

export { BUILTINS as THEMES }
