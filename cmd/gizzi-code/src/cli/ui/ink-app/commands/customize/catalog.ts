/**
 * /customize: the Customize catalog Desktop shows (A://TERNIT OFFICIAL shelf,
 * then the directory) and the same Add.
 *
 * - Skill: `POST /api/v1/skills/catalog/install {id}` on the local
 *   allternit-api (Desktop's installer: copies into ~/.agents/skills and
 *   reloads gizzi's skills).
 * - MCP server: writes `mcp.<key>` into gizzi.json, exactly like Desktop's
 *   writeGizziMcp (a gizzi.json that isn't plain JSON is never rewritten),
 *   and adds it to this terminal's user MCP config so the TUI loads it too.
 * - Command: writes `command/<name>.md` like Desktop's writeGizziCommand.
 *
 * catalog.json is generated from allternit-ai's Customize data; see its $comment.
 */
import { existsSync } from 'fs'
import { mkdir, readFile, writeFile } from 'fs/promises'
import { homedir } from 'os'
import { join } from 'path'
import catalog from './catalog.json'

export type CatalogAdd =
  | { skill: string }
  | { mcp: { key: string; gizzi?: boolean; args?: string[]; entry?: Record<string, unknown> } }
  | { command: { name: string; template: string } }

export type CatalogItem = {
  id: string
  name: string
  description: string
  kind: string
  author: string
  version?: string
  category?: string
  homepage?: string
  requires?: string
  add: CatalogAdd
}

export type CatalogShelf = { title: string; items: CatalogItem[] }
export type CatalogSection = { id: string; title: string; shelves: CatalogShelf[] }

export const SECTIONS = catalog.sections as CatalogSection[]

/** gizzi's config folder, the one Desktop writes (~/.config/gizzi-code). */
export function gizziConfigDir(home = homedir()): string {
  return process.env.GIZZI_CONFIG_DIR?.trim() || join(home, '.config', 'gizzi-code')
}

/** The exact `mcp` entry Add writes for an item (Desktop's directoryMcpEntry / official entry). */
export function mcpEntry(add: Extract<CatalogAdd, { mcp: unknown }>['mcp'], home = homedir()): Record<string, unknown> {
  if (add.gizzi) {
    // gizzi starts MCP commands without a shell, so the path is absolute (Desktop's gizziBinaryPath).
    const bin = process.platform === 'win32' ? 'gizzi' : join(home, '.local', 'bin', 'gizzi')
    return { type: 'local', command: [bin, ...(add.args ?? [])], enabled: true }
  }
  const entry = { ...(add.entry ?? {}) }
  if (Array.isArray(entry.command)) entry.command = (entry.command as string[]).map(a => (a === '{home}' ? home : a))
  return entry
}

/** What Add writes or calls, shown on the item page. */
export function addsWhat(item: CatalogItem): string {
  const add = item.add
  if ('skill' in add) return 'Installs into ~/.agents/skills (Allternit\'s skill installer), then reloads skills'
  if ('mcp' in add) return `Adds "${add.mcp.key}" to mcp in ~/.config/gizzi-code/gizzi.json: ${JSON.stringify(mcpEntry(add.mcp, '~'))}`
  return `Writes ~/.config/gizzi-code/command/${add.command.name}.md, then type /${add.command.name}`
}

async function readGizziJson(file: string): Promise<Record<string, unknown>> {
  if (!existsSync(file)) return {}
  const raw = await readFile(file, 'utf8')
  try {
    const parsed = JSON.parse(raw) as unknown
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) return parsed as Record<string, unknown>
    return {}
  } catch {
    throw new Error(`${file} isn't plain JSON, so it wasn't changed. Add the server with: gizzi mcp add`)
  }
}

/** Desktop's writeGizziMcp. Returns the file written. */
export async function writeGizziMcp(key: string, entry: Record<string, unknown>, dir = gizziConfigDir()): Promise<string> {
  await mkdir(dir, { recursive: true })
  const file = join(dir, 'gizzi.json')
  const config = await readGizziJson(file)
  const mcp = config.mcp && typeof config.mcp === 'object' && !Array.isArray(config.mcp) ? (config.mcp as Record<string, unknown>) : {}
  config.mcp = { ...mcp, [key]: entry }
  await writeFile(file, JSON.stringify(config, null, 2) + '\n')
  return file
}

/** Desktop's writeGizziCommand. Returns the file written. */
export async function writeGizziCommand(name: string, description: string, template: string, dir = gizziConfigDir()): Promise<string> {
  const folder = join(dir, 'command')
  await mkdir(folder, { recursive: true })
  const file = join(folder, `${name}.md`)
  const desc = description.replace(/\r?\n/g, ' ').replace(/"/g, '\\"')
  await writeFile(file, `---\ndescription: "${desc}"\n---\n\n${template.trim()}\n`)
  return file
}

/** Which items are already added: installed skills, gizzi.json mcp keys, command files. */
export async function addedIds(installedSkills: string[], dir = gizziConfigDir()): Promise<Set<string>> {
  const added = new Set<string>(installedSkills)
  const config = await readGizziJson(join(dir, 'gizzi.json')).catch(() => ({}) as Record<string, unknown>)
  const mcp = (config.mcp ?? {}) as Record<string, unknown>
  for (const section of SECTIONS) {
    for (const shelf of section.shelves) {
      for (const item of shelf.items) {
        const add = item.add
        if ('mcp' in add && add.mcp.key in mcp) added.add(item.id)
        if ('command' in add && existsSync(join(dir, 'command', `${add.command.name}.md`))) added.add(item.id)
      }
    }
  }
  return added
}

/** The TUI's own MCP config shape for a gizzi.json entry. */
export function tuiMcpConfig(entry: Record<string, unknown>): Record<string, unknown> | undefined {
  if (typeof entry.url === 'string') return { type: 'http', url: entry.url }
  if (Array.isArray(entry.command) && entry.command.length > 0) {
    const [command, ...args] = entry.command as string[]
    return { type: 'stdio', command, args }
  }
  return undefined
}
