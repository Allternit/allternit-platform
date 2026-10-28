/**
 * Minimal CopilotKit Intelligence run client, wire-compatible with `IntelligenceAgent.runAgent()` in @copilotkit/core 1.75.0
 * (src/intelligence-agent.ts, Phoenix 1.8 JS client, vsn 2.0.0 serializer). It implements only the "run" mode:
 * 1. POST the AG-UI RunAgentInput to `<runtimeUrl>/agent/<agentId>/run` and read the receipt {threadId, runId, joinToken, realtime:{clientUrl, topic}};
 * 2. open `<clientUrl>/websocket?join_token=…&vsn=2.0.0`, join `topic` with {stream_mode:'run', run_id}, heartbeat every 30 s;
 * 3. fold the channel's `ag_ui_event` pushes (AG-UI events) until RUN_FINISHED or RUN_ERROR for the receipt's thread and run.
 * No /connect replay, no reconnect and no retry: a dropped socket or an unfinished run is reported as uncertain, never repeated.
 */
import { isObject } from './paired-local.js';
import { AgUiRunState, type AgUiMessage } from './ag-ui.js';

/** The subset of the WHATWG WebSocket this client uses (Node 22+ global `WebSocket`, or a test fake). */
export type IntelligenceSocket = { send(data: string): void; close(code?: number, reason?: string): void; onopen: ((e: unknown) => void) | null; onmessage: ((e: { data: unknown }) => void) | null; onerror: ((e: unknown) => void) | null; onclose: ((e: unknown) => void) | null };
export type IntelligenceSocketFactory = (url: string) => IntelligenceSocket;
export type IntelligenceReceipt = { threadId: string; runId: string; joinToken: string; realtime: { clientUrl: string; topic: string } };
export type IntelligenceRunInput = { runtimeUrl: string; agentId: string; threadId: string; runId?: string; messages: AgUiMessage[]; headers?: Record<string, string>; fetch?: typeof fetch; socket?: IntelligenceSocketFactory; timeoutMs?: number; joinTimeoutMs?: number; heartbeatMs?: number; maxBytes?: number; signal?: AbortSignal };
/**
 * `transport` is set when the run could not be trusted: unreachable/timeout (outcome unknown), locked (HTTP 409, thread busy), receipt (missing join credentials),
 * join (channel join refused or timed out), closed (socket or channel ended before the run did), too_large, malformed (protocol drift).
 */
export type IntelligenceRunResult = { status?: number; transport?: 'unreachable' | 'timeout' | 'locked' | 'receipt' | 'join' | 'closed' | 'too_large' | 'malformed'; receipt?: IntelligenceReceipt; started: boolean; finished: boolean; interrupted: boolean; error?: { message: string; code?: string }; identityMismatch: boolean; messages: AgUiMessage[] };

const str = (v: unknown): v is string => typeof v === 'string' && v.length > 0;
const fold = (state: AgUiRunState, extra: Partial<IntelligenceRunResult>): IntelligenceRunResult => { const { started, finished, interrupted, identityMismatch, error, messages } = state.result(); return { started, finished, interrupted, identityMismatch, ...(error ? { error } : {}), messages, ...extra }; };
const defaultSocket: IntelligenceSocketFactory = url => new (globalThis as unknown as { WebSocket: new (u: string) => IntelligenceSocket }).WebSocket(url);

/** Same URL the SDK builds in buildRuntimeUrl (multi-route transport). */
export const intelligenceRunUrl = (runtimeUrl: string, agentId: string) => new URL(`${runtimeUrl}/agent/${encodeURIComponent(agentId)}/run`).toString();
/** Same URL the Phoenix client builds: endpoint + "/websocket", socket params, then vsn. */
export const intelligenceSocketUrl = (clientUrl: string, joinToken: string) => `${clientUrl}/websocket${clientUrl.includes('?') ? '&' : '?'}join_token=${encodeURIComponent(joinToken)}&vsn=2.0.0`;

/** Mirrors normalizeJoinCredentials: joinToken, realtime.clientUrl and realtime.topic are required; threadId/runId fall back to the request. */
export function intelligenceReceipt(payload: unknown, input: { threadId: string; runId: string }): IntelligenceReceipt | null {
  if (!isObject(payload) || !str(payload.joinToken) || !isObject(payload.realtime) || !str(payload.realtime.clientUrl) || !str(payload.realtime.topic)) return null;
  return { threadId: str(payload.threadId) ? payload.threadId : input.threadId, runId: str(payload.runId) ? payload.runId : input.runId, joinToken: payload.joinToken, realtime: { clientUrl: payload.realtime.clientUrl, topic: payload.realtime.topic } };
}

