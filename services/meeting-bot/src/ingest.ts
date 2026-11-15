// Pod -> cloud: batched, idempotent ingest, the control stream, tool calls,
// and the active-speaker timeline used to label transcript lines.
// Shapes: docs/meetings/CONTRACT.md section 6. Auth: the pod token (Bearer).
//
// Ingest rules:
// - A batch goes out every 2 s while there is anything to send, and at once on
//   a state change. At most 500 rows per batch (cloud's MAX_BATCH_ROWS).
// - Each batch carries at most one state. Two quick moves (joining ->
//   waiting_admit) are two batches, in order.
// - A failed POST is retried with the same batch_id and the same rows, so a
//   lost response never doubles anything; newer rows wait behind it.
// - 409 = cloud refused the state move (rows were written before the check):
//   the state is dropped and the batch counts as delivered.
// - 401/403/404: the token is no longer valid (session ended + 5 min). Stop.

import { randomUUID } from "node:crypto";
import type { ActiveSpeakerEvent, EndReason, Participant } from "./adapter.js";

export type SessionState = "launching" | "joining" | "waiting_admit" | "live" | "paused" | "ended";

export interface IngestParticipant {
  platform_participant_id: string;
  display_name: string;
  email: string | null;
  class: "org_member" | "guest";
  joined_at: string;
  left_at: string | null;
}

export interface IngestSegment {
  seq: number;
  idempotency_key: string;
  speaker: string | null;
  confidence: number;
  source: "audio" | "captions";
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface IngestChat {
  idempotency_key: string;
  direction: "in" | "out";
  sender: string;
  text: string;
  at: string;
  /** Out rows only: `page_only` = shown on the meeting page, never posted in the call (CONTRACT "meeting turns"). */
  visibility?: "chat" | "page_only";
}

export interface IngestSummary {
  summary: string;
  decisions: string[];
  action_items: Array<{ text: string; owner: string | null; due: string | null }>;
}

export interface IngestScreenNote {
  idempotency_key: string;
  at_ms: number;
  description: string;
}

export interface IngestBatch {
  batch_id: string;
  sent_at: string;
  state?: { state: SessionState; end_reason: EndReason | null; detail?: string };
  participants?: IngestParticipant[];
  transcript?: IngestSegment[];
  chat?: IngestChat[];
  screen_notes?: IngestScreenNote[];
  notice_at?: string | null;
  /** End-of-meeting summary from the in-pod allternit-api; rides in the final batch. */
  summary?: IngestSummary;
}

export type ControlMessage =
  | { type: "stop_listening" | "resume" | "leave" }
  | { type: "speak"; text: string }
  | { type: "set_speak_replies"; on: boolean }
  | { type: "set_guest_quiet"; on: boolean }
  | { type: "approval"; approval_id: string; status: string };

export type ToolCallResult =
  | { status: "ok"; result: unknown }
  | { status: "approval_required"; approval_id: string }
  | { status: "denied" }
  | { status: "clarify"; question: string };

type Log = (kind: string, detail?: unknown) => void;

const MAX_BATCH_ROWS = 500;
const iso = (t: number) => new Date(t).toISOString();
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

// ---- HTTP ------------------------------------------------------------------------

export class CloudClient {
  constructor(
    readonly baseUrl: string,
    readonly sessionId: string,
    private readonly podToken: string,
    private readonly log: Log,
  ) {}

  url(path: string): string {
    return `${this.baseUrl.replace(/\/+$/, "")}/api/v1/meeting-sessions/${encodeURIComponent(this.sessionId)}/${path}`;
  }

  headers(extra: Record<string, string> = {}): Record<string, string> {
    return { authorization: `Bearer ${this.podToken}`, "user-agent": "allternit-meeting-bot", ...extra };
  }

