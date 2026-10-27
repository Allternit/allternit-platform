/**
 * Minimal AG-UI HTTP/SSE client (https://docs.ag-ui.com, event names as in @ag-ui/core 0.0.59).
 * POSTs one RunAgentInput, reads the SSE stream, and reports what the run proved: RUN_STARTED/RUN_FINISHED for the
 * submitted thread and run, RUN_ERROR, and the assistant text assembled per messageId (MESSAGES_SNAPSHOT replaces it).
 * It never retries: an uncertain run is reported, not repeated.
 */
import { isObject } from './paired-local.js';

export type AgUiMessage = { id: string; role: 'user' | 'assistant' | 'system'; content: string };
export type AgUiRunInput = { url: string; threadId: string; runId: string; messages: AgUiMessage[]; state?: unknown; headers?: Record<string, string>; fetch?: typeof fetch; timeoutMs?: number; signal?: AbortSignal; maxBytes?: number };
/** `transport` is set when the stream could not be trusted: unreachable/timeout (outcome unknown), too_large, malformed (protocol drift), not_sse. */
export type AgUiRunResult = { status?: number; transport?: 'unreachable' | 'timeout' | 'too_large' | 'malformed' | 'not_sse'; started: boolean; finished: boolean; interrupted: boolean; error?: { message: string; code?: string }; identityMismatch: boolean; threadId?: string; runId?: string; messages: AgUiMessage[] };

const KNOWN = new Set(['RUN_STARTED', 'RUN_FINISHED', 'RUN_ERROR', 'STEP_STARTED', 'STEP_FINISHED', 'TEXT_MESSAGE_START', 'TEXT_MESSAGE_CONTENT', 'TEXT_MESSAGE_END', 'TEXT_MESSAGE_CHUNK', 'TOOL_CALL_START', 'TOOL_CALL_ARGS', 'TOOL_CALL_END', 'TOOL_CALL_CHUNK', 'TOOL_CALL_RESULT', 'STATE_SNAPSHOT', 'STATE_DELTA', 'MESSAGES_SNAPSHOT', 'RAW', 'CUSTOM']);
const str = (v: unknown): v is string => typeof v === 'string';
const ROLES = new Set(['user', 'assistant', 'system', 'developer']);

/** Folds AG-UI events into run facts. Exported for tests and other transports. Returns false on a malformed known event. */
export class AgUiRunState {
  started = false; finished = false; interrupted = false; identityMismatch = false; error?: { message: string; code?: string }; threadId?: string; runId?: string;
  private text = new Map<string, { role: string; content: string }>();
  private lastChunk?: string;
  constructor(private expect: { threadId: string; runId: string }, private inputIds: Set<string>) {}
  get done() { return this.finished || this.interrupted || !!this.error || this.identityMismatch; }
  apply(e: unknown): boolean {
    if (!isObject(e) || !str(e.type)) return false;
    if (!KNOWN.has(e.type)) return true; // forward compatible: reasoning, activity, subagent and future events are ignored
    const ids = () => { if (!str(e.threadId) || !str(e.runId)) return false; this.threadId = e.threadId; this.runId = e.runId; if (e.threadId !== this.expect.threadId || e.runId !== this.expect.runId) this.identityMismatch = true; return true; };
    switch (e.type) {
      case 'RUN_STARTED': this.started = true; return ids();
      case 'RUN_FINISHED': { if (!ids()) return false; const o = e.outcome; if (o == null || (isObject(o) && o.type === 'success')) this.finished = true; else if (isObject(o) && o.type === 'interrupt') this.interrupted = true; else return false; return true; }
      case 'RUN_ERROR': if (!str(e.message)) return false; this.error = { message: e.message, ...(str(e.code) ? { code: e.code } : {}) }; return true;
      case 'TEXT_MESSAGE_START': if (!str(e.messageId)) return false; if (!this.text.has(e.messageId)) this.text.set(e.messageId, { role: str(e.role) ? e.role : 'assistant', content: '' }); return true;
      case 'TEXT_MESSAGE_CONTENT': if (!str(e.messageId) || !str(e.delta)) return false; return this.append(e.messageId, e.delta);
      case 'TEXT_MESSAGE_END': return str(e.messageId);
      case 'TEXT_MESSAGE_CHUNK': {
        const id = str(e.messageId) ? e.messageId : this.lastChunk; if (!id || (e.delta !== undefined && !str(e.delta))) return false;
        if (!this.text.has(id)) this.text.set(id, { role: str(e.role) ? e.role : 'assistant', content: '' }); this.lastChunk = id;
        return str(e.delta) ? this.append(id, e.delta) : true;
      }
      case 'TOOL_CALL_START': return str(e.toolCallId) && str(e.toolCallName);
      case 'TOOL_CALL_ARGS': return str(e.toolCallId) && str(e.delta);
      case 'TOOL_CALL_END': return str(e.toolCallId);
      case 'TOOL_CALL_CHUNK': return e.toolCallId === undefined || str(e.toolCallId);
      case 'TOOL_CALL_RESULT': return str(e.toolCallId) && str(e.messageId);
      case 'STEP_STARTED': case 'STEP_FINISHED': return str(e.stepName);
      case 'STATE_DELTA': return Array.isArray(e.delta);
      case 'MESSAGES_SNAPSHOT': {
        if (!Array.isArray(e.messages)) return false;
        this.text.clear(); this.lastChunk = undefined;
        for (const m of e.messages) { if (!isObject(m) || !str(m.id) || !str(m.role)) return false; if (!this.inputIds.has(m.id) && str(m.content)) this.text.set(m.id, { role: m.role, content: m.content }); }
        return true;
      }
      case 'CUSTOM': return str(e.name);
      default: return true; // STATE_SNAPSHOT, RAW
    }
  }
  private append(id: string, delta: string) { const m = this.text.get(id); if (m) m.content += delta; else this.text.set(id, { role: 'assistant', content: delta }); return true; }
  /** Messages the run produced, in first-seen order (assistant/system/user roles only). */
  messages(): AgUiMessage[] { return [...this.text].filter(([, m]) => ROLES.has(m.role) && m.role !== 'developer').map(([id, m]) => ({ id, role: m.role as AgUiMessage['role'], content: m.content })); }
  result(extra: Partial<AgUiRunResult> = {}): AgUiRunResult { return { started: this.started, finished: this.finished, interrupted: this.interrupted, identityMismatch: this.identityMismatch, ...(this.error ? { error: this.error } : {}), ...(this.threadId ? { threadId: this.threadId, runId: this.runId } : {}), messages: this.messages(), ...extra }; }
}

