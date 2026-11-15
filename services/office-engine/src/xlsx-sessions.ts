/**
 * Workbook session endpoints — the HTTP form of the GenOffice sheets app's
 * `window.desktopApi` workbook surface. The browser bridge in
 * @allternit/office-sheets-app maps 1:1 onto these routes.
 *
 * Sessions are in-memory: opening stages the bytes in a temp dir, opens a
 * sidecar session, and tracks the sheet-name mapping the save pipeline needs
 * (session.ts, ported from the upstream Electron main process).
 */
import { Hono } from 'hono'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  openWorkbookSession,
  writeWorkbookTo,
  type WorkbookSessionInfo,
  type XlsxSidecarClient,
} from '@allternit/office-xlsx-engine'
import { workbookSaveRequestSchema } from '@allternit/office-xlsx-engine/shared/desktop-api'

interface SessionEntry {
  dir: string
  info: WorkbookSessionInfo
  lastUsed: number
}

export interface XlsxSessionLimits {
  /** Open sessions across everyone; past it, open answers 429. */
  maxSessions: number
  /** A session unused this long is closed and its files removed. */
  idleMs: number
  maxFileBytes: number
  /** What a file may claim to unpack to, so a zip bomb never reaches the parser. */
  maxUnpackedBytes: number
  now: () => number
}

const DEFAULT_LIMITS: XlsxSessionLimits = {
  maxSessions: 64,
  idleMs: 3 * 60 * 60 * 1000,
  maxFileBytes: 48 * 1024 * 1024,
  maxUnpackedBytes: 1024 * 1024 * 1024,
  now: Date.now,
}

const SWEEP_MS = 5 * 60 * 1000
const MAX_UNPACK_ENTRIES = 10_000

/**
 * What a zip's central directory claims it unpacks to, or null when it can't
 * be read as a plain zip (a workbook has no business being a zip64). Not a .xlsx
 * at all (.xls, .csv) is left to the parser.
 */
export function zipUnpackedBytes(file: Buffer): number | null {
  if (file.length < 22 || file.readUInt32LE(0) !== 0x04034b50) return 0
  let end = -1
  for (let at = file.length - 22; at >= Math.max(0, file.length - 22 - 0xffff); at -= 1) {
    if (file.readUInt32LE(at) === 0x06054b50) {
      end = at
      break
    }
  }
  if (end < 0) return null
  const count = file.readUInt16LE(end + 10)
  const start = file.readUInt32LE(end + 16)
  if (count === 0xffff || start === 0xffffffff || count > MAX_UNPACK_ENTRIES) return null
  let total = 0
  let at = start
  for (let n = 0; n < count; n += 1) {
    if (at + 46 > file.length || file.readUInt32LE(at) !== 0x02014b50) return null
    const size = file.readUInt32LE(at + 24)
    if (size === 0xffffffff) return null
    total += size
    at += 46 + file.readUInt16LE(at + 28) + file.readUInt16LE(at + 30) + file.readUInt16LE(at + 32)
  }
  return total
}