  async postJson(path: string, body: unknown, timeoutMs = 15_000): Promise<{ status: number; json: unknown }> {
    const res = await fetch(this.url(path), {
      method: "POST",
      headers: this.headers({ "content-type": "application/json" }),
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
    const text = await res.text();
    let json: unknown = null;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      json = { raw: text.slice(0, 300) };
    }
    return { status: res.status, json };
  }

  /**
   * One tool call for the in-pod agent session. Polls the same call_id while
   * the owner decides on the meeting page (approval_required), up to `waitMs`.
   */
  async toolCall(
    call: { call_id?: string; tool: string; args: unknown; asked_by?: string | null; speaker_confidence?: number },
    waitMs = 10 * 60_000,
  ): Promise<ToolCallResult> {
    const body = { call_id: call.call_id ?? randomUUID(), tool: call.tool, args: call.args ?? {}, asked_by: call.asked_by ?? null, speaker_confidence: call.speaker_confidence ?? 0 };
    const until = Date.now() + waitMs;
    for (;;) {
      const r = await this.postJson("tool-calls", body);
      if (r.status !== 200) throw new Error(`tool-calls ${r.status}: ${JSON.stringify(r.json).slice(0, 200)}`);
      const res = r.json as ToolCallResult;
      if (res.status !== "approval_required" || Date.now() >= until) return res;
      await sleep(3000);
    }
  }
}

// ---- ingest ----------------------------------------------------------------------

export class Ingest {
  private states: Array<NonNullable<IngestBatch["state"]>> = [];
  private participants = new Map<string, IngestParticipant>();
  private dirtyParticipants = new Set<string>();
  private transcript: IngestSegment[] = [];
  private chat: IngestChat[] = [];
  private notes: IngestScreenNote[] = [];
  private noticeAt: string | null = null;
  private summary: IngestSummary | null = null;
  private inflight: IngestBatch | null = null;
  private seq = 0;
  private timer: ReturnType<typeof setInterval> | null = null;
  private flushing: Promise<void> | null = null;
  private dead = false;
  readonly stats = { batches: 0, rows: 0, accepted: 0, duplicates: 0, dropped: 0, failures: 0, conflicts: 0 };

  constructor(
    private readonly cloud: CloudClient,
    private readonly log: Log,
    private readonly intervalMs = 2000,
  ) {}

  get stopped(): boolean {
    return this.dead;
  }

  start(): void {
    this.timer ??= setInterval(() => void this.flush(), this.intervalMs);
  }

  setState(state: SessionState, endReason: EndReason | null = null, detail?: string): Promise<void> {
    this.states.push({ state, end_reason: endReason, ...(detail ? { detail } : {}) });
    return this.flush();
  }

  /** Full current set from the adapter; joins and leaves are derived here. */
  setParticipants(all: Participant[], now = Date.now(), orgDomain?: string): void {
    const seen = new Set<string>();
    for (const p of all) {
      const id = p.id || p.name;
      seen.add(id);
      const prev = this.participants.get(id);
      const cls: IngestParticipant["class"] =
        p.internal === true || (orgDomain && p.email?.toLowerCase().endsWith(`@${orgDomain}`)) ? "org_member" : "guest";
      if (!prev || prev.left_at !== null || prev.display_name !== p.name || prev.class !== cls) {
        this.participants.set(id, {
          platform_participant_id: id,
          display_name: p.name,
          email: p.email ?? null,
          class: cls,
          joined_at: prev && prev.left_at === null ? prev.joined_at : iso(now),
          left_at: null,
        });
        this.dirtyParticipants.add(id);
      }
    }
    for (const [id, p] of this.participants) {
      if (!seen.has(id) && p.left_at === null) {
        p.left_at = iso(now);
        this.dirtyParticipants.add(id);
      }
    }
  }

  /** Mark everyone still present as left (session end). */
  closeParticipants(now = Date.now()): void {
    for (const [id, p] of this.participants) {
      if (p.left_at === null) {
        p.left_at = iso(now);
        this.dirtyParticipants.add(id);
      }
    }
  }

  addChat(row: Omit<IngestChat, "at"> & { at: number }): void {
    this.chat.push({ ...row, at: iso(row.at) });
  }

  addSegment(seg: Omit<IngestSegment, "seq" | "idempotency_key">): number {
    const seq = ++this.seq;
    this.transcript.push({ ...seg, seq, idempotency_key: `seg-${seq}` });
    return seq;
  }

  addScreenNote(note: IngestScreenNote): void {
    this.notes.push(note);
  }

  setNoticeAt(t: number): void {
    this.noticeAt = iso(t);
  }

  /** Sent with the next batch (call before the final `ended` state so they travel together). */
  setSummary(s: IngestSummary): void {
    this.summary = s;
  }

  /** The class ingest gave a display name, for a turn's asker; "unknown" when nobody matches. */
  classOf(name: string): "org_member" | "guest" | "unknown" {
    const n = name.trim().toLowerCase();
    for (const p of this.participants.values()) if (p.display_name.trim().toLowerCase() === n) return p.class;
    return "unknown";
  }

  /** Present participants as the turn route's meeting.participants. */
  present(): Array<{ name: string; class: "org_member" | "guest" }> {
    return Array.from(this.participants.values())
      .filter((p) => p.left_at === null)
      .map((p) => ({ name: p.display_name, class: p.class }));
  }

