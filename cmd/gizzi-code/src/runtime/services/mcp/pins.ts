// MCP tool pinning. Stores a sha256 of (name, description, inputSchema, annotations)
// per tool, per server. Once a server is pinned, a new tool or a changed one is held
// out of the tool list until the owner re-approves it (same rule as the app side's
// `mcp_directory_held.rs` / `mcp_tool_pins`). The first sight of a server pins what it
// lists (trust on first use); only later changes and additions are held.
import { createHash } from 'crypto'
import { mkdirSync, readFileSync, writeFileSync } from 'fs'
import { dirname, join } from 'path'
import { homedir } from 'os'

export type PinnableTool = {
  name: string
  description?: string
  inputSchema?: unknown
  annotations?: unknown
}

export type PinStore = Record<string, Record<string, string>>

export type PinCheck<T extends PinnableTool> = {
  allowed: T[]
  held: Array<{ tool: string; reason: 'new' | 'changed' }>
}

function canonical(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(canonical)
  if (v && typeof v === 'object') {
    return Object.fromEntries(
      Object.keys(v as object)
        .sort()
        .map(k => [k, canonical((v as Record<string, unknown>)[k])]),
    )
  }
  return v ?? null
}

export function toolHash(t: PinnableTool): string {
  const body = JSON.stringify(
    canonical([t.name, t.description, t.inputSchema, t.annotations]),
  )
  return createHash('sha256').update(body).digest('hex')
}

function pinsPath(): string {
  return join(process.env.GIZZI_CONFIG_DIR ?? join(homedir(), '.gizzi'), 'mcp-tool-pins.json')
}

export function loadPins(path = pinsPath()): PinStore {
  try {
    return JSON.parse(readFileSync(path, 'utf8')) as PinStore
  } catch {
    return {}
  }
}

export function savePins(store: PinStore, path = pinsPath()): void {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, JSON.stringify(store, null, 2))
}

/** Pure check against `store`; pins a server's first listing in `store`. */
export function checkTools<T extends PinnableTool>(
  store: PinStore,
  server: string,
  tools: T[],
): PinCheck<T> & { dirty: boolean } {
  const pinned = store[server]
  if (!pinned) {
    store[server] = Object.fromEntries(tools.map(t => [t.name, toolHash(t)]))
    return { allowed: tools, held: [], dirty: true }
  }
  const allowed: T[] = []
  const held: PinCheck<T>['held'] = []
  for (const t of tools) {
    const h = pinned[t.name]
    if (h === toolHash(t)) allowed.push(t)
    else held.push({ tool: t.name, reason: h ? 'changed' : 'new' })
  }
  return { allowed, held, dirty: false }
}

/** Approve: pin the tools as they are listed now. */
export function approveTools(
  store: PinStore,
  server: string,
  tools: PinnableTool[],
): void {
  store[server] = { ...(store[server] ?? {}) }
  for (const t of tools) store[server]![t.name] = toolHash(t)
}

/** `GIZZI_MCP_APPROVE_TOOLS=<server>[,<server>…]` (or `*`) approves held tools on the next load. */
function approvalRequested(server: string): boolean {
  const raw = process.env.GIZZI_MCP_APPROVE_TOOLS
  if (!raw) return false
  const names = raw.split(',').map(x => x.trim())
  return names.includes('*') || names.includes(server)
}

/** Load, check and persist. Returns the allowed tools and what was held. */
export function applyPins<T extends PinnableTool>(
  server: string,
  tools: T[],
  path = pinsPath(),
): PinCheck<T> {
  const store = loadPins(path)
  const res = checkTools(store, server, tools)
  if (res.held.length > 0 && approvalRequested(server)) {
    // The owner asked for this server's tools to be approved: re-pin and allow.
    approveTools(store, server, tools)
    savePins(store, path)
    return { allowed: tools, held: [] }
  }
  if (res.dirty) savePins(store, path)
  return { allowed: res.allowed, held: res.held }
}

/** Re-approve a server's held tools (`gizzi mcp approve-tools <server>`). */
export function approvePins(server: string, tools: PinnableTool[], path = pinsPath()): void {
  const store = loadPins(path)
  approveTools(store, server, tools)
  savePins(store, path)
}
