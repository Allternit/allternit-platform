import type { IntelligenceSocket } from '../adapters/_shared/copilotkit-intelligence.js';

/** Fake CopilotKit Intelligence realtime server speaking Phoenix vsn 2.0.0 frames over a fake WebSocket. */
export type PhoenixScript = { joinStatus?: 'ok' | 'error' | 'silent'; events?: (topic: string, runId: string) => unknown[]; dropAfterJoin?: boolean };
export function fakePhoenix(script: PhoenixScript = {}) {
  const urls: string[] = [], sent: unknown[][] = []; let closed = 0;
  const factory = (url: string): IntelligenceSocket => {
    urls.push(url);
    const socket: IntelligenceSocket = {
      onopen: null, onmessage: null, onerror: null, onclose: null,
      send(data: string) {
        const frame = JSON.parse(data) as unknown[]; sent.push(frame);
        const [joinRef, ref, topic, event, payload] = frame as [string | null, string, string, string, Record<string, unknown>];
        const deliver = (f: unknown[]) => queueMicrotask(() => socket.onmessage?.({ data: JSON.stringify(f) }));
        if (event === 'heartbeat') return deliver([null, ref, 'phoenix', 'phx_reply', { status: 'ok', response: {} }]);
        if (event !== 'phx_join' || script.joinStatus === 'silent') return;
        deliver([joinRef, ref, topic, 'phx_reply', { status: script.joinStatus ?? 'ok', response: {} }]);
        if (script.dropAfterJoin) return void queueMicrotask(() => socket.onclose?.({}));
        for (const e of script.events?.(topic, String(payload.run_id)) ?? []) deliver([joinRef, null, topic, 'ag_ui_event', e]);
      },
      close() { closed++; }
    };
    queueMicrotask(() => socket.onopen?.({}));
    return socket;
  };
  return { factory, urls, sent, closed: () => closed };
}

/** A complete AG-UI run as the Intelligence gateway pushes it. */
export const replyEvents = (threadId: string, text: string, opts: { runId?: string } = {}) => (_topic: string, runId: string) => [
  { type: 'RUN_STARTED', threadId, runId: opts.runId ?? runId, metadata: { cpki_event_id: 'e1' } },
  { type: 'TEXT_MESSAGE_START', messageId: 'answer', role: 'assistant' },
  { type: 'TEXT_MESSAGE_CONTENT', messageId: 'answer', delta: text },
  { type: 'TEXT_MESSAGE_END', messageId: 'answer' },
  { type: 'RUN_FINISHED', threadId, runId: opts.runId ?? runId }
];
