/**
 * Office Engine
 *
 * Exposes GenOffice engine capabilities behind a small HTTP service:
 * docx/pptx parse + round-trip, unified text extraction, and xlsx
 * reading/recalculation via the vendored Rust sidecar. The Rust gateway
 * proxies these routes under /api/office/*.
 */

import { pathToFileURL } from 'node:url'
import { Hono } from 'hono'
import { parseDocx, saveDocx, type ParsedDocFull, type SaveBlock } from '@allternit/office-docx-engine'
import {
  openPptx,
  savePptx,
  type Slide,
  type TextElement,
} from '@allternit/office-pptx-engine'
import {
  resolveSidecarBinary,
  XlsxSidecarClient,
  type OpenWorkbookResult,
  type RecalcEdit,
  type RecalcRead,
} from '@allternit/office-xlsx-engine'
import { docxToText, parseFileToText, xlsxToText } from '@allternit/office-file-parse'
import { anydocStatus, convertToMarkdown, MarkdownConversionError } from './markdown'
import { convertUrlToMarkdown } from './markdown-url'
import { createXlsxSessionRouter } from './xlsx-sessions'
import { randomUUID } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { extname, join } from 'node:path'

const app = new Hono()

// The hosted engine serves workbook sessions and nothing else. The other routes
// assume one trusted local user (`/markdown-url` fetches URLs from this machine).
if (process.env.OFFICE_ENGINE_SESSIONS_ONLY === '1') {
  app.use('*', async (c, next) => {
    const path = c.req.path
    if (path === '/health' || path.startsWith('/xlsx/session/')) return next()
    return c.json({ error: 'not_available' }, 404)
  })
}

const PPTX_MIME = 'application/vnd.openxmlformats-officedocument.presentationml.presentation'
const XLSX_MIME = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'

/** Lazily created xlsx sidecar client; null when the binary is not built/found. */
let xlsxClient: XlsxSidecarClient | null | undefined

function getXlsxClient(): XlsxSidecarClient | null {
  if (xlsxClient === undefined) {
    const binary = resolveSidecarBinary()
    if (binary) {
      xlsxClient = new XlsxSidecarClient(binary)
      xlsxClient.start()
    } else {
      xlsxClient = null
    }
  }
  return xlsxClient
}

const MIME_BY_EXT: Record<string, string> = {
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  pptx: PPTX_MIME,
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  pdf: 'application/pdf',
  txt: 'text/plain',
  md: 'text/markdown',
  csv: 'text/csv',
  json: 'application/json',
  xml: 'application/xml',
  html: 'text/html',
  htm: 'text/html',
  log: 'text/plain',
  tsv: 'text/tab-separated-values',
  markdown: 'text/markdown',
}

app.get('/health', (c) =>
  c.json({
    status: 'ok',
    service: 'office-engine',
    version: '0.1.0',
    engines: {
      docx: '@allternit/office-docx-engine',
      pptx: '@allternit/office-pptx-engine',
      'file-parse': '@allternit/office-file-parse',
      xlsx: getXlsxClient()
        ? '@allternit/office-xlsx-engine'
        : 'unavailable (sidecar binary not found; run `pnpm --filter @allternit/office-xlsx-engine sidecar:build`)',
      anydoc: anydocStatus(),
    },
  }),
)

/**
 * POST /parse
 * Body: raw .docx bytes
 * Headers: x-office-filename
 * Returns: artifact-shaped JSON with extracted text and metadata.
 */
app.post('/parse', async (c) => {
  const filename = c.req.header('x-office-filename') ?? 'unknown.docx'
  const bytes = await c.req.arrayBuffer()

  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const buffer = new Uint8Array(bytes)

  let text = ''
  let paragraphCount = 0
  let tableCount = 0
  let title = filename

  try {
    const doc = await parseDocx(buffer)
    for (const block of doc.blocks) {
      if (block.type === 'paragraph') paragraphCount++
      if (block.type === 'table') tableCount++
    }
    title = extractTitle(doc) ?? filename
    text = await docxToText(buffer)
  } catch (err) {
    return c.json({ error: 'parse failed', detail: (err as Error).message }, 422)
  }

  const artifact = {
    id: randomUUID(),
    type: 'office-document',
    title,
    filename,
    mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    sizeBytes: buffer.byteLength,
    extractedText: text,
    stats: {
      paragraphCount,
      tableCount,
      textLength: text.length,
    },
    engine: {
      name: '@allternit/office-docx-engine',
      phase: 'prototype',
    },
    createdAt: new Date().toISOString(),
  }

  return c.json(artifact)
})

