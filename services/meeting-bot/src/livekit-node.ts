// The pod's one LiveKit publisher (every platform): call audio in Node with
// @livekit/rtc-node, room meeting-<sid>, same pod <-> worker names as
// CONTRACT.md "pod <-> worker names".
//
// Where the PCM comes from (a CallAudioPort):
//   Meet/Teams  media.ts PodMedia: the page's peer-track mixer runs an
//               AudioWorklet that sends 48 kHz mono s16 chunks (20 ms) to Node
//               through a Playwright binding; replies go back into the page's
//               virtual mic (the getUserMedia track media.ts hands Meet).
//   Zoom        the Meeting SDK sidecar: mixed call PCM (AUDIO_OUT, 32 kHz) and
//               the virtual mic (AUDIO_IN), adapters/zoom/index.ts.
// It replaced the in-page livekit-client publish, which Meet broke on
// 2026-11-14 (its RTCPeerConnection became non-extensible, so the client's
// webrtc-adapter shims threw "Cannot add property getLocalStreams").
// livekit.ts LiveKitBridge stays only behind MEETING_BOT_INPAGE_LIVEKIT=1.
//
//   publish   track `call-audio`: the call mix, resampled to 48 kHz mono and
//             sent in 10 ms frames. While the source sends nothing (Zoom raw
//             recording not granted yet, a gap) silence is filled in so the
//             track stays continuous on the pod's clock, like DTX off.
//             Paused ("stop listening") = nothing sent; the worker sees the
//             gap and restarts its stream at the real time.
//   subscribe track `tts` (the worker's Kokoro replies, 48 kHz mono) and
//             feed it to the port's mic while speak_replies is on.
//   data      `allternit.meeting.stt` / `allternit.meeting.turn` in, queued
//             for the runner's drain(); `allternit.meeting.speak` out.

import type { Unsubscribe } from "./adapter.js";
import {
  CALL_AUDIO_TRACK,
  SPEAK_TOPIC,
  STT_TOPIC,
  TTS_TRACK,
  TURN_TOPIC,
  type BridgeEvent,
  type LiveKitStatus,
  type LiveKitTarget,
  type RoomBridge,
  type SttSegment,
  type WakeTurn,
} from "./livekit.js";
import { PcmResampler, downmixS16 } from "./pcm.js";

/** One chunk of call audio: s16le PCM bytes, interleaved when channels > 1. */
export interface PcmChunk {
  sampleRate: number;
  channels: number;
  pcm: Uint8Array;
}

/** Where the call audio comes from and where replies go (ZoomAdapter, PodMedia). */
export interface CallAudioPort {
  onAudio(cb: (a: PcmChunk & { t: number }) => void): Unsubscribe;
  /** s16le PCM into the bot's mic. False when it cannot be delivered now. */
  sendAudio(pcm: Uint8Array, sampleRate: number, channels?: number): boolean;
}

export const LK_RATE = 48_000;
const FRAME = LK_RATE / 100; // 10 ms
/** Fill silence only after the source has been quiet this long. */
const GAP_MS = 100;
/** More than this many frames waiting for the source = drop new audio (the source plays in real time). */
const MAX_BACKLOG = 50;

/** Parse one worker data message: STT segments or a wake-word turn. Same checks as the in-page bridge. */
export function parseWorkerData(topic: string | undefined, payload: Uint8Array): { data: SttSegment[]; turn: WakeTurn | null } | null {
  if (topic !== STT_TOPIC && topic !== TURN_TOPIC) return null;
  const msg = JSON.parse(new TextDecoder().decode(payload));
  if (topic === TURN_TOPIC) {
    return { data: [], turn: msg && typeof msg.turn_id === "string" && typeof msg.text === "string" ? (msg as WakeTurn) : null };
  }
  const rows = Array.isArray(msg?.segments) ? msg.segments : [msg];
  const data = rows.filter((r: SttSegment) => r && typeof r.text === "string" && Number.isFinite(r.start_ms) && Number.isFinite(r.end_ms));
  return { data, turn: null };
}

type Rtc = typeof import("@livekit/rtc-node");

