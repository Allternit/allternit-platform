// @ts-nocheck
/**
 * Errors a language server reports right after an edit, for the edit's tool
 * line. Both the TUI's own Edit/Write tools and edits a CLI brain made itself
 * (cliObserved) call trackEditDiagnostics(); the tool line subscribes with
 * useSyncExternalStore and fills in when the server publishes.
 *
 * Never blocks a turn: tracking returns at once, servers start lazily on the
 * first edit of a file they handle, and a file no installed server handles
 * resolves to nothing.
 */
import { randomUUID } from 'crypto'
import { readFile } from 'fs/promises'
import path from 'path'
import type { DiagnosticFile } from '../diagnosticTracking.js'
import { logForDebugging } from '../../utils/debug.js'
import { lspDiagnosticsEnabled } from './builtinServers.js'
import { clearDeliveredDiagnosticsForFile } from './LSPDiagnosticRegistry.js'
import { getLspServerManager, waitForInitialization } from './manager.js'

export type EditLspError = { line: number; col: number; message: string; code?: string; source?: string }
export type EditDiagnostics = {
  filePath: string
  state: 'waiting' | 'done'
  errors: EditLspError[]
  /** True when the file had a published error list before this edit. */
  hadBaseline: boolean
}

/** How long a tool line waits for a server (first TypeScript start can take a while). */
const WAIT_MS = 20_000
const MAX_ENTRIES = 300

const entries = new Map<string, EditDiagnostics>()
const waitingByPath = new Map<string, string>()
const lastErrorKeys = new Map<string, Set<string>>()
const listeners = new Set<() => void>()

function emit(): void {
  for (const l of listeners) l()
}

function finish(key: string, errors: EditLspError[], hadBaseline: boolean): void {
  const entry = entries.get(key)
  if (!entry || entry.state === 'done') return
  entries.set(key, { ...entry, state: 'done', errors, hadBaseline })
  if (waitingByPath.get(entry.filePath) === key) waitingByPath.delete(entry.filePath)
  emit()
}

/**
 * Tell the file's language server about an edit (didChange + didSave) and
 * start waiting for its diagnostics. Returns a key for the tool line, or
 * undefined when diagnostics are turned off.
 */
export function trackEditDiagnostics(filePath: string, content?: string): string | undefined {
  if (!lspDiagnosticsEnabled() || !filePath) return undefined
  const abs = path.resolve(filePath)
  const key = randomUUID()
  const previous = waitingByPath.get(abs)
  if (previous) finish(previous, [], false)
  entries.set(key, { filePath: abs, state: 'waiting', errors: [], hadBaseline: false })
  waitingByPath.set(abs, key)
  if (entries.size > MAX_ENTRIES) entries.delete(entries.keys().next().value)

  void (async () => {
    await waitForInitialization()
    const manager = getLspServerManager()
    if (!manager?.getServerForFile(abs)) return finish(key, [], false)
    const text = content ?? (await readFile(abs, 'utf8'))
    clearDeliveredDiagnosticsForFile(`file://${abs}`)
    await manager.changeFile(abs, text)
    await manager.saveFile(abs)
    setTimeout(() => finish(key, [], false), WAIT_MS).unref?.()
  })().catch((err: Error) => {
    logForDebugging(`LSP: diagnostics after edit failed for ${abs}: ${err?.message}`)
    finish(key, [], false)
  })
  return key
}

const errorKey = (d: { message: string; code?: string }) => `${d.code ?? ''}|${d.message}`

/** Called for every publishDiagnostics (passiveFeedback), empty lists included. */
export function noteLspPublish(files: DiagnosticFile[]): void {
  for (const file of files) {
    const abs = path.resolve(file.uri.replace(/^file:\/\//, ''))
    const errors = file.diagnostics.filter(d => d.severity === 'Error')
    const prev = lastErrorKeys.get(abs)
    lastErrorKeys.set(abs, new Set(errors.map(errorKey)))
    const key = waitingByPath.get(abs)
    if (!key) continue
    const fresh = prev ? errors.filter(d => !prev.has(errorKey(d))) : errors
    finish(
      key,
      fresh.map(d => ({
        line: d.range.start.line + 1,
        col: d.range.start.character + 1,
        message: d.message,
        code: d.code,
        source: d.source,
      })),
      !!prev,
    )
  }
}

export function getEditDiagnostics(key: string | undefined): EditDiagnostics | undefined {
  return key ? entries.get(key) : undefined
}

export function subscribeEditDiagnostics(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** One line for the tool row: "2 new errors · L3:7 Type 'string' is not … (+1 more)". */
export function formatEditDiagnostics(d: EditDiagnostics | undefined): string | undefined {
  if (!d || d.state !== 'done' || d.errors.length === 0) return undefined
  const n = d.errors.length
  const first = d.errors[0]!
  const msg = first.message.split('\n')[0]!.slice(0, 120)
  const more = n > 1 ? ` (+${n - 1} more)` : ''
  return `${n} ${d.hadBaseline ? 'new ' : ''}error${n === 1 ? '' : 's'} · L${first.line}:${first.col} ${msg}${more}`
}