/**
 * POST /docx/roundtrip
 * Body: raw .docx bytes
 * Returns: { originalSize, outputSize, changed: boolean }
 */
app.post('/docx/roundtrip', async (c) => {
  const bytes = await c.req.arrayBuffer()
  if (bytes.byteLength === 0) return c.json({ error: 'empty body' }, 400)

  const buffer = new Uint8Array(bytes)
  try {
    const doc = await parseDocx(buffer)
    const full = doc as ParsedDocFull
    const visibleBlocks = doc.blocks.filter((b) => !b.hidden && b.docxIndex != null)
    const saveBlocks: SaveBlock[] = visibleBlocks.map((b) => ({ kind: 'original', docxIndex: b.docxIndex! }))
    const output = await saveDocx(full, saveBlocks)
    return c.json({
      originalSize: buffer.byteLength,
      outputSize: output.byteLength,
      changed: buffer.byteLength !== output.byteLength,
    })
  } catch (err) {
    return c.json({ error: 'roundtrip failed', detail: (err as Error).message }, 422)
  }
})

/**
 * POST /pptx/parse
 * Body: raw .pptx bytes
 * Headers: x-office-filename
 * Returns: artifact-shaped JSON with per-slide extracted text and metadata.
 */
app.post('/pptx/parse', async (c) => {
  const filename = c.req.header('x-office-filename') ?? 'unknown.pptx'
  const bytes = await c.req.arrayBuffer()

  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const buffer = new Uint8Array(bytes)

  try {
    const { deck } = await openPptx(buffer)
    const slideTexts = deck.slides.map((slide, i) =>
      [`## Slide ${i + 1}`, ...slideTextLines(slide)].join('\n'),
    )
    const text = slideTexts.join('\n\n')
    const title = slideTextLines(deck.slides[0])[0] ?? filename

    return c.json({
      id: randomUUID(),
      type: 'office-presentation',
      title,
      filename,
      mimeType: PPTX_MIME,
      sizeBytes: buffer.byteLength,
      extractedText: text,
      stats: {
        slideCount: deck.slides.length,
        textLength: text.length,
      },
      engine: {
        name: '@allternit/office-pptx-engine',
        phase: 'prototype',
      },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    return c.json({ error: 'parse failed', detail: (err as Error).message }, 422)
  }
})

/**
 * POST /pptx/roundtrip
 * Body: raw .pptx bytes
 * Returns: { originalSize, outputSize, changed: boolean }
 * Parses the deck and saves it back with no edits (untouched entries are
 * written back byte-for-byte; the zip container itself is re-compressed).
 */
app.post('/pptx/roundtrip', async (c) => {
  const bytes = await c.req.arrayBuffer()
  if (bytes.byteLength === 0) return c.json({ error: 'empty body' }, 400)

  const buffer = new Uint8Array(bytes)
  try {
    const opened = await openPptx(buffer)
    const output = await savePptx(opened)
    return c.json({
      originalSize: buffer.byteLength,
      outputSize: output.byteLength,
      changed: buffer.byteLength !== output.byteLength,
    })
  } catch (err) {
    return c.json({ error: 'roundtrip failed', detail: (err as Error).message }, 422)
  }
})

/**
 * POST /extract
 * Body: raw bytes of any supported format (pdf/docx/pptx/xlsx/txt/md/csv/...)
 * Headers: x-office-filename (the extension drives format detection)
 * Returns: artifact-shaped JSON with the extracted plain text.
 */
app.post('/extract', async (c) => {
  const filename = c.req.header('x-office-filename') ?? 'unknown'
  const bytes = await c.req.arrayBuffer()

  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const buffer = new Uint8Array(bytes)
  const ext = extname(filename).slice(1).toLowerCase()

  // parseFileToText works from a path (pdfjs-dist needs random access), so
  // stage the bytes in a temp file carrying the original extension.
  const dir = await mkdtemp(join(tmpdir(), 'office-engine-'))
  try {
    const staged = join(dir, `upload.${ext || 'bin'}`)
    await writeFile(staged, buffer)
    const parsed = await parseFileToText(staged)

    if (!parsed.ok || parsed.kind !== 'text' || parsed.text == null) {
      return c.json(
        { error: 'extraction failed', detail: parsed.error ?? `no text extraction for .${ext || 'unknown'}` },
        422,
      )
    }

    return c.json({
      id: randomUUID(),
      type: 'text-extraction',
      filename,
      mimeType: parsed.mime ?? MIME_BY_EXT[ext] ?? 'application/octet-stream',
      sizeBytes: buffer.byteLength,
      extractedText: parsed.text,
      stats: {
        textLength: parsed.text.length,
      },
      engine: {
        name: '@allternit/office-file-parse',
        phase: 'prototype',
      },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    return c.json({ error: 'extraction failed', detail: (err as Error).message }, 422)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

/**
 * POST /markdown
 * Body: raw bytes of any anydoc-supported format (docx/pptx/xlsx/pdf/csv/...)
 * Headers: x-office-filename (the extension drives format detection)
 * Returns: artifact-shaped JSON with the converted GFM Markdown.
 * The LLM-ready sibling of /extract: 400 empty body, 415 unsupported format,
 * 422 conversion failure, 503 when the anydoc napi binding failed to load.
 */
app.post('/markdown', async (c) => {
  const filename = c.req.header('x-office-filename') ?? 'unknown'
  const bytes = await c.req.arrayBuffer()

  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const buffer = new Uint8Array(bytes)

  try {
    const converted = await convertToMarkdown(buffer, filename)
    return c.json({
      id: randomUUID(),
      type: 'markdown-conversion',
      title: converted.title ?? filename,
      filename,
      mimeType: 'text/markdown',
      sizeBytes: buffer.byteLength,
      markdown: converted.markdown,
      format: converted.format,
      stats: {
        textLength: converted.markdown.length,
      },
      engine: {
        name: '@firecrawl/anydoc',
        phase: 'prototype',
      },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    if (err instanceof MarkdownConversionError) {
      if (err.code === 'unavailable') {
        return c.json({ error: 'anydoc_unavailable', code: err.code, detail: err.message }, 503)
      }
      if (err.code === 'unsupported') {
        return c.json({ error: 'unsupported format', code: err.code, detail: err.message }, 415)
      }
      return c.json({ error: 'conversion failed', code: err.code, detail: err.message }, 422)
    }
    return c.json({ error: 'conversion failed', detail: (err as Error).message }, 422)
  }
})

/**
 * POST /markdown-url
 * Body: JSON `{ url }`
 * Returns: artifact-shaped JSON with the converted GFM Markdown, `sourceUrl`
 * (after redirects) and detected `format` ('html' for pages; the document
 * format when the URL serves a pdf/docx/... passed through the anydoc path).
 * 400 invalid/blocked URL (SSRF guard), 415 unsupported content-type,
 * 422 fetch/parse/conversion failure.
 */
app.post('/markdown-url', async (c) => {
  let payload: { url?: string }
  try {
    payload = await c.req.json()
  } catch {
    return c.json({ error: 'invalid JSON body', code: 'invalidUrl', detail: 'expected { url }' }, 400)
  }
  if (!payload.url || typeof payload.url !== 'string') {
    return c.json({ error: 'missing url', code: 'invalidUrl', detail: 'expected { url }' }, 400)
  }

  try {
    const converted = await convertUrlToMarkdown(payload.url)
    return c.json({
      id: randomUUID(),
      type: 'markdown-conversion',
      title: converted.title ?? converted.sourceUrl,
      sourceUrl: converted.sourceUrl,
      mimeType: 'text/markdown',
      markdown: converted.markdown,
      format: converted.format,
      stats: {
        textLength: converted.markdown.length,
      },
      engine: {
        name: converted.format === 'html' ? 'readability+turndown' : '@firecrawl/anydoc',
        phase: 'prototype',
      },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    if (err instanceof MarkdownConversionError) {
      if (err.code === 'invalidUrl' || err.code === 'blockedUrl') {
        return c.json({ error: 'invalid url', code: err.code, detail: err.message }, 400)
      }
      if (err.code === 'unavailable') {
        return c.json({ error: 'anydoc_unavailable', code: err.code, detail: err.message }, 503)
      }
      if (err.code === 'unsupported') {
        return c.json({ error: 'unsupported format', code: err.code, detail: err.message }, 415)
      }
      return c.json({ error: 'conversion failed', code: err.code, detail: err.message }, 422)
    }
    return c.json({ error: 'conversion failed', detail: (err as Error).message }, 422)
  }
})

/**
 * POST /xlsx/parse
 * Body: raw .xlsx bytes
 * Headers: x-office-filename
 * Returns: artifact-shaped JSON with sheet metadata and extracted text.
 */
/**
 * POST /deliverable/render
 * Body: JSON `{ template: "report" | "sheet" | "deck", title, markdown }`
 * Returns: `{ ext, mimeType, bytesBase64 }` — a finished office document the
 * caller persists and serves (consumer-packaged Cowork P3.1 artifact
 * pipeline). Templates the agent fills; generation is deliberately minimal
 * OOXML (the office engines handle the rich round-trip on open).
 */
app.post('/deliverable/render', async (c) => {
  const body = await c.req.json().catch(() => null)
  const template = String(body?.template ?? 'report')
  const title = String(body?.title ?? 'Deliverable')
  const markdown = String(body?.markdown ?? '')
  if (!markdown.trim()) return c.json({ error: 'markdown is required' }, 400)
  try {
    const { buildDocx, buildXlsx, buildPptx } = await import('./deliverable-generators')
    const [ext, mimeType, bytes] =
      template === 'sheet'
        ? ['xlsx', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', await buildXlsx(markdown, title)]
        : template === 'deck'
          ? ['pptx', 'application/vnd.openxmlformats-officedocument.presentationml.presentation', await buildPptx(markdown, title)]
          : ['docx', 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', await buildDocx(markdown, title)]
    return c.json({
      template,
      ext,
      mimeType,
      sizeBytes: bytes.byteLength,
      bytesBase64: Buffer.from(bytes).toString('base64'),
      engine: { name: 'deliverable-generators', phase: 'minimal-ooxml' },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    return c.json({ error: 'render failed', detail: (err as Error).message }, 422)
  }
})

app.post('/xlsx/parse', async (c) => {
  const filename = c.req.header('x-office-filename') ?? 'unknown.xlsx'
  const bytes = await c.req.arrayBuffer()
  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const client = getXlsxClient()
  if (!client) {
    return c.json({ error: 'xlsx_sidecar_unavailable', detail: 'sidecar binary not found' }, 503)
  }

  const buffer = new Uint8Array(bytes)
  const dir = await mkdtemp(join(tmpdir(), 'office-engine-xlsx-'))
  let sessionId: string | null = null
  try {
    const staged = join(dir, 'workbook.xlsx')
    await writeFile(staged, buffer)

    const opened: OpenWorkbookResult = await client.open(staged)
    sessionId = opened.sessionId
    const sheets = (opened.sheets ?? []).map((s) => ({
      id: s.id,
      name: s.name,
      ...(s as unknown as Record<string, unknown>),
    }))
    const text = await xlsxToText(buffer)

    return c.json({
      id: randomUUID(),
      type: 'office-spreadsheet',
      title: sheets[0]?.name ?? filename,
      filename,
      mimeType: XLSX_MIME,
      sizeBytes: buffer.byteLength,
      extractedText: text,
      stats: {
        sheetCount: sheets.length,
        textLength: text.length,
      },
      sheets,
      engine: {
        name: '@allternit/office-xlsx-engine',
        phase: 'sidecar',
      },
      createdAt: new Date().toISOString(),
    })
  } catch (err) {
    return c.json({ error: 'parse failed', detail: (err as Error).message }, 422)
  } finally {
    if (sessionId) {
      await client.close(sessionId).catch(() => {})
    }
    await rm(dir, { recursive: true, force: true })
  }
})

/**
 * POST /xlsx/read
 * Body: raw .xlsx bytes
 * Headers: x-office-filename
 * Returns: `{ sheets: [{id, name, rowCount, columnCount, cells}] }` — the full
 * used range of every sheet (capped at 1000 rows × 20 columns per sheet to
 * stay under the sidecar's 20k-cell response limit), for grid rendering.
 */
const XLSX_READ_MAX_ROWS = 1000
const XLSX_READ_MAX_COLS = 20

app.post('/xlsx/read', async (c) => {
  const bytes = await c.req.arrayBuffer()
  if (bytes.byteLength === 0) {
    return c.json({ error: 'empty body' }, 400)
  }

  const client = getXlsxClient()
  if (!client) {
    return c.json({ error: 'xlsx_sidecar_unavailable', detail: 'sidecar binary not found' }, 503)
  }

  const dir = await mkdtemp(join(tmpdir(), 'office-engine-read-'))
  let sessionId: string | null = null
  try {
    const staged = join(dir, 'workbook.xlsx')
    await writeFile(staged, new Uint8Array(bytes))

    const opened: OpenWorkbookResult = await client.open(staged)
    sessionId = opened.sessionId

    const sheets: unknown[] = []
    for (const sheet of opened.sheets ?? []) {
      const meta = sheet as unknown as {
        id: string
        name: string
        rowCount?: number
        columnCount?: number
      }
      const rowCount = meta.rowCount ?? 0
      const columnCount = meta.columnCount ?? 0
      let cells: unknown[] = []
      let truncated = false
      if (rowCount > 0 && columnCount > 0) {
        const endRow = Math.min(rowCount, XLSX_READ_MAX_ROWS) - 1
        const endColumn = Math.min(columnCount, XLSX_READ_MAX_COLS) - 1
        truncated = rowCount > XLSX_READ_MAX_ROWS || columnCount > XLSX_READ_MAX_COLS
        const range = (await client.readRange({
          sessionId: opened.sessionId,
          sheetId: meta.id,
          range: { startRow: 0, endRow, startColumn: 0, endColumn },
        })) as { cells?: unknown[] }
        cells = range.cells ?? []
      }
      sheets.push({
        id: meta.id,
        name: meta.name,
        rowCount,
        columnCount,
        truncated,
        cells,
      })
    }

    return c.json({ sheets })
  } catch (err) {
    return c.json({ error: 'read failed', detail: (err as Error).message }, 422)
  } finally {
    if (sessionId) {
      await client.close(sessionId).catch(() => {})
    }
    await rm(dir, { recursive: true, force: true })
  }
})

/**
 * POST /xlsx/recalc
 * Body: JSON envelope `{ workbookBase64, edits, reads }` — the workbook is
 * loaded by the sidecar, edits are applied, formulas are recalculated with
 * IronCalc, and the requested cells' computed values are returned.
 */
app.post('/xlsx/recalc', async (c) => {
  let payload: { workbookBase64?: string; edits?: RecalcEdit[]; reads?: RecalcRead[] }
  try {
    payload = await c.req.json()
  } catch {
    return c.json({ error: 'invalid JSON body' }, 400)
  }
  if (!payload.workbookBase64 || !Array.isArray(payload.edits) || !Array.isArray(payload.reads)) {
    return c.json({ error: 'expected { workbookBase64, edits, reads }' }, 400)
  }

  const client = getXlsxClient()
  if (!client) {
    return c.json({ error: 'xlsx_sidecar_unavailable', detail: 'sidecar binary not found' }, 503)
  }

  const dir = await mkdtemp(join(tmpdir(), 'office-engine-recalc-'))
  try {
    const staged = join(dir, 'workbook.xlsx')
    await writeFile(staged, Buffer.from(payload.workbookBase64, 'base64'))
    const result = await client.recalcCells({
      path: staged,
      edits: payload.edits,
      reads: payload.reads,
    })
    return c.json(result as Record<string, unknown>)
  } catch (err) {
    return c.json({ error: 'recalc failed', detail: (err as Error).message }, 422)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

/** Visible text lines of one slide: one line per non-empty paragraph, in element order. */
function slideTextLines(slide: Slide | undefined): string[] {
  if (!slide) return []
  const lines: string[] = []
  for (const el of slide.elements) {
    if (el.type !== 'text' && el.type !== 'shape') continue
    const body = (el as TextElement).text
    if (!body) continue
    for (const para of body.paragraphs) {
      const line = para.runs
        .map((r) => r.text)
        .join('')
        .trim()
      if (line) lines.push(line)
    }
  }
  return lines
}

function extractTitle(doc: { blocks: { type: string; runs?: { text: string }[] }[] }): string | undefined {
  for (const block of doc.blocks) {
    if (block.type === 'paragraph' || block.type === 'heading') {
      const text = block.runs?.map((r) => r.text).join('').trim()
      if (text) return text
    }
  }
  return undefined
}

const port = Number(process.env.OFFICE_ENGINE_PORT ?? 8099)

// Workbook session API (sheets app): /xlsx/session/open|range|formulas|session-recalc|save|close
app.route('/xlsx/session', createXlsxSessionRouter(getXlsxClient))

export default {
  port,
  fetch: app.fetch,
}

// Direct Node startup when run as a script. Packaged Electron
// `ELECTRON_RUN_AS_NODE=1` + esbuild bundle makes import.meta.url a
// file:// URL that does not equal argv[1], so the old exact match never
// listened and the sidecar exited 0.
function isDirectOfficeEngineRun(): boolean {
  const entry = process.argv[1]
  if (!entry) return false
  if (process.env.OFFICE_ENGINE_PORT) return true
  try {
    return import.meta.url === pathToFileURL(entry).href
  } catch {
    return entry.includes('office-engine')
  }
}

if (isDirectOfficeEngineRun()) {
  const { serve } = await import('@hono/node-server')
  serve({ fetch: app.fetch, hostname: process.env.OFFICE_ENGINE_HOST || '127.0.0.1', port })
  console.log(`Office engine prototype listening on http://127.0.0.1:${port}`)
}