export function createXlsxSessionRouter(
  getClient: () => XlsxSidecarClient | null,
  limitOverrides: Partial<XlsxSessionLimits> = {},
): Hono {
  const limits = { ...DEFAULT_LIMITS, ...limitOverrides }
  const app = new Hono()
  const infos = new Map<string, WorkbookSessionInfo>()
  const entries = new Map<string, SessionEntry>()

  async function closeSession(sessionId: string): Promise<void> {
    const entry = entries.get(sessionId)
    entries.delete(sessionId)
    infos.delete(sessionId)
    await getClient()?.close(sessionId).catch(() => undefined)
    if (entry) await rm(entry.dir, { recursive: true, force: true }).catch(() => undefined)
  }

  async function closeIdleSessions(): Promise<void> {
    const cutoff = limits.now() - limits.idleMs
    for (const [sessionId, entry] of [...entries]) {
      if (entry.lastUsed <= cutoff) await closeSession(sessionId)
    }
  }

  setInterval(() => void closeIdleSessions(), SWEEP_MS).unref?.()

  /** The live session a request names; using it keeps it alive. */
  function useSession(sessionId: string): SessionEntry | undefined {
    const entry = entries.get(sessionId)
    if (entry) entry.lastUsed = limits.now()
    return entry
  }

  const unavailable = (c: { json: (body: unknown, status: 503) => Response }) =>
    c.json({ error: 'xlsx_sidecar_unavailable', detail: 'sidecar binary not found' }, 503)

  /** POST /open — `{ name, bytesBase64 }` → WorkbookFile (session metadata). */
  app.post('/open', async (c) => {
    const client = getClient()
    if (!client) return unavailable(c)
    let payload: { name?: string; bytesBase64?: string }
    try {
      payload = await c.req.json()
    } catch {
      return c.json({ error: 'invalid JSON body' }, 400)
    }
    if (!payload.bytesBase64) return c.json({ error: 'expected { name, bytesBase64 }' }, 400)

    const file = Buffer.from(payload.bytesBase64, 'base64')
    if (file.length > limits.maxFileBytes) return c.json({ error: 'workbook too large' }, 413)
    const unpacked = zipUnpackedBytes(file)
    if (unpacked === null || unpacked > limits.maxUnpackedBytes) {
      return c.json({ error: 'workbook too large to open' }, 413)
    }
    await closeIdleSessions()
    if (entries.size >= limits.maxSessions) return c.json({ error: 'engine busy' }, 429)

    const dir = await mkdtemp(join(tmpdir(), 'office-engine-session-'))
    try {
      const path = join(dir, (payload.name ?? 'workbook.xlsx').replace(/[/\\]/g, '_'))
      await writeFile(path, file)
      const opened = await openWorkbookSession(client, path, infos)
      const info = infos.get(opened.sessionId)
      if (!info) throw new Error('session info missing after open')
      entries.set(opened.sessionId, { dir, info, lastUsed: limits.now() })
      return c.json(opened)
    } catch (err) {
      await rm(dir, { recursive: true, force: true })
      return c.json({ error: 'open failed', detail: (err as Error).message }, 422)
    }
  })

  /** POST /range — `{ sessionId, sheetId, range }` → RangeResult */
  app.post('/range', async (c) => {
    const client = getClient()
    if (!client) return unavailable(c)
    const body = await c.req.json().catch(() => null)
    if (!body?.sessionId || !body?.sheetId || !body?.range) {
      return c.json({ error: 'expected { sessionId, sheetId, range }' }, 400)
    }
    if (!useSession(body.sessionId)) return c.json({ error: 'unknown session' }, 404)
    try {
      return c.json(
        await client.readRange({ sessionId: body.sessionId, sheetId: body.sheetId, range: body.range }),
      )
    } catch (err) {
      return c.json({ error: 'read failed', detail: (err as Error).message }, 422)
    }
  })

  /** POST /formulas — `{ sessionId, sheetId }` → formula cells */
  app.post('/formulas', async (c) => {
    const client = getClient()
    if (!client) return unavailable(c)
    const body = await c.req.json().catch(() => null)
    if (!body?.sessionId || !body?.sheetId) {
      return c.json({ error: 'expected { sessionId, sheetId }' }, 400)
    }
    if (!useSession(body.sessionId)) return c.json({ error: 'unknown session' }, 404)
    try {
      return c.json(await client.readFormulaCells({ sessionId: body.sessionId, sheetId: body.sheetId }))
    } catch (err) {
      return c.json({ error: 'read failed', detail: (err as Error).message }, 422)
    }
  })

  /** POST /session-recalc — `{ sessionId, edits, reads }` → recalculated cells */
  app.post('/session-recalc', async (c) => {
    const client = getClient()
    if (!client) return unavailable(c)
    const body = await c.req.json().catch(() => null)
    if (!body?.sessionId || !Array.isArray(body?.edits) || !Array.isArray(body?.reads)) {
      return c.json({ error: 'expected { sessionId, edits, reads }' }, 400)
    }
    const entry = useSession(body.sessionId)
    if (!entry) return c.json({ error: 'unknown session' }, 404)
    try {
      return c.json(
        await client.recalcCells({ path: entry.info.path, edits: body.edits, reads: body.reads }),
      )
    } catch (err) {
      return c.json({ error: 'recalc failed', detail: (err as Error).message }, 422)
    }
  })

  /**
   * POST /save — WorkbookSaveRequest (+ `includeBytes`) → saved WorkbookFile.
   * Applies the edit set through the streaming gateway save, swaps the
   * session onto the saved file, and returns the fresh session metadata plus
   * the new file bytes (base64) so the host can persist/download them.
   */
  app.post('/save', async (c) => {
    const client = getClient()
    if (!client) return unavailable(c)
    let parsed: ReturnType<typeof workbookSaveRequestSchema.parse>
    try {
      parsed = workbookSaveRequestSchema.parse(await c.req.json())
    } catch (err) {
      return c.json({ error: 'invalid save request', detail: (err as Error).message }, 400)
    }
    const entry = useSession(parsed.sessionId)
    if (!entry) return c.json({ error: 'unknown session' }, 404)

    try {
      const mutation = await writeWorkbookTo(client, entry.info, parsed, entry.info.path)
      // Swap the session onto the saved file so subsequent reads match disk.
      entries.delete(parsed.sessionId)
      await client.close(parsed.sessionId).catch(() => undefined)
      const file = await openWorkbookSession(client, entry.info.path, infos)
      const nextInfo = infos.get(file.sessionId)
      if (!nextInfo) throw new Error('session info missing after save')
      entries.set(file.sessionId, { dir: entry.dir, info: nextInfo, lastUsed: limits.now() })
      const bytes = await readFile(entry.info.path)
      return c.json({
        canceled: false,
        file,
        touchedEntries: mutation.touchedEntries,
        bytesBase64: bytes.toString('base64'),
      })
    } catch (err) {
      return c.json({ error: 'save failed', detail: (err as Error).message }, 422)
    }
  })

  /** POST /close — `{ sessionId }` → closes the sidecar session, drops temp files. */
  app.post('/close', async (c) => {
    const body = await c.req.json().catch(() => null)
    if (!body?.sessionId) return c.json({ error: 'expected { sessionId }' }, 400)
    await closeSession(body.sessionId)
    return c.json({ ok: true })
  })

  return app
}
