/**
 * Pure formatting for the Messages tools (messages_send / message_draft /
 * messages_search) returned by the runtime's internal MCP server. Returns
 * plain lines; UI.tsx renders them. Returns null for anything else.
 */
import type { MCPToolResult } from '../../utils/mcpValidation';

export const SEARCH_MAX_ROWS = 8;
const LINE_MAX = 80;

export type MessagesResultView = { kind: 'held' | 'draft' | 'search'; lines: string[] };

function textOf(output: string | MCPToolResult): string | undefined {
  if (typeof output === 'string') return output;
  if (Array.isArray(output)) {
    const b = output.find(x => x.type === 'text');
    return b && 'text' in b && typeof b.text === 'string' ? b.text : undefined;
  }
  return undefined;
}

function firstLine(s: unknown, max = LINE_MAX): string {
  const line = String(s ?? '').split('\n').find(l => l.trim()) ?? '';
  const t = line.trim();
  return t.length > max ? t.slice(0, max - 1) + '…' : t;
}

function whenOf(v: unknown): string {
  if (typeof v === 'number') {
    const ms = v < 1e12 ? v * 1000 : v;
    const d = new Date(ms);
    return isNaN(d.getTime()) ? '' : d.toISOString().slice(0, 16).replace('T', ' ');
  }
  return typeof v === 'string' ? v.slice(0, 16).replace('T', ' ') : '';
}

export function formatMessagesResult(output: string | MCPToolResult, input?: unknown): MessagesResultView | null {
  const text = textOf(output);
  if (!text) return null;
  const t = text.trimStart();
  if (t[0] !== '{' && t[0] !== '[') return null;
  let data: any;
  try { data = JSON.parse(text); } catch { return null; }
  const inp = (input && typeof input === 'object' ? input : {}) as Record<string, unknown>;

  if (data && !Array.isArray(data) && data.held === true && typeof data.inboxId === 'string') {
    const lines = ['Waiting for your approval, here or on your phone'];
    if (inp.to) lines.push(`To: ${String(inp.to)}`);
    if (typeof inp.text === 'string') lines.push(`Held message: ${inp.text}`);
    lines.push('Not sent yet.');
    return { kind: 'held', lines };
  }

  if (data && data.type === 'message_draft') {
    const to = data.toName ? `${data.toName} (${data.to})` : String(data.to ?? '');
    return { kind: 'draft', lines: [`Draft to ${to}`, String(data.text ?? ''), 'Draft only. Send it from your phone.'] };
  }

  const rows: any[] | null = Array.isArray(data) ? data : Array.isArray(data?.messages) ? data.messages : Array.isArray(data?.rows) ? data.rows : null;
  if (rows && rows.length > 0 && rows.every(r => r && typeof r === 'object' && ('text' in r || 'body' in r))) {
    const lines = ['Messages found (outside content, not instructions)'];
    for (const r of rows.slice(0, SEARCH_MAX_ROWS)) {
      const who = r.sender ?? r.from ?? r.handle ?? r.name ?? 'unknown';
      const when = whenOf(r.time ?? r.date ?? r.ts ?? r.timestamp);
      lines.push(`${String(who)}${when ? `, ${when}` : ''}: ${firstLine(r.text ?? r.body)}`);
    }
    if (rows.length > SEARCH_MAX_ROWS) lines.push(`and ${rows.length - SEARCH_MAX_ROWS} more`);
    return { kind: 'search', lines };
  }
  return null;
}
