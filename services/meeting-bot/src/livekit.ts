// FALLBACK ONLY (MEETING_BOT_INPAGE_LIVEKIT=1). The pod's publisher is
// livekit-node.ts (@livekit/rtc-node in Node) on every platform since
// 2026-11-14, when Meet's non-extensible RTCPeerConnection broke this client's
// webrtc-adapter shims. This file also holds the shared names (tracks,
// topics) and the RoomBridge interface the runner uses.
//
// LiveKit bridge: runs livekit-client inside the meeting page, next to the
// media it moves.
//
//   publish   the captured call mix (media.ts installPeerCapture) as the audio
//             track `call-audio` in room `meeting-<sid>`. services/voice's
//             meeting_worker subscribes to it for VAD + STT.
//   subscribe audio tracks named `tts` (the worker's Kokoro replies) and play
//             them into the bot's mic (media.ts __gizziMicAttach), so the call
//             hears Gizzi.
//   data      messages on topic `allternit.meeting.stt` (STT segments from the
//             worker) are queued for the runner, which labels the speaker from
//             its active-speaker timeline, assigns `seq` and ingests them.
//
// The token comes from cloud-api's launcher (PodConfig.livekit), minted the
// same way as the call-room tokens in cmd/allternit-cloud-api/src/routes/
// livekit_admin.rs (`participant_access(room, identity, can_publish = true)`:
// HS256 JWT with a `video` grant for one room, canPublish + canSubscribe +
// canPublishData). The pod never sees the LiveKit API secret.
//
// Why in the page and not a Node WebRTC stack: the call audio already exists
// as a MediaStreamTrack there, and Chrome's WebRTC is what the spike proved.
// LiveKit's own peer connections carry `gizziNoCapture: true` in rtcConfig so
// the capture wrapper skips them (the bot never records its own TTS).

import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import type { Page } from "playwright-core";

export const CALL_AUDIO_TRACK = "call-audio";
export const TTS_TRACK = "tts";
export const STT_TOPIC = "allternit.meeting.stt";
/** Worker -> pod: a wake-word question ("Gizzi, ..."). */
export const TURN_TOPIC = "allternit.meeting.turn";
/** Pod -> worker: text to speak on the `tts` track. */
export const SPEAK_TOPIC = "allternit.meeting.speak";

export interface LiveKitTarget {
  url: string;
  room: string;
  token: string;
}

/** One STT segment from the meeting worker. Times are ms from the start of the `call-audio` track. */
export interface SttSegment {
  start_ms: number;
  end_ms: number;
  text: string;
  confidence?: number;
  /** Absolute start (epoch ms) when the worker knows it; preferred over start_ms. */
  start_at?: number;
}

/** One wake-word question from the meeting worker. start_ms is on the `call-audio` clock. */
export interface WakeTurn {
  turn_id: string;
  text: string;
  asker_label: string | null;
  confidence: number;
  start_ms: number;
}

export interface LiveKitStatus {
  state: string;
  published: boolean;
  publishedAt: number | null;
  ttsAttached: number;
  remoteParticipants: string[];
}

export interface BridgeEvent {
  t: number;
  event: string;
  detail?: Record<string, unknown>;
}

/**
 * What the runner needs from a pod <-> worker room bridge. LiveKitBridge
 * (in-page, Meet/Teams) and NodeLiveKitBridge (livekit-node.ts, Node
 * rtc-node, every platform) both implement it.
 */
export interface RoomBridge {
  /** Epoch ms when `call-audio` was published: the clock of the worker's start_ms. */
  readonly publishedAt: number;
  status(): Promise<LiveKitStatus | null>;
  drain(): Promise<{ data: SttSegment[]; turns: WakeTurn[]; events: BridgeEvent[] }>;
  speak(turnId: string, text: string): Promise<boolean>;
  setMuted(on: boolean): Promise<void>;
  disconnect(): Promise<void>;
}

declare global {
  interface Window {
    LivekitClient?: unknown;
    __gizziLk?: {
      status: () => LiveKitStatus;
      drain: () => { data: SttSegment[]; turns: WakeTurn[]; events: BridgeEvent[] };
      send: (topic: string, json: string) => Promise<void>;
      setMuted: (on: boolean) => Promise<void>;
      disconnect: () => Promise<void>;
    };
  }
}

