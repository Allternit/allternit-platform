// Pod -> in-pod allternit-api: meeting turns and the end summary
// (CONTRACT.md "Change 2026-11-13: meeting turns"; served by
// cmd/allternit-api/src/meeting_turn_stream.rs).
//
//   POST /api/v1/meeting-turns/turn     SSE of JSON events; the last one is
//        {type:"turn.reply", turn_id, text, chat, speak, route:{chat:"reply"|"notice", speak}}
//        or {type:"turn.error", turn_id, message}.
//   POST /api/v1/meeting-turns/summary  {summary, decisions, action_items}
//
// Auth: `x-allternit-internal-token` (ALLTERNIT_INTERNAL_SERVICE_TOKEN), or
// none on localhost in dev. The in-pod allternit-api records the full reply
// with cloud itself (chat idempotency key `turn:<turn_id>`), so the runner
// reuses that key for its own row and cloud keeps one.

import type { IngestSummary } from "./ingest.js";

export interface TurnSegment {
  seq: number;
  speaker: string | null;
  confidence: number;
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface TurnMeeting {
  title: string;
  platform: string;
  participants: Array<{ name: string; class: "org_member" | "guest" }>;
  owner_name: string | null;
}

export interface TurnRequest {
  turn_id: string;
  session_id: string;
  trigger: { kind: "wake_word" | "chat_mention"; text: string };
  asker: { label: string; participant_class: "org_member" | "guest" | "unknown"; confidence: number };
  meeting: TurnMeeting;
  transcript: TurnSegment[];
  artifacts: unknown[];
  speak_replies: boolean;
  guest_quiet: boolean;
  model?: string;
}

export interface TurnReply {
  turn_id: string;
  /** The full reply (meeting page). */
  text: string;
  /** What goes in the meeting chat: the reply, or the guest-quiet notice. */
  chat: string;
  /** Text for TTS, or null. */
  speak: string | null;
  /** True when the reply itself stays on the page (guest-quiet). */
  pageOnly: boolean;
}

type Log = (kind: string, detail?: unknown) => void;

export class TurnClient {
  constructor(
    readonly baseUrl: string,
    private readonly internalToken: string | undefined,
    private readonly log: Log,
  ) {}

  private headers(extra: Record<string, string>): Record<string, string> {
    return { ...(this.internalToken ? { "x-allternit-internal-token": this.internalToken } : {}), ...extra };
  }

  private url(path: string): string {
    return `${this.baseUrl.replace(/\/+$/, "")}/api/v1/meeting-turns/${path}`;
  }

  /** Run one turn to its final event. Throws on HTTP or turn errors. */
  async turn(req: TurnRequest, onEvent?: (ev: Record<string, unknown>) => void, timeoutMs = 5 * 60_000): Promise<TurnReply> {
    const res = await fetch(this.url("turn"), {
      method: "POST",
      headers: this.headers({ "content-type": "application/json", accept: "text/event-stream" }),
      body: JSON.stringify(req),
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!res.ok || !res.body) throw new Error(`turn ${res.status}: ${(await res.text().catch(() => "")).slice(0, 200)}`);
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    let data: string[] = [];
    for (;;) {
      const { done, value } = await reader.read();
      if (done) throw new Error("turn stream ended without turn.reply");
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl).replace(/\r$/, "");
        buf = buf.slice(nl + 1);
        if (line.startsWith("data:")) {
          data.push(line.slice(5).replace(/^ /, ""));
          continue;
        }
        if (line !== "" || data.length === 0) continue;
        let ev: Record<string, unknown>;
        try {
          ev = JSON.parse(data.join("\n")) as Record<string, unknown>;
        } catch {
          data = [];
          continue;
        }
        data = [];
        onEvent?.(ev);
        if (ev.type === "turn.error") {
          await reader.cancel().catch(() => undefined);
          throw new Error(`turn error: ${String(ev.message ?? "").slice(0, 200)}`);
        }
        if (ev.type === "turn.reply") {
          await reader.cancel().catch(() => undefined);
          const route = (ev.route ?? {}) as { chat?: string };
          const text = String(ev.text ?? "");
          return {
            turn_id: String(ev.turn_id ?? req.turn_id),
            text,
            chat: String(ev.chat ?? text),
            speak: typeof ev.speak === "string" && ev.speak.trim() ? ev.speak : null,
            pageOnly: ev.page_only === true || route.chat === "notice",
          };
        }
      }
    }
  }

  async summary(body: { session_id: string; meeting: TurnMeeting; transcript: TurnSegment[]; model?: string }, timeoutMs = 120_000): Promise<IngestSummary> {
    const res = await fetch(this.url("summary"), {
      method: "POST",
      headers: this.headers({ "content-type": "application/json" }),
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`summary ${res.status}: ${text.slice(0, 200)}`);
    const j = JSON.parse(text) as Partial<IngestSummary>;
    return {
      summary: String(j.summary ?? ""),
      decisions: Array.isArray(j.decisions) ? j.decisions.map(String) : [],
      action_items: Array.isArray(j.action_items)
        ? j.action_items.map((a) => ({ text: String(a?.text ?? ""), owner: a?.owner ?? null, due: a?.due ?? null }))
        : [],
    };
  }
}