  private hasWork(): boolean {
    return Boolean(
      this.inflight ||
        this.states.length ||
        this.dirtyParticipants.size ||
        this.transcript.length ||
        this.chat.length ||
        this.notes.length ||
        this.noticeAt,
    );
  }

  private nextBatch(): IngestBatch | null {
    if (this.inflight) return this.inflight;
    if (!this.hasWork()) return null;
    const b: IngestBatch = { batch_id: randomUUID(), sent_at: iso(Date.now()) };
    let room = MAX_BATCH_ROWS;
    // The summary goes with the ended state (cloud emits meeting.summary as the session closes).
    if (this.summary && this.states[0]?.state === "ended") {
      b.summary = this.summary;
      this.summary = null;
    }
    const state = this.states.shift();
    if (state) b.state = state;
    if (this.dirtyParticipants.size) {
      const ids = Array.from(this.dirtyParticipants).slice(0, room);
      b.participants = ids.map((id) => this.participants.get(id) as IngestParticipant);
      for (const id of ids) this.dirtyParticipants.delete(id);
      room -= ids.length;
    }
    // notice_at is applied by cloud to participants present, so it rides with (or after) them.
    if (this.noticeAt) {
      b.notice_at = this.noticeAt;
      this.noticeAt = null;
    }
    if (room > 0 && this.chat.length) {
      b.chat = this.chat.splice(0, room);
      room -= b.chat.length;
    }
    if (room > 0 && this.transcript.length) {
      b.transcript = this.transcript.splice(0, room);
      room -= b.transcript.length;
    }
    if (room > 0 && this.notes.length) {
      b.screen_notes = this.notes.splice(0, room);
    }
    this.inflight = b;
    return b;
  }

  /**
   * Send everything queued (several batches if needed). A call made while a
   * flush runs joins it: the running loop re-reads the queue before every
   * batch, so rows queued meanwhile go out in the same run.
   */
  flush(): Promise<void> {
    if (this.dead) return Promise.resolve();
    if (this.flushing) return this.flushing;
    this.flushing = (async () => {
      // Yield first, so `this.flushing` is assigned before the finally below can clear it.
      await Promise.resolve();
      try {
        while (!this.dead) {
          const b = this.nextBatch();
          if (!b) return;
          if (!(await this.send(b))) return; // stays in flight; the next tick retries it
        }
      } finally {
        this.flushing = null;
      }
    })();
    return this.flushing;
  }

  private async send(b: IngestBatch): Promise<boolean> {
    const rows = (b.participants?.length ?? 0) + (b.transcript?.length ?? 0) + (b.chat?.length ?? 0) + (b.screen_notes?.length ?? 0);
    let r: { status: number; json: unknown };
    try {
      r = await this.cloud.postJson("ingest", b);
    } catch (e) {
      this.stats.failures++;
      this.log("ingest_error", { batch_id: b.batch_id, error: String(e).slice(0, 200) });
      return false;
    }
    if (r.status === 200 || r.status === 409) {
      this.inflight = null;
      this.stats.batches++;
      this.stats.rows += rows;
      const c = (r.json ?? {}) as { accepted?: number; duplicates?: number; dropped?: number; message?: string };
      this.stats.accepted += c.accepted ?? 0;
      this.stats.duplicates += c.duplicates ?? 0;
      this.stats.dropped += c.dropped ?? 0;
      if (r.status === 409) {
        this.stats.conflicts++;
        this.log("ingest_state_refused", { state: b.state, message: c.message });
      } else {
        this.log("ingest_ok", { batch_id: b.batch_id, state: b.state?.state, rows, ...c });
      }
      return true;
    }
    if (r.status === 401 || r.status === 403 || r.status === 404) {
      this.dead = true;
      this.log("ingest_unauthorized", { status: r.status, body: r.json });
      return false;
    }
    if (r.status === 400) {
      // A malformed batch never gets better by retrying; drop it loudly.
      this.inflight = null;
      this.log("ingest_rejected", { status: r.status, body: r.json, batch_id: b.batch_id });
      return true;
    }
    this.stats.failures++;
    this.log("ingest_retry", { status: r.status, batch_id: b.batch_id });
    return false;
  }

  /** Final flush with retries (the ended state must land), then stop the timer. */
  async stop(deadlineMs = 20_000): Promise<void> {
    const until = Date.now() + deadlineMs;
    let delay = 500;
    while (!this.dead && this.hasWork() && Date.now() < until) {
      await this.flush();
      if (!this.hasWork()) break;
      await sleep(delay);
      delay = Math.min(delay * 2, 4000);
    }
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    if (this.hasWork()) this.log("ingest_unsent_at_exit", { inflight: Boolean(this.inflight) });
  }
}

// ---- control stream (SSE) --------------------------------------------------------

/** Reads GET /control-stream; reconnects with backoff until stopped. */
export class ControlStream {
  private abort: AbortController | null = null;
  private stopped = false;
  connected = false;