/** Runs one Intelligence turn. Never throws; HTTP failures come back as `status`, everything else as `transport`. */
export async function runIntelligence(input: IntelligenceRunInput): Promise<IntelligenceRunResult> {
  const runId = input.runId ?? crypto.randomUUID();
  let state = new AgUiRunState({ threadId: input.threadId, runId }, new Set(input.messages.map(m => m.id)));
  const timeout = AbortSignal.timeout(input.timeoutMs ?? 120_000), signal = input.signal ? AbortSignal.any([input.signal, timeout]) : timeout;
  const lost = () => (timeout.aborted ? 'timeout' : 'unreachable') as 'timeout' | 'unreachable';
  let receipt: IntelligenceReceipt | null;
  try {
    const response = await (input.fetch ?? fetch)(intelligenceRunUrl(input.runtimeUrl, input.agentId), { method: 'POST', redirect: 'error', signal, headers: { 'Content-Type': 'application/json', ...input.headers },
      body: JSON.stringify({ threadId: input.threadId, runId, tools: [], context: [], forwardedProps: {}, state: {}, messages: input.messages }) });
    if (response.status === 409) { void response.body?.cancel().catch(() => undefined); return fold(state, { status: 409, transport: 'locked' }); }
    if (!response.ok) { void response.body?.cancel().catch(() => undefined); return fold(state, { status: response.status }); }
    receipt = intelligenceReceipt(await response.json(), { threadId: input.threadId, runId });
  } catch { return fold(state, { transport: lost() }); }
  if (!receipt) return fold(state, { transport: 'receipt' });
  const r = receipt;
  // The run's identity is the receipt's (applyCanonicalRunIdentity): events for any other thread or run are refused.
  state = new AgUiRunState({ threadId: r.threadId, runId: r.runId }, new Set(input.messages.map(m => m.id)));
  const done = (extra: Partial<IntelligenceRunResult> = {}) => fold(state, { receipt: r, ...extra });
  if (signal.aborted) return done({ transport: lost() });

  let ws: IntelligenceSocket;
  try { ws = (input.socket ?? defaultSocket)(intelligenceSocketUrl(r.realtime.clientUrl, r.joinToken)); } catch { return done({ transport: 'unreachable' }); }
  const JOIN = '1', maxBytes = input.maxBytes ?? 8 * 1024 * 1024;
  let ref = 1, joined = false, total = 0, heartbeat: ReturnType<typeof setInterval> | undefined, joinTimer: ReturnType<typeof setTimeout> | undefined, onAbort: (() => void) | undefined;
  const push = (joinRef: string | null, event: string, payload: unknown, topic = r.realtime.topic) => { try { ws.send(JSON.stringify([joinRef, String(ref++), topic, event, payload])); } catch { /* reported by onclose/onerror */ } };
  const outcome = await new Promise<IntelligenceRunResult>(resolve => {
    let settled = false;
    const end = (extra: Partial<IntelligenceRunResult> = {}) => { if (!settled) { settled = true; resolve(done(extra)); } };
    onAbort = () => end({ transport: lost() }); signal.addEventListener('abort', onAbort, { once: true });
    ws.onopen = () => {
      push(JOIN, 'phx_join', { stream_mode: 'run', run_id: r.runId });
      joinTimer = setTimeout(() => { if (!joined) end({ transport: 'join' }); }, input.joinTimeoutMs ?? 10_000);
      heartbeat = setInterval(() => push(null, 'heartbeat', {}, 'phoenix'), input.heartbeatMs ?? 30_000);
    };
    ws.onmessage = ({ data }) => {
      if (typeof data !== 'string') return; // binary frames carry no AG-UI events
      total += data.length; if (total > maxBytes) return end({ transport: 'too_large' });
      let frame: unknown; try { frame = JSON.parse(data); } catch { return end({ transport: 'malformed' }); }
      if (!Array.isArray(frame) || frame.length !== 5) return end({ transport: 'malformed' });
      const [joinRef, msgRef, topic, event, payload] = frame as [unknown, unknown, unknown, unknown, unknown];
      if (topic !== r.realtime.topic || (joinRef != null && joinRef !== JOIN)) return; // heartbeat replies and outdated joins
      if (event === 'phx_reply' && msgRef === JOIN) { if (isObject(payload) && payload.status === 'ok') { joined = true; clearTimeout(joinTimer); } else end({ transport: 'join' }); return; }
      if (event === 'phx_error' || event === 'phx_close') return end({ transport: 'closed' });
      if (event !== 'ag_ui_event') return; // replay_complete, stream_idle and other control events belong to connect mode
      if (!state.apply(payload)) return end({ transport: 'malformed' });
      if (state.done) end();
    };
    ws.onerror = () => end({ transport: 'closed' });
    ws.onclose = () => end({ transport: 'closed' });
  });
  clearInterval(heartbeat); clearTimeout(joinTimer); if (onAbort) signal.removeEventListener('abort', onAbort);
  if (joined) push(JOIN, 'phx_leave', {});
  ws.onopen = ws.onmessage = ws.onerror = ws.onclose = null;
  try { ws.close(1000, 'run complete'); } catch { /* already closed */ }
  return outcome;
}