export interface NodeBridgeOptions {
  /** Where the PCM comes from, for logs: "zoom_sidecar" or "meet_page" / "teams_page". */
  source: string;
  /** Read live: spoken replies reach the virtual mic only while this is true. */
  speakReplies: () => boolean;
  onEvent: (kind: string, detail?: unknown) => void;
}

export class NodeLiveKitBridge implements RoomBridge {
  private events: BridgeEvent[] = [];
  private data: SttSegment[] = [];
  private turns: WakeTurn[] = [];
  private muted = false;
  private closed = false;
  private resampler: PcmResampler | null = null;
  private pending = new Int16Array(FRAME);
  private pendingLen = 0;
  private chain: Promise<void> = Promise.resolve();
  private backlog = 0;
  /** Clock: samples due = (now - clockBase) * 48. Re-based on unmute so a pause is a gap, not silence. */
  private clockBase: number;
  private lastInAt = 0;
  private filler: ReturnType<typeof setInterval> | null = null;
  private unsubAudio: Unsubscribe | null = null;
  private tts = new Map<string, ReadableStreamDefaultReader<import("@livekit/rtc-node").AudioFrame>>();
  private ttsLevel = 0;
  private ttsLevelAt = 0;
  readonly counters = { inFrames: 0, inSamples: 0, samplesOut: 0, silenceSamples: 0, droppedSamples: 0, ttsFrames: 0, ttsSamplesToMic: 0 };

  /** Set once the worker subscribes to call-audio (audio is reaching it). */
  subscribedAt: number | null = null;
  private firstStt = false;

  private constructor(
    private readonly rtc: Rtc,
    private readonly room: InstanceType<Rtc["Room"]>,
    private readonly source: InstanceType<Rtc["AudioSource"]>,
    private readonly port: CallAudioPort,
    private readonly opts: NodeBridgeOptions,
    readonly publishedAt: number,
  ) {
    this.clockBase = publishedAt;
  }

  static async start(port: CallAudioPort, target: LiveKitTarget, opts: NodeBridgeOptions): Promise<NodeLiveKitBridge> {
    if (!target.url || !target.token) throw new Error("livekit target missing url or token");
    // Loaded here, not at the top: Meet/Teams pods never load the native module.
    const rtc: Rtc = await import("@livekit/rtc-node");
    const room = new rtc.Room();
    const early: Array<(b: NodeLiveKitBridge) => void> = [];
    let bridge: NodeLiveKitBridge | null = null;
    const on = (fn: (b: NodeLiveKitBridge) => void) => (bridge ? fn(bridge) : early.push(fn));
    room
      .on(rtc.RoomEvent.TrackSubscribed, (track, publication, participant) => on((b) => b.onTrack(track, publication.name ?? "", publication.sid ?? "", participant.identity)))
      .on(rtc.RoomEvent.TrackUnsubscribed, (_t, publication) => on((b) => b.dropTts(publication.sid ?? "")))
      .on(rtc.RoomEvent.DataReceived, (payload, participant, _kind, topic) => on((b) => b.onData(payload, participant?.identity ?? "", topic)))
      .on(rtc.RoomEvent.ParticipantConnected, (p) => on((b) => b.log("lk_participant_joined", { identity: p.identity })))
      .on(rtc.RoomEvent.ParticipantDisconnected, (p) => on((b) => b.log("lk_participant_left", { identity: p.identity })))
      .on(rtc.RoomEvent.Reconnecting, () => on((b) => b.log("lk_reconnecting")))
      .on(rtc.RoomEvent.Reconnected, () => on((b) => b.log("lk_reconnected")))
      .on(rtc.RoomEvent.Disconnected, (reason) => on((b) => b.log("lk_disconnected", { reason: String(reason) })));

    const source = new rtc.AudioSource(LK_RATE, 1, 1000);
    try {
      await room.connect(target.url, target.token, { autoSubscribe: true, dynacast: false });
      const track = rtc.LocalAudioTrack.createAudioTrack(CALL_AUDIO_TRACK, source);
      const pub = await room.localParticipant!.publishTrack(
        track,
        new rtc.TrackPublishOptions({ source: rtc.TrackSource.SOURCE_MICROPHONE, dtx: false, red: false }),
      );
      const b = new NodeLiveKitBridge(rtc, room, source, port, opts, Date.now());
      bridge = b;
      b.log("lk_published", { track: pub.sid, room: room.name, via: "rtc-node" });
      // The worker subscribing is the proof that call audio reaches it.
      void pub.waitForSubscription().then(() => {
        b.subscribedAt = Date.now();
        b.log("lk_call_audio_subscribed", { after_ms: b.subscribedAt - b.publishedAt });
      }, () => undefined);
    } catch (e) {
      await room.disconnect().catch(() => undefined);
      await source.close().catch(() => undefined);
      throw e;
    }
    for (const fn of early.splice(0)) fn(bridge);
    bridge.begin();
    opts.onEvent("livekit_connected", { room: target.room, track: CALL_AUDIO_TRACK, via: "rtc-node" });
    opts.onEvent("call_audio_bridge", { phase: "start", source: opts.source, rate: LK_RATE, room: target.room });
    return bridge;
  }

