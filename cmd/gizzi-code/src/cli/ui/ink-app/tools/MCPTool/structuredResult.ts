// Pure formatting for MCP `structuredContent` (PLAN §3 metadata.mcp). No React here
// so it can be tested without a terminal.
import { jsonStringify } from '../../utils/slowOperations';

export const MAX_TABLE_ROWS = 10;
const MAX_CELL_CHARS = 40;
const MAX_VALUE_CHARS = 120;
const MAX_TABLE_COLS = 6;

export type StructuredView =
  | { kind: 'kv'; rows: [string, string][] }
  | { kind: 'table'; columns: string[]; rows: string[][]; more: number };

function cell(value: unknown, max: number): string {
  let s: string;
  if (value === null || value === undefined) s = '';
  else if (typeof value === 'string') s = value;
  else if (typeof value === 'number' || typeof value === 'boolean') s = String(value);
  else s = jsonStringify(value);
  s = s.replace(/\s+/g, ' ');
  return s.length > max ? s.slice(0, max - 1) + '…' : s;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/** Array of objects -> table (first 10 rows + "+N more"); object -> key/value rows. */
export function structuredView(sc: unknown): StructuredView | null {
  if (Array.isArray(sc)) {
    const objs = sc.filter(isRecord);
    if (sc.length === 0 || objs.length !== sc.length) return null;
    const columns: string[] = [];
    for (const o of objs) for (const k of Object.keys(o)) if (!columns.includes(k) && columns.length < MAX_TABLE_COLS) columns.push(k);
    if (columns.length === 0) return null;
    return {
      kind: 'table',
      columns,
      rows: objs.slice(0, MAX_TABLE_ROWS).map(o => columns.map(c => cell(o[c], MAX_CELL_CHARS))),
      more: Math.max(0, objs.length - MAX_TABLE_ROWS),
    };
  }
  if (!isRecord(sc)) return null;
  const entries = Object.entries(sc);
  if (entries.length === 0) return null;
  // A single key holding an array of objects (e.g. { items: [...] }) reads best as a table.
  if (entries.length === 1 && Array.isArray(entries[0]![1])) {
    const inner = structuredView(entries[0]![1]);
    if (inner) return inner;
  }
  return { kind: 'kv', rows: entries.map(([k, v]) => [k, cell(v, MAX_VALUE_CHARS)] as [string, string]) };
}

/** Pad cells into aligned text lines (header first for tables). */
export function alignLines(view: StructuredView): string[] {
  if (view.kind === 'kv') {
    const w = Math.max(...view.rows.map(([k]) => k.length));
    return view.rows.map(([k, v]) => `${k.padEnd(w)}  ${v}`);
  }
  const all = [view.columns, ...view.rows];
  const widths = view.columns.map((_, i) => Math.max(...all.map(r => (r[i] ?? '').length)));
  const lines = all.map(r => r.map((c, i) => c.padEnd(widths[i]!)).join('  ').trimEnd());
  if (view.more > 0) lines.push(`+${view.more} more`);
  return lines;
}