  constructor(
    private readonly cloud: CloudClient,
    private readonly onMessage: (m: ControlMessage) => void,
    private readonly log: Log,
  ) {}

  start(): void {
    void this.loop();
  }

  stop(): void {
    this.stopped = true;
    this.abort?.abort();
  }

  private async loop(): Promise<void> {
    let delay = 1000;
    while (!this.stopped) {
      this.abort = new AbortController();
      try {
        const res = await fetch(this.cloud.url("control-stream"), {
          headers: this.cloud.headers({ accept: "text/event-stream" }),
          signal: this.abort.signal,
        });
        if (res.status === 401 || res.status === 403 || res.status === 404) {
          this.log("control_unauthorized", { status: res.status });
          return;
        }
        if (!res.ok || !res.body) throw new Error(`status ${res.status}`);
        this.connected = true;
        this.log("control_connected");
        delay = 1000;
        await this.read(res.body);
        this.log("control_closed");
      } catch (e) {
        if (this.stopped) return;
        this.log("control_error", String(e).slice(0, 200));
      } finally {
        this.connected = false;
      }
      if (this.stopped) return;
      await sleep(delay);
      delay = Math.min(delay * 2, 15_000);
    }
  }

  private async read(body: ReadableStream<Uint8Array>): Promise<void> {
    const reader = body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    let data: string[] = [];
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl).replace(/\r$/, "");
        buf = buf.slice(nl + 1);
        if (line === "") {
          if (data.length) this.dispatch(data.join("\n"));
          data = [];
        } else if (line.startsWith("data:")) {
          data.push(line.slice(5).replace(/^ /, ""));
        }
        // "event:", "id:" and ": ping" comments: the type is inside data.
      }
    }
  }

  private dispatch(raw: string): void {
    try {
      const m = JSON.parse(raw) as ControlMessage;
      if (m && typeof m.type === "string") this.onMessage(m);
    } catch {
      this.log("control_bad_message", raw.slice(0, 120));
    }
  }
}

// ---- speaker timeline --------------------------------------------------------------

/**
 * The adapter's active-speaker polls (about every 250 ms), kept for 15 minutes.
 * An STT segment's speaker is the name speaking nearest its midpoint (within
 * 1 s), as in spike/merge-transcript.ts; confidence is the share of polls in
 * the segment where that name spoke alone. Captions cross-check: when the
 * timeline has nobody, a final caption overlapping the segment names the speaker.
 */
export class SpeakerTimeline {
  private polls: ActiveSpeakerEvent[] = [];
  private captions: Array<{ t: number; speaker: string }> = [];
  private static readonly KEEP_MS = 15 * 60_000;

  add(e: ActiveSpeakerEvent): void {
    this.polls.push(e);
    const cut = e.t - SpeakerTimeline.KEEP_MS;
    if (this.polls.length > 4000 && this.polls[0].t < cut) this.polls = this.polls.filter((p) => p.t >= cut);
  }

  addCaption(t: number, speaker: string): void {
    if (!speaker) return;
    this.captions.push({ t, speaker });
    if (this.captions.length > 2000) this.captions.splice(0, 1000);
  }

  /** Speaker label for [startAt, endAt] (epoch ms). */
  label(startAt: number, endAt: number): { speaker: string | null; confidence: number } {
    const mid = (startAt + endAt) / 2;
    let best: ActiveSpeakerEvent | null = null;
    for (const p of this.polls) {
      if (p.names.length === 0 || Math.abs(p.t - mid) > 1000) continue;
      if (!best || Math.abs(p.t - mid) < Math.abs(best.t - mid)) best = p;
    }
    const inside = this.polls.filter((p) => p.t >= startAt && p.t <= endAt);
    if (best) {
      const name = best.names[0];
      const solo = inside.filter((p) => p.names.length === 1 && p.names[0] === name).length;
      const conf = inside.length ? solo / inside.length : 0.5;
      return { speaker: name, confidence: Math.max(0.05, Math.min(1, conf)) };
    }
    const cap = this.captions.find((c) => c.t >= startAt - 1500 && c.t <= endAt + 1500);
    if (cap) return { speaker: cap.speaker, confidence: 0.4 };
    return { speaker: null, confidence: 0 };
  }
}