  private log(event: string, detail?: Record<string, unknown>): void {
    this.events.push({ t: Date.now(), event, ...(detail ? { detail } : {}) });
    if (this.events.length > 500) this.events.splice(0, this.events.length - 500);
  }

  private begin(): void {
    this.unsubAudio = this.port.onAudio((a) => this.onCallAudio(a));
    this.filler = setInterval(() => this.fillSilence(), 50);
  }

  // ---- call-audio ---------------------------------------------------------------------

  private onCallAudio(a: PcmChunk & { t: number }): void {
    if (this.closed || this.muted) return;
    // Byte offsets need not be 2-aligned (the sidecar frame body): read the s16le bytes.
    const n = a.pcm.length >> 1;
    const s16 = new Int16Array(n);
    for (let i = 0; i < n; i++) s16[i] = (a.pcm[2 * i] | (a.pcm[2 * i + 1] << 8)) << 16 >> 16;
    const mono = downmixS16(s16, a.channels);
    if (!this.resampler || this.resampler.from !== a.sampleRate) this.resampler = new PcmResampler(a.sampleRate, LK_RATE);
    this.counters.inFrames++;
    this.counters.inSamples += mono.length;
    this.lastInAt = Date.now();
    this.push(this.resampler.process(mono));
  }

  /** Keep the track continuous while the source sends nothing (Zoom before raw recording is granted, or a gap). */
  private fillSilence(): void {
    if (this.closed || this.muted || Date.now() - this.lastInAt < GAP_MS) return;
    const due = Math.floor(((Date.now() - this.clockBase) * LK_RATE) / 1000);
    let behind = due - this.counters.samplesOut - this.pendingLen;
    if (behind < FRAME) return;
    behind = Math.min(behind, LK_RATE); // never more than 1 s at once
    behind -= behind % FRAME;
    this.counters.silenceSamples += behind;
    this.push(new Int16Array(behind));
  }

  private push(pcm: Int16Array): void {
    let off = 0;
    while (off < pcm.length) {
      const take = Math.min(FRAME - this.pendingLen, pcm.length - off);
      this.pending.set(pcm.subarray(off, off + take), this.pendingLen);
      this.pendingLen += take;
      off += take;
      if (this.pendingLen === FRAME) {
        this.capture(this.pending.slice());
        this.pendingLen = 0;
      }
    }
  }

  private capture(frame: Int16Array): void {
    if (this.backlog >= MAX_BACKLOG) {
      this.counters.droppedSamples += frame.length;
      return;
    }
    this.backlog++;
    this.counters.samplesOut += frame.length;
    this.chain = this.chain
      .then(() => (this.closed ? undefined : this.source.captureFrame(new this.rtc.AudioFrame(frame, LK_RATE, 1, frame.length))))
      .catch((e: unknown) => this.log("capture_frame_error", { error: String(e).slice(0, 160) }))
      .finally(() => void this.backlog--);
  }

  // ---- tts -> virtual mic -----------------------------------------------------------------

