/**
 * CSV for agent_batch (RFC 4180, read leniently, written strictly).
 *
 * Reading: a UTF-8 byte-order mark is dropped; fields may be quoted, a quoted
 * field may hold commas, doubled quotes and line breaks; lines end in \n, \r\n
 * or \r. A quote inside an unquoted field is kept as text. An unclosed quote
 * is an error naming the line it opened on.
 *
 * Writing: a field is quoted when it holds a comma, quote, line break or
 * leading/trailing space; lines end in \r\n.
 *
 * Free of runtime state, so the Ink adapter and bun tests can use it.
 */

export const BOM = "﻿"

export type CsvTable = {
  header: string[]
  /** Data rows, each padded to the header's width. Blank lines are dropped. */
  rows: string[][]
  /** The 1-based line each data row starts on (for error messages). */
  lines: number[]
  /** True when the text started with a byte-order mark. */
  bom: boolean
}

export class CsvError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "CsvError"
  }
}

/** Split CSV text into records of fields. */
export function parseRecords(text: string): { records: string[][]; lines: number[]; bom: boolean } {
  const bom = text.startsWith(BOM)
  const src = bom ? text.slice(1) : text
  const records: string[][] = []
  const lines: number[] = []
  let record: string[] = []
  let field = ""
  let quoted = false
  let inQuotes = false
  let fieldStarted = false
  let line = 1
  let recordLine = 1
  let quoteLine = 1

  const endField = () => {
    record.push(field)
    field = ""
    quoted = false
    fieldStarted = false
  }
  const endRecord = () => {
    endField()
    records.push(record)
    lines.push(recordLine)
    record = []
  }

  for (let i = 0; i < src.length; i++) {
    const ch = src[i]!
    if (inQuotes) {
      if (ch === '"') {
        if (src[i + 1] === '"') {
          field += '"'
          i++
        } else inQuotes = false
      } else {
        if (ch === "\n" || (ch === "\r" && src[i + 1] !== "\n")) line++
        field += ch
      }
      continue
    }
    if (ch === '"' && !fieldStarted) {
      inQuotes = true
      quoted = true
      fieldStarted = true
      quoteLine = line
      continue
    }
    if (ch === ",") {
      endField()
      continue
    }
    if (ch === "\r" || ch === "\n") {
      if (ch === "\r" && src[i + 1] === "\n") i++
      endRecord()
      line++
      recordLine = line
      continue
    }
    fieldStarted = true
    field += ch
  }
  if (inQuotes) throw new CsvError(`The CSV has a quote opened on line ${quoteLine} that is never closed.`)
  // The last record, unless the text ended with a line break.
  if (field !== "" || quoted || record.length > 0) endRecord()
  return { records, lines, bom }
}

const blank = (record: string[]) => record.every((f) => f === "")

/**
 * Parse a CSV with a header row. Header names are trimmed and must be unique
 * and non-empty. A row with more fields than the header is an error; a
 * shorter row is padded with empty fields.
 */
export function parseCsv(text: string): CsvTable {
  const { records, lines, bom } = parseRecords(text)
  let start = 0
  while (start < records.length && blank(records[start]!)) start++
  if (start >= records.length) throw new CsvError("The CSV is empty: it needs a header row and at least one data row.")
  const header = records[start]!.map((h) => h.trim())
  const seen = new Set<string>()
  header.forEach((name, i) => {
    if (!name) throw new CsvError(`Column ${i + 1} of the header row has no name.`)
    if (seen.has(name)) throw new CsvError(`The header row names the column "${name}" twice.`)
    seen.add(name)
  })
  const rows: string[][] = []
  const rowLines: number[] = []
  for (let r = start + 1; r < records.length; r++) {
    const record = records[r]!
    if (blank(record)) continue
    if (record.length > header.length) {
      throw new CsvError(
        `Line ${lines[r]} has ${record.length} fields but the header has ${header.length}. Quote fields that contain commas.`,
      )
    }
    rows.push(record.length < header.length ? [...record, ...Array(header.length - record.length).fill("")] : record)
    rowLines.push(lines[r]!)
  }
  return { header, rows, lines: rowLines, bom }
}

/** Quote one field when it needs it. */
export function formatField(value: string): string {
  if (/[",\r\n]/.test(value) || /^\s|\s$/.test(value)) return `"${value.replaceAll('"', '""')}"`
  return value
}

/** One CSV line, ending in \r\n. */
export function formatRow(values: readonly string[]): string {
  return values.map(formatField).join(",") + "\r\n"
}

/**
 * Text a model wrote, made safe to open in a spreadsheet: a cell that starts
 * with = + - @ tab or carriage return would run as a formula, so it gets a
 * leading apostrophe (OWASP CSV injection guidance). Applied to the columns
 * the workers fill, never to the user's own input columns.
 */
export function neutralizeFormula(value: string): string {
  return /^[=+\-@\t\r]/.test(value) ? `'${value}` : value
}