/** Runs one AG-UI turn. Never throws; HTTP failures come back as `status`, transport failures as `transport`. */
export async function runAgUi(input: AgUiRunInput): Promise<AgUiRunResult> {
  const state = new AgUiRunState(input, new Set(input.messages.map((m) => m.id)));
  const timeout = AbortSignal.timeout(input.timeoutMs ?? 120_000), signal = input.signal ? AbortSignal.any([input.signal, timeout]) : timeout;
  const maxBytes = input.maxBytes ?? 8 * 1024 * 1024;
  const failed = () => state.result({ transport: timeout.aborted ? 'timeout' : 'unreachable' });
  let response: Response;
  try {
    response = await (input.fetch ?? fetch)(input.url, { method: 'POST', redirect: 'error', signal, headers: { ...input.headers, 'content-type': 'application/json', accept: 'text/event-stream' },
      body: JSON.stringify({ threadId: input.threadId, runId: input.runId, state: input.state ?? {}, messages: input.messages, tools: [], context: [], forwardedProps: {} }) });
  } catch { return failed(); }
  if (!response.ok) { void response.body?.cancel().catch(() => undefined); return state.result({ status: response.status }); }
  const type = response.headers.get('content-type');
  if (!response.body || (type && !type.includes('text/event-stream'))) { void response.body?.cancel().catch(() => undefined); return state.result({ transport: 'not_sse' }); }
  const reader = response.body.getReader(), decoder = new TextDecoder();
  let buffer = '', total = 0;
  const frame = (block: string): boolean => {
    const data = block.split('\n').filter((l) => l.startsWith('data:')).map((l) => l.slice(l[5] === ' ' ? 6 : 5)).join('\n');
    if (!data) return true; // comments, keep-alives, event:/id:/retry: only
    let event: unknown; try { event = JSON.parse(data); } catch { return false; }
    return state.apply(event);
  };
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (value) { total += value.byteLength; if (total > maxBytes) return state.result({ transport: 'too_large' }); buffer += decoder.decode(value, { stream: true }); }
      if (done) buffer += decoder.decode() + '\n\n';
      buffer = buffer.replace(/\r\n?/g, '\n');
      let at: number;
      while ((at = buffer.indexOf('\n\n')) >= 0) { const block = buffer.slice(0, at); buffer = buffer.slice(at + 2); if (!frame(block)) return state.result({ transport: 'malformed' }); if (state.done) return state.result(); }
      if (done) return state.result();
    }
  } catch { return failed(); }
  finally { void reader.cancel().catch(() => undefined); }
}