  private onTrack(track: unknown, name: string, sid: string, from: string): void {
    if (name !== TTS_TRACK || this.closed) return;
    const stream = new this.rtc.AudioStream(track as never, { sampleRate: LK_RATE, numChannels: 1 });
    const reader = stream.getReader();
    this.tts.set(sid, reader);
    this.log("lk_tts_attached", { from, track: sid });
    void (async () => {
      try {
        for (;;) {
          const { value: f, done } = await reader.read();
          if (done || this.closed) break;
          this.counters.ttsFrames++;
          let peak = 0;
          let sum = 0;
          for (const x of f.data) {
            const v = x < 0 ? -x : x;
            if (v > peak) peak = v;
            sum += x * x;
          }
          this.ttsLevel = Math.min(1, (Math.sqrt(sum / Math.max(1, f.data.length)) / 32768) * 3);
          this.ttsLevelAt = Date.now();
          // Digital silence between replies stays off the mic.
          if (peak <= 8 || !this.opts.speakReplies()) continue;
          const bytes = new Uint8Array(f.data.buffer, f.data.byteOffset, f.data.byteLength);
          if (this.port.sendAudio(bytes, f.sampleRate, f.channels)) this.counters.ttsSamplesToMic += f.samplesPerChannel;
        }
      } catch (e) {
        this.log("lk_tts_error", { error: String(e).slice(0, 160) });
      }
    })();
  }

  private dropTts(sid: string): void {
    const r = this.tts.get(sid);
    if (!r) return;
    this.tts.delete(sid);
    void r.cancel().catch(() => undefined);
    this.log("lk_tts_detached", { track: sid });
  }

  /** The bot's own voice level (0..1) for the tile's speaking state. */
  outLevel(): number {
    return Date.now() - this.ttsLevelAt > 300 ? 0 : this.ttsLevel;
  }

  // ---- data -----------------------------------------------------------------------------

  private onData(payload: Uint8Array, from: string, topic?: string): void {
    try {
      const got = parseWorkerData(topic, payload);
      if (!got) return;
      this.data.push(...got.data);
      if (got.data.length && !this.firstStt) {
        this.firstStt = true;
        this.log("lk_first_stt", { from, after_ms: Date.now() - this.publishedAt });
      }
      if (got.turn) this.turns.push(got.turn);
    } catch (e) {
      this.log("lk_bad_stt_payload", { from, error: String(e).slice(0, 120) });
    }
  }

  // ---- RoomBridge -------------------------------------------------------------------------

  async status(): Promise<LiveKitStatus | null> {
    if (this.closed) return null;
    return {
      state: String(this.room.connectionState),
      published: true,
      publishedAt: this.publishedAt,
      ttsAttached: this.tts.size,
      remoteParticipants: Array.from(this.room.remoteParticipants.values()).map((p) => p.identity),
    };
  }

  async drain(): Promise<{ data: SttSegment[]; turns: WakeTurn[]; events: BridgeEvent[] }> {
    return { data: this.data.splice(0), turns: this.turns.splice(0), events: this.events.splice(0) };
  }

  async speak(turnId: string, text: string): Promise<boolean> {
    if (this.closed || !this.room.localParticipant) return false;
    try {
      await this.room.localParticipant.publishData(new TextEncoder().encode(JSON.stringify({ turn_id: turnId, text })), { reliable: true, topic: SPEAK_TOPIC });
      return true;
    } catch {
      return false;
    }
  }

  async setMuted(on: boolean): Promise<void> {
    if (on === this.muted) return;
    this.muted = on;
    this.pendingLen = 0;
    // Unpaused: start the clock again from what was sent, so the pause stays a gap.
    if (!on) this.clockBase = Date.now() - (this.counters.samplesOut * 1000) / LK_RATE;
  }

  async disconnect(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.unsubAudio?.();
    if (this.filler) clearInterval(this.filler);
    for (const sid of [...this.tts.keys()]) this.dropTts(sid);
    this.opts.onEvent("call_audio_bridge", { phase: "stop", source: this.opts.source, subscribed: this.subscribedAt !== null, ...this.counters });
    await this.chain.catch(() => undefined);
    await this.room.disconnect().catch(() => undefined);
    await this.source.close().catch(() => undefined);
  }
}