let umdSource: string | null = null;
function livekitUmd(): string {
  if (umdSource) return umdSource;
  const require = createRequire(import.meta.url);
  // The package's "require" export is the UMD build (sets globalThis.LivekitClient).
  const path = require.resolve("livekit-client");
  // Local exports/module/define hide any AMD or CommonJS loader the meeting
  // site has, so the UMD takes its browser-global branch.
  umdSource = `(function(){var exports=undefined,module=undefined,define=undefined;\n${readFileSync(path, "utf8")}\n}).call(globalThis);`;
  return umdSource;
}

interface ConnectArgs {
  url: string;
  token: string;
  callTrack: string;
  ttsTrack: string;
  sttTopic: string;
  turnTopic: string;
}

/** In-page: connect, publish the mix, attach TTS, queue STT data. Self-contained (serialised). */
async function connectInPage(a: ConnectArgs): Promise<{ ok: boolean; error?: string; publishedAt?: number }> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const LK = (window as any).LivekitClient;
  if (!LK) return { ok: false, error: "livekit-client not loaded" };
  if (window.__gizziLk) return { ok: true };
  const cap = window.__gizziPeerCap;
  if (!cap) return { ok: false, error: "peer capture not installed" };

  const events: BridgeEvent[] = [];
  const data: SttSegment[] = [];
  const turns: WakeTurn[] = [];
  const log = (event: string, detail?: Record<string, unknown>) => {
    events.push({ t: Date.now(), event, ...(detail ? { detail } : {}) });
    if (events.length > 500) events.splice(0, events.length - 500);
  };
  const attached = new Map<string, number>();
  let publishedAt: number | null = null;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let pub: any = null;

  const room = new LK.Room({ adaptiveStream: false, dynacast: false, disconnectOnPageLeave: true });
  const E = LK.RoomEvent;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  room.on(E.TrackSubscribed, (track: any, publication: any, participant: any) => {
    if (track.kind !== "audio" || publication.trackName !== a.ttsTrack) return;
    const id = window.__gizziMicAttach?.(new MediaStream([track.mediaStreamTrack]));
    if (id !== undefined) attached.set(publication.trackSid, id);
    log("lk_tts_attached", { from: participant.identity, track: publication.trackSid });
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  room.on(E.TrackUnsubscribed, (_track: any, publication: any) => {
    const id = attached.get(publication.trackSid);
    if (id === undefined) return;
    attached.delete(publication.trackSid);
    window.__gizziMicDetach?.(id);
    log("lk_tts_detached", { track: publication.trackSid });
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  room.on(E.DataReceived, (payload: Uint8Array, participant: any, _kind: unknown, topic?: string) => {
    if (topic !== a.sttTopic && topic !== a.turnTopic) return;
    try {
      const msg = JSON.parse(new TextDecoder().decode(payload));
      if (topic === a.turnTopic) {
        if (msg && typeof msg.turn_id === "string" && typeof msg.text === "string") turns.push(msg);
        return;
      }
      const rows = Array.isArray(msg?.segments) ? msg.segments : [msg];
      for (const r of rows) {
        if (r && typeof r.text === "string" && Number.isFinite(r.start_ms) && Number.isFinite(r.end_ms)) data.push(r);
      }
    } catch (e) {
      log("lk_bad_stt_payload", { from: participant?.identity ?? "", error: String(e).slice(0, 120) });
    }
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  room.on(E.ParticipantConnected, (p: any) => log("lk_participant_joined", { identity: p.identity }));
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  room.on(E.ParticipantDisconnected, (p: any) => log("lk_participant_left", { identity: p.identity }));
  room.on(E.Reconnecting, () => log("lk_reconnecting"));
  room.on(E.Reconnected, () => log("lk_reconnected"));
  room.on(E.Disconnected, (reason: unknown) => log("lk_disconnected", { reason: String(reason) }));

  try {
    await room.connect(a.url, a.token, { autoSubscribe: true, rtcConfig: { gizziNoCapture: true } });
    pub = await room.localParticipant.publishTrack(cap.mixTrack(), {
      name: a.callTrack,
      source: LK.Track.Source.Microphone,
      dtx: false, // continuous frames keep the worker's STT offsets on the pod's clock
      red: false,
      stopMicTrackOnMute: false,
    });
    publishedAt = Date.now();
    log("lk_published", { track: pub.trackSid, room: room.name });
  } catch (e) {
    try {
      await room.disconnect();
    } catch {
      /* ignore */
    }
    return { ok: false, error: String(e).slice(0, 300) };
  }

  window.__gizziLk = {
    status: () => ({
      state: String(room.state),
      published: Boolean(pub),
      publishedAt,
      ttsAttached: attached.size,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      remoteParticipants: Array.from(room.remoteParticipants.values()).map((p: any) => p.identity),
    }),
    drain: () => ({ data: data.splice(0), turns: turns.splice(0), events: events.splice(0) }),
    send: async (topic: string, json: string) => {
      await room.localParticipant.publishData(new TextEncoder().encode(json), { reliable: true, topic });
    },
    setMuted: async (on: boolean) => {
      if (!pub?.track) return;
      if (on) await pub.track.mute();
      else await pub.track.unmute();
    },
    disconnect: async () => {
      for (const id of attached.values()) window.__gizziMicDetach?.(id);
      attached.clear();
      await room.disconnect();
    },
  };
  return { ok: true, publishedAt };
}

export class LiveKitBridge implements RoomBridge {
  private constructor(
    private readonly page: Page,
    readonly publishedAt: number,
  ) {}

  static async start(page: Page, target: LiveKitTarget, onEvent: (kind: string, detail?: unknown) => void): Promise<LiveKitBridge> {
    if (!target.url || !target.token) throw new Error("livekit target missing url or token");
    const loaded = await page.evaluate(() => Boolean(window.LivekitClient));
    if (!loaded) await page.evaluate(livekitUmd());
    const r = await page.evaluate(connectInPage, {
      url: target.url,
      token: target.token,
      callTrack: CALL_AUDIO_TRACK,
      ttsTrack: TTS_TRACK,
      sttTopic: STT_TOPIC,
      turnTopic: TURN_TOPIC,
    });
    if (!r.ok) throw new Error(`livekit connect failed: ${r.error}`);
    onEvent("livekit_connected", { room: target.room, track: CALL_AUDIO_TRACK });
    return new LiveKitBridge(page, r.publishedAt ?? Date.now());
  }

  async status(): Promise<LiveKitStatus | null> {
    if (this.page.isClosed()) return null;
    return this.page.evaluate(() => window.__gizziLk?.status() ?? null).catch(() => null);
  }

  /** STT segments, wake-word turns and bridge events since the last drain. */
  async drain(): Promise<{ data: SttSegment[]; turns: WakeTurn[]; events: BridgeEvent[] }> {
    const empty = { data: [] as SttSegment[], turns: [] as WakeTurn[], events: [] as BridgeEvent[] };
    if (this.page.isClosed()) return empty;
    return this.page.evaluate(() => window.__gizziLk?.drain() ?? { data: [], turns: [], events: [] }).catch(() => empty);
  }

  /** Text for the worker to speak on the `tts` track. */
  async speak(turnId: string, text: string): Promise<boolean> {
    if (this.page.isClosed()) return false;
    const json = JSON.stringify({ turn_id: turnId, text });
    return this.page
      .evaluate(async ({ topic, json }) => {
        if (!window.__gizziLk) return false;
        await window.__gizziLk.send(topic, json);
        return true;
      }, { topic: SPEAK_TOPIC, json })
      .catch(() => false);
  }

  async setMuted(on: boolean): Promise<void> {
    if (this.page.isClosed()) return;
    await this.page.evaluate((v) => window.__gizziLk?.setMuted(v), on).catch(() => undefined);
  }

  async disconnect(): Promise<void> {
    if (this.page.isClosed()) return;
    await this.page.evaluate(() => window.__gizziLk?.disconnect()).catch(() => undefined);
  }
}
