// Pod media, both directions, for browser platforms (Meet, Teams).
//
// Outbound (what the call sees and hears from the bot):
//   - camera, live mode (default, live-tile.ts): the /meeting-tile?live=1 page
//     draws its stage into a 640x360 canvas (HTML-in-canvas) and sends it as
//     real video over a WebRTC loopback between the tile tab and this page;
//     getUserMedia hands Meet that received track. The same loopback carries
//     Allternit Live's voice (onto the voice bus below) one way and the call
//     mix (installPeerCapture) the other way, into the Live session's mic.
//   - camera, fallback: a 1280x720 canvas streamed with captureStream(15). It
//     shows the same tile page, opened in its own window and CDP-screencast
//     into the canvas (startTileFeed), when the tile has no live mode.
//   - mic: a WebAudio MediaStreamDestination behind a "voice bus". The TTS
//     track from LiveKit (livekit.ts) is attached to the bus with
//     __gizziMicAttach(); __gizziSpeak(url) plays a file into it. Silent
//     otherwise. No real device is opened, so the pod needs no v4l2loopback
//     or virtual mic.
// Inbound (what the bot hears):
//   - every remote WebRTC audio track of the meeting page, collected by
//     wrapping RTCPeerConnection, mixed into one MediaStreamDestination. Its
//     track is what livekit.ts publishes as `call-audio`. Peer connections
//     created with the config key `gizziNoCapture: true` (LiveKit's own) are
//     never wrapped, so the bot never records its own TTS.
//
// Both in-page installers are passed to context.addInitScript(). Playwright
// serialises them with toString(): they must stay self-contained (no imports,
// no closures over module scope).
//
// Ported from spike/gizzi-media.ts, spike/peer-capture.ts and spike/tile-feed.ts
// (all proven by spike/local-test). Changes: a voice bus with stream attach,
// a capture gain for pause, a level meter on the mix, the MediaRecorder made
// optional (debug only; production audio leaves through LiveKit), and both
// scripts skip the tile page itself (by URL prefix).

import { appendFileSync } from "node:fs";
import type { Browser, BrowserContext, CDPSession, Page } from "playwright-core";
import type { TileState } from "./adapter.js";
import { LiveHub } from "./live-tile.js";

export type { TileState };
export const TILE_STATES: readonly TileState[] = ["joining", "listening", "thinking", "speaking", "paused"];

// ---- in-page: outbound camera + mic ----------------------------------------------

export interface GizziMediaOptions {
  name: string;
  state: string;
  /**
   * URL prefixes (origin + path) where the override must not run: the tile
   * page itself. A prefix, not an origin: /meeting-zoom shares the tile's
   * origin and must get the override.
   */
  skipPrefixes: string[];
  /** Live mode: answer the tile's loopback and use its video as the camera. */
  live?: boolean;
}

export interface CaptureLevels {
  /** Highest 100 ms RMS since the last take (0..1). */
  peak: number;
  /** Mean of the 100 ms RMS values since the last take. */
  mean: number;
  samples: number;
}

export interface PeerCaptureState {
  tracks: number;
  live: number;
  recording: boolean;
  startedAt: number | null;
  bytes: number;
  paused: boolean;
}

export interface PeerCaptureEvent {
  t: number;
  event: string;
  detail?: Record<string, unknown>;
}

interface PeerCaptureApi {
  drain: () => { chunks: string[]; events: PeerCaptureEvent[] };
  flush: () => Promise<void>;
  status: () => PeerCaptureState;
  takeLevels: () => CaptureLevels;
  mixTrack: () => MediaStreamTrack;
  setPaused: (on: boolean) => void;
}

declare global {
  interface Window {
    __gizziSpeak?: (url: string) => Promise<{ durationSec: number }>;
    __gizziMicAttach?: (stream: MediaStream) => number;
    __gizziMicDetach?: (id: number) => void;
    __gizziTileFrame?: (jpegBase64: string) => Promise<boolean>;
    __gizziOutLevel?: () => number;
    __gizziMedia?: { installed: boolean; framesDrawn: number; tileFrames: number; state: string };
    __gizziPeerCap?: PeerCaptureApi;
    /** Node binding (Playwright exposeBinding): 20 ms of the call mix, 48 kHz mono s16le, base64. */
    __gizziPcmOut?: (b64: string) => unknown;
    /** Reply audio from Node (s16le mono, base64) into the voice bus, scheduled gap-free. */
    __gizziMicPcm?: (b64: string, sampleRate: number) => boolean;
    /** Node binding (live-tile.ts): loopback signalling between the tile tab and this page. */
    __gizziLoopSignal?: (msg: LoopMessage) => unknown;
    /** Loopback messages from the tile tab, delivered by Node. */
    __gizziLoopIn?: (msg: LoopMessage) => void;
    __gizziLoop?: { state: () => string; video: () => boolean };
  }
}

/** Loopback signalling (same shapes as allternit-ai src/lib/voice/meeting-live.ts). */
export type LoopMessage =
  | { type: "hello"; side: "tile" | "meet" }
  | { type: "restart" }
  /** Node to the meeting page: no live tile this session, use the canvas camera. */
  | { type: "off" }
  | { type: "offer" | "answer"; sdp: string };

export function installGizziMedia(opts: GizziMediaOptions): void {
  if (opts.skipPrefixes.some((p) => (location.origin + location.pathname).startsWith(p))) return;
  const md = navigator.mediaDevices;
  if (!md || window.__gizziMedia) return;
  // Before installPeerCapture wraps it: the loopback must never be captured as call audio.
  const NativePC: typeof RTCPeerConnection | undefined = window.RTCPeerConnection;

  const W = 1280;
  const H = 720;
  const FPS = 15;
  const CAM_ID = "gizzi-bot-camera";
  const MIC_ID = "gizzi-voice";
  const GROUP_ID = "gizzi";
  const EMPTY = "#1f1f1e"; // graphite --bg-primary, until the first tile frame

  const state = { installed: true, framesDrawn: 0, tileFrames: 0, state: opts.state };
  window.__gizziMedia = state;

  // ---- camera ----------------------------------------------------------------
  let canvas: HTMLCanvasElement | null = null;
  let ctx2d: CanvasRenderingContext2D | null = null;
  let latest: ImageBitmap | null = null;

  // Live video from the tile tab (loopback). The canvas fallback draws it too,
  // for a camera Meet asked for before the loopback was up.
  let loopVideo: MediaStreamTrack | null = null;
  let loopEl: HTMLVideoElement | null = null;
  let videoWaiters: Array<(t: MediaStreamTrack | null) => void> = [];
  function setLoopVideo(t: MediaStreamTrack | null): void {
    loopVideo = t;
    if (t) {
      loopEl ??= document.createElement("video");
      loopEl.muted = true;
      loopEl.srcObject = new MediaStream([t]);
      void loopEl.play().catch(() => undefined);
    }
    const w = videoWaiters;
    videoWaiters = [];
    for (const f of w) f(t);
  }
  function liveVideo(ms: number): Promise<MediaStreamTrack | null> {
    if (loopVideo && loopVideo.readyState === "live") return Promise.resolve(loopVideo);
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(null), ms);
      videoWaiters.push((t) => {
        clearTimeout(timer);
        resolve(t);
      });
    });
  }

  function draw(): void {
    if (!ctx2d) return;
    if (loopEl && loopVideo?.readyState === "live" && loopEl.readyState >= 2) {
      ctx2d.drawImage(loopEl, 0, 0, W, H);
    } else if (latest) {
      ctx2d.drawImage(latest, 0, 0, W, H);
    } else {
      ctx2d.fillStyle = EMPTY;
      ctx2d.fillRect(0, 0, W, H);
    }
    state.framesDrawn++;
  }

  window.__gizziTileFrame = async (b64: string) => {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }));
    latest?.close();
    latest = bitmap;
    state.tileFrames++;
    draw();
    return true;
  };

  function ensureCanvas(): HTMLCanvasElement {
    if (canvas) return canvas;
    canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    ctx2d = canvas.getContext("2d");
    draw();
    // setInterval, not rAF: rAF stops when the tab is hidden and would freeze the camera.
    setInterval(draw, Math.round(1000 / FPS));
    return canvas;
  }

  // ---- mic: voice bus -> destination ---------------------------------------------
  let audioCtx: AudioContext | null = null;
  let dest: MediaStreamAudioDestinationNode | null = null;
  let bus: GainNode | null = null;
  let meter: AnalyserNode | null = null;
  const attached = new Map<number, { el: HTMLAudioElement; src: MediaStreamAudioSourceNode }>();
  let attachSeq = 0;

  function ensureAudio(): { ac: AudioContext; bus: GainNode; dest: MediaStreamAudioDestinationNode } {
    if (audioCtx && dest && bus) return { ac: audioCtx, bus, dest };
    audioCtx = new AudioContext({ sampleRate: 48000 });
    dest = audioCtx.createMediaStreamDestination();
    // A constant zero source keeps the track producing samples: an open, quiet mic.
    const silence = audioCtx.createConstantSource();
    silence.offset.value = 0;
    silence.connect(dest);
    silence.start();
    bus = audioCtx.createGain();
    bus.connect(dest); // into the outgoing mic only, never to the speakers
    meter = audioCtx.createAnalyser();
    meter.fftSize = 1024;
    bus.connect(meter);
    void audioCtx.resume().catch(() => undefined);
    return { ac: audioCtx, bus, dest };
  }

  const samples = new Float32Array(1024);
  window.__gizziOutLevel = () => {
    if (!meter) return 0;
    meter.getFloatTimeDomainData(samples);
    let sum = 0;
    for (const x of samples) sum += x * x;
    return Math.min(1, Math.sqrt(sum / samples.length) * 3);
  };

  window.__gizziSpeak = async (url: string) => {
    const a = ensureAudio();
    if (a.ac.state !== "running") await a.ac.resume();
    const res = await fetch(url);
    if (!res.ok) throw new Error(`__gizziSpeak: ${res.status} for ${url}`);
    const buf = await a.ac.decodeAudioData(await res.arrayBuffer());
    const src = a.ac.createBufferSource();
    src.buffer = buf;
    src.connect(a.bus);
    await new Promise<void>((resolve) => {
      src.onended = () => resolve();
      src.start();
    });
    return { durationSec: buf.duration };
  };

  // A remote stream (the LiveKit TTS track) into the mic. Chrome feeds remote
  // WebRTC audio to WebAudio only while a media element also pulls it, so a
  // muted <audio> holds it.
  window.__gizziMicAttach = (stream: MediaStream) => {
    const a = ensureAudio();
    void a.ac.resume().catch(() => undefined);
    const el = document.createElement("audio");
    el.muted = true;
    el.autoplay = true;
    el.srcObject = stream;
    void el.play().catch(() => undefined);
    const src = a.ac.createMediaStreamSource(stream);
    src.connect(a.bus);
    const id = ++attachSeq;
    attached.set(id, { el, src });
    return id;
  };
  // Reply PCM from Node (livekit-node.ts subscribes to the worker's `tts`
  // track): each chunk is scheduled right after the previous one, with 80 ms
  // of lead after a gap so chunk jitter never clicks.
  let micNext = 0;
  window.__gizziMicPcm = (b64: string, sampleRate: number) => {
    const a = ensureAudio();
    if (a.ac.state !== "running") void a.ac.resume().catch(() => undefined);
    const bin = atob(b64);
    const n = bin.length >> 1;
    if (!n) return false;
    const f = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      const v = (bin.charCodeAt(2 * i) | (bin.charCodeAt(2 * i + 1) << 8)) << 16 >> 16;
      f[i] = v / 32768;
    }
    const buf = a.ac.createBuffer(1, n, sampleRate);
    buf.copyToChannel(f, 0);
    const src = a.ac.createBufferSource();
    src.buffer = buf;
    src.connect(a.bus);
    const now = a.ac.currentTime;
    if (micNext < now + 0.02) micNext = now + 0.08;
    src.start(micNext);
    micNext += buf.duration;
    return true;
  };

  window.__gizziMicDetach = (id: number) => {
    const t = attached.get(id);
    if (!t) return;
    attached.delete(id);
    try {
      t.src.disconnect();
    } catch {
      /* already gone */
    }
    t.el.srcObject = null;
  };

  // ---- the overrides ------------------------------------------------------------------
  function tagTrack(track: MediaStreamTrack, deviceId: string): MediaStreamTrack {
    const orig = track.getSettings.bind(track);
    track.getSettings = () => ({ ...orig(), deviceId, groupId: GROUP_ID });
    return track;
  }

  const origEnumerate = md.enumerateDevices.bind(md);

  // ---- live loopback (this page answers the tile tab's offer) -------------------------
  let loopPc: RTCPeerConnection | null = null;
  let voiceId = 0;
  const loopSignal = (msg: LoopMessage) => {
    try {
      void Promise.resolve(window.__gizziLoopSignal?.(msg)).catch(() => undefined);
    } catch {
      /* binding gone */
    }
  };
  async function answer(sdp: string): Promise<void> {
    if (!NativePC) return;
    loopPc?.close();
    const pc = new NativePC({ iceServers: [] });
    loopPc = pc;
    let gotVideo = false;
    pc.ontrack = (e: RTCTrackEvent) => {
      if (e.track.kind === "video") {
        gotVideo = true;
        setLoopVideo(e.track);
      } else if (e.track.kind === "audio" && window.__gizziMicAttach) {
        // Allternit Live's voice onto the voice bus: the mic Meet already has.
        if (voiceId) window.__gizziMicDetach?.(voiceId);
        voiceId = window.__gizziMicAttach(new MediaStream([e.track]));
      }
    };
    await pc.setRemoteDescription({ type: "offer", sdp });
    // The tile has no video (no HTML-in-canvas): the canvas fallback, fed by Node's screencast.
    if (!gotVideo) setLoopVideo(null);
    const mix = window.__gizziPeerCap?.mixTrack();
    for (const t of pc.getTransceivers()) {
      if (t.receiver.track?.kind !== "audio") continue;
      if (mix) await t.sender.replaceTrack(mix);
      t.direction = "sendrecv";
    }
    await pc.setLocalDescription(await pc.createAnswer());
    if (pc.iceGatheringState !== "complete") {
      await new Promise<void>((resolve) => {
        const done = () => pc.iceGatheringState === "complete" && resolve();
        pc.addEventListener("icegatheringstatechange", done);
        setTimeout(resolve, 3000);
      });
    }
    if (loopPc === pc) loopSignal({ type: "answer", sdp: pc.localDescription?.sdp ?? "" });
  }
  if (opts.live && window.top === window && location.protocol.startsWith("http")) {
    window.__gizziLoopIn = (msg: LoopMessage) => {
      if (msg.type === "offer") void answer(msg.sdp).catch(() => setLoopVideo(null));
      else if (msg.type === "off") {
        delete window.__gizziLoopIn;
        setLoopVideo(null);
      }
    };
    window.__gizziLoop = { state: () => loopPc?.connectionState ?? "none", video: () => loopVideo?.readyState === "live" };
    // The binding appears with the page; say hello once it is there.
    const hello = (n: number) => {
      if (typeof window.__gizziLoopSignal === "function") loopSignal({ type: "hello", side: "meet" });
      else if (n < 50) setTimeout(() => hello(n + 1), 200);
    };
    hello(0);
  }

  md.getUserMedia = async (constraints?: MediaStreamConstraints) => {
    const out = new MediaStream();
    if (constraints?.video) {
      // Live: the tile's own video track (waits for the loopback a little), else the canvas.
      const lv = opts.live && window.__gizziLoopIn ? await liveVideo(15_000) : null;
      out.addTrack(tagTrack(lv ? lv.clone() : ensureCanvas().captureStream(FPS).getVideoTracks()[0], CAM_ID));
    }
    // A clone per call: the page can stop its copy without killing ours.
    if (constraints?.audio) out.addTrack(tagTrack(ensureAudio().dest.stream.getAudioTracks()[0].clone(), MIC_ID));
    if (out.getTracks().length === 0) throw new DOMException("No audio or video requested", "TypeError");
    return out;
  };

  md.enumerateDevices = async () => {
    let real: MediaDeviceInfo[] = [];
    try {
      real = await origEnumerate();
    } catch {
      real = [];
    }
    const fake = (deviceId: string, kind: MediaDeviceKind, label: string): MediaDeviceInfo =>
      ({
        deviceId,
        kind,
        label,
        groupId: GROUP_ID,
        toJSON() {
          return { deviceId, kind, label, groupId: GROUP_ID };
        },
      }) as MediaDeviceInfo;
    return [
      fake(MIC_ID, "audioinput", `${opts.name} voice`),
      fake(CAM_ID, "videoinput", `${opts.name} camera`),
      ...real.filter((d) => d.kind === "audiooutput"),
    ];
  };
}

// ---- in-page: inbound call audio --------------------------------------------------

export interface PeerCaptureOptions {
  timesliceMs: number;
  /** Also record the mix with MediaRecorder (debug; drained to a .webm file by Node). */
  record: boolean;
  skipPrefixes: string[];
}

export function installPeerCapture(opts: PeerCaptureOptions): void {
  if (opts.skipPrefixes.some((p) => (location.origin + location.pathname).startsWith(p))) return;
  const w = window as Window & { RTCPeerConnection?: typeof RTCPeerConnection; webkitRTCPeerConnection?: unknown };
  if (!w.RTCPeerConnection || w.__gizziPeerCap) return;
  const Orig: typeof RTCPeerConnection = w.RTCPeerConnection;

  const events: PeerCaptureEvent[] = [];
  const chunks: string[] = [];
  const pcs = new Set<RTCPeerConnection>();
  const tracks = new Map<string, { track: MediaStreamTrack; el: HTMLAudioElement; src: MediaStreamAudioSourceNode | null; ended: boolean }>();
  let ctx: AudioContext | null = null;
  let dest: MediaStreamAudioDestinationNode | null = null;
  let master: GainNode | null = null;
  let meter: AnalyserNode | null = null;
  let rec: MediaRecorder | null = null;
  let startedAt: number | null = null;
  let bytes = 0;
  let paused = false;
  let queue: Promise<void> = Promise.resolve();
  let pcSeq = 0;
  const pcIds = new WeakMap<RTCPeerConnection, number>();
  let lvPeak = 0;
  let lvSum = 0;
  let lvN = 0;

  const log = (event: string, detail?: Record<string, unknown>) => {
    events.push({ t: Date.now(), event, ...(detail ? { detail } : {}) });
    if (events.length > 2000) events.splice(0, events.length - 2000);
  };

  function toB64(buf: Uint8Array): string {
    let bin = "";
    const step = 0x8000;
    for (let i = 0; i < buf.length; i += step) bin += String.fromCharCode(...buf.subarray(i, i + step));
    return btoa(bin);
  }

  function ensureMixer(): GainNode {
    if (ctx && master) return master;
    ctx = new AudioContext({ sampleRate: 48000 });
    dest = ctx.createMediaStreamDestination();
    // A zero source keeps the mix producing samples between talkers, so the
    // published track's timeline stays continuous (STT offsets line up).
    const silence = ctx.createConstantSource();
    silence.offset.value = 0;
    silence.connect(dest);
    silence.start();
    master = ctx.createGain();
    master.gain.value = paused ? 0 : 1;
    master.connect(dest);
    meter = ctx.createAnalyser();
    meter.fftSize = 2048;
    master.connect(meter);
    const buf = new Float32Array(meter.fftSize);
    setInterval(() => {
      if (!meter) return;
      meter.getFloatTimeDomainData(buf);
      let s = 0;
      for (const x of buf) s += x * x;
      const r = Math.sqrt(s / buf.length);
      lvPeak = Math.max(lvPeak, r);
      lvSum += r;
      lvN++;
    }, 100);
    void ctx.resume().catch(() => undefined);
    startedAt = Date.now();
    if (opts.record) {
      rec = new MediaRecorder(dest.stream, { mimeType: "audio/webm;codecs=opus" });
      rec.ondataavailable = (e: BlobEvent) => {
        if (!e.data || e.data.size === 0) return;
        queue = queue.then(async () => {
          const b = new Uint8Array(await e.data.arrayBuffer());
          bytes += b.length;
          chunks.push(toB64(b));
        });
      };
      rec.onerror = (e: Event) => log("peer_recorder_error", { error: String((e as unknown as { error?: unknown }).error ?? e.type) });
      rec.start(opts.timesliceMs);
    }
    log("peer_mixer_started", { sampleRate: ctx.sampleRate, ctxState: ctx.state, recording: Boolean(rec) });
    void startPcmTap(ctx, master);
    return master;
  }

  // The call mix to Node as 20 ms s16 chunks (livekit-node.ts publishes them
  // as `call-audio`). An AudioWorklet does the float->s16 work off the main
  // thread; ScriptProcessor is the fallback if the worklet cannot load.
  async function startPcmTap(ac: AudioContext, from: AudioNode): Promise<void> {
    const send = w.__gizziPcmOut;
    if (typeof send !== "function") return log("pcm_tap_unavailable", { reason: "no binding" });
    const CHUNK = Math.round(ac.sampleRate / 50);
    const post = (buf: ArrayBuffer) => {
      const u = new Uint8Array(buf);
      let s = "";
      for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000) as unknown as number[]);
      void Promise.resolve(send(btoa(s))).catch(() => undefined);
    };
    const sink = dest as MediaStreamAudioDestinationNode;
    try {
      const code = `class GizziPcmTap extends AudioWorkletProcessor {
  constructor() { super(); this.n = 0; this.b = new Int16Array(${CHUNK}); }
  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    const len = ch ? ch.length : 128;
    for (let k = 0; k < len; k++) {
      let v = ch ? ch[k] : 0;
      v = v < -1 ? -1 : v > 1 ? 1 : v;
      this.b[this.n++] = v < 0 ? v * 32768 : v * 32767;
      if (this.n === this.b.length) { this.port.postMessage(this.b.buffer, [this.b.buffer]); this.b = new Int16Array(${CHUNK}); this.n = 0; }
    }
    return true;
  }
}
registerProcessor("gizzi-pcm-tap", GizziPcmTap);`;
      const url = URL.createObjectURL(new Blob([code], { type: "application/javascript" }));
      await ac.audioWorklet.addModule(url);
      URL.revokeObjectURL(url);
      const node = new AudioWorkletNode(ac, "gizzi-pcm-tap", { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1], channelCount: 1, channelCountMode: "explicit" });
      node.port.onmessage = (e: MessageEvent) => post(e.data as ArrayBuffer);
      from.connect(node);
      node.connect(sink); // pulled by the graph; it outputs silence
      log("pcm_tap_started", { kind: "worklet", sampleRate: ac.sampleRate, chunk: CHUNK });
    } catch (e) {
      const sp = ac.createScriptProcessor(1024, 1, 1);
      sp.onaudioprocess = (ev: AudioProcessingEvent) => {
        const ch = ev.inputBuffer.getChannelData(0);
        const out = new Int16Array(ch.length);
        for (let k = 0; k < ch.length; k++) {
          const v = ch[k] < -1 ? -1 : ch[k] > 1 ? 1 : ch[k];
          out[k] = v < 0 ? v * 32768 : v * 32767;
        }
        post(out.buffer);
      };
      from.connect(sp);
      sp.connect(sink);
      log("pcm_tap_started", { kind: "script_processor", sampleRate: ac.sampleRate, error: String(e).slice(0, 120) });
    }
  }

  function localTrackIds(): Set<string> {
    const ids = new Set<string>();
    for (const pc of pcs) {
      try {
        for (const s of pc.getSenders()) if (s.track) ids.add(s.track.id);
      } catch {
        /* closed */
      }
    }
    return ids;
  }

  function markEnded(id: string, why: string): void {
    const t = tracks.get(id);
    if (!t || t.ended) return;
    t.ended = true;
    try {
      t.src?.disconnect();
    } catch {
      /* ignore */
    }
    t.el.srcObject = null;
    log("peer_track_ended", { id, why });
  }

  function addTrack(track: MediaStreamTrack | null | undefined, pc: RTCPeerConnection, via: string): void {
    if (!track || track.kind !== "audio" || tracks.has(track.id)) return;
    if (track.readyState === "ended") return;
    if (localTrackIds().has(track.id)) return; // our own outbound media, never recorded
    const m = ensureMixer();
    const stream = new MediaStream([track]);
    const el = document.createElement("audio");
    el.muted = true;
    el.autoplay = true;
    el.srcObject = stream;
    void el.play().catch(() => undefined);
    let src: MediaStreamAudioSourceNode | null = null;
    try {
      src = (ctx as AudioContext).createMediaStreamSource(stream);
      src.connect(m);
    } catch (e) {
      log("peer_track_mix_error", { id: track.id, error: String(e) });
    }
    tracks.set(track.id, { track, el, src, ended: false });
    track.addEventListener("ended", () => markEnded(track.id, "ended_event"));
    log("peer_track_added", { id: track.id, muted: track.muted, pc: pcIds.get(pc) ?? 0, via });
  }

  function scan(pc: RTCPeerConnection, via: string): void {
    try {
      for (const t of pc.getTransceivers()) {
        const dir = t.currentDirection;
        if (dir === "sendrecv" || dir === "recvonly") addTrack(t.receiver.track, pc, via);
      }
    } catch {
      /* closed */
    }
  }

  function watch(pc: RTCPeerConnection): void {
    if (pcs.has(pc)) return;
    pcs.add(pc);
    pcIds.set(pc, ++pcSeq);
    log("peer_connection_created", { pc: pcSeq });
    pc.addEventListener("track", (e: Event) => addTrack((e as RTCTrackEvent).track, pc, "track_event"));
  }

  class GizziRTCPeerConnection extends Orig {
    private readonly gizziSkip: boolean;
    constructor(...a: ConstructorParameters<typeof RTCPeerConnection>) {
      super(...a);
      // LiveKit's own connections (livekit.ts passes this key in rtcConfig).
      this.gizziSkip = Boolean((a[0] as (RTCConfiguration & { gizziNoCapture?: boolean }) | undefined)?.gizziNoCapture);
      if (!this.gizziSkip) watch(this);
    }
    setRemoteDescription(...a: unknown[]): Promise<void> {
      const p = (Orig.prototype.setRemoteDescription as (...x: unknown[]) => Promise<void>).apply(this, a);
      if (!this.gizziSkip) void Promise.resolve(p).then(() => scan(this, "receivers_after_srd"), () => undefined);
      return p;
    }
    close(): void {
      Orig.prototype.close.call(this);
      if (this.gizziSkip) return;
      try {
        for (const r of this.getReceivers()) if (r.track) markEnded(r.track.id, "pc_closed");
      } catch {
        /* ignore */
      }
      pcs.delete(this);
    }
  }
  Object.defineProperty(GizziRTCPeerConnection, "name", { value: "RTCPeerConnection" });
  w.RTCPeerConnection = GizziRTCPeerConnection as unknown as typeof RTCPeerConnection;
  if ("webkitRTCPeerConnection" in w) w.webkitRTCPeerConnection = GizziRTCPeerConnection;

  setInterval(() => {
    for (const pc of pcs) scan(pc, "rescan");
    for (const [id, t] of tracks) if (!t.ended && t.track.readyState === "ended") markEnded(id, "ready_state");
  }, 1000);

  w.__gizziPeerCap = {
    drain: () => ({ chunks: chunks.splice(0), events: events.splice(0) }),
    flush: () =>
      new Promise<void>((resolve) => {
        if (!rec || rec.state !== "recording") return resolve();
        const r = rec;
        const once = () => {
          r.removeEventListener("dataavailable", once);
          setTimeout(() => void queue.then(() => resolve()), 0);
        };
        r.addEventListener("dataavailable", once);
        r.requestData();
        setTimeout(resolve, 2000);
      }),
    status: () => ({
      tracks: tracks.size,
      live: Array.from(tracks.values()).filter((t) => !t.ended).length,
      recording: rec?.state === "recording",
      startedAt,
      bytes,
      paused,
    }),
    takeLevels: () => {
      const out = { peak: lvPeak, mean: lvN ? lvSum / lvN : 0, samples: lvN };
      lvPeak = 0;
      lvSum = 0;
      lvN = 0;
      return out;
    },
    mixTrack: () => {
      ensureMixer();
      return (dest as MediaStreamAudioDestinationNode).stream.getAudioTracks()[0];
    },
    setPaused: (on: boolean) => {
      paused = on;
      if (master) master.gain.value = on ? 0 : 1;
      log(on ? "capture_paused" : "capture_resumed");
    },
  };
}

// ---- Node side: tile feed (spike/tile-feed.ts) ----------------------------------------

export interface TileFeedOptions {
  url: string;
  name: string;
  state?: TileState;
  /** base64 JSON of the user's own bot (an allternit-ai Agent); absent = Gizzi. */
  agent?: string;
  /** Layout size of the tile page (CSS px). */
  width?: number;
  height?: number;
  /** Size of the screencast frames (default the layout size); smaller frames are scaled down by Chrome. */
  frameWidth?: number;
  frameHeight?: number;
  fps?: number;
  loadTimeoutMs?: number;
  onEvent?: (kind: string, detail?: unknown) => void;
}

export interface TileFeed {
  tilePage: Page;
  setState(state: TileState): Promise<void>;
  setLevel(level: number): void;
  stats(): { framesIn: number; framesSent: number; framesDropped: number };
  stop(): Promise<void>;
}

/** Resolves with `fallback` after `ms` if `p` has not settled (never rejects). */
export function within<T>(p: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise<T>((resolve) => {
    const t = setTimeout(() => resolve(fallback), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      () => {
        clearTimeout(t);
        resolve(fallback);
      },
    );
  });
}

export function tileUrl(base: string, opts: { name: string; state: TileState; agent?: string; live?: boolean }): string {
  const u = new URL(base);
  u.searchParams.set("name", opts.name);
  u.searchParams.set("state", opts.state);
  if (opts.agent) u.searchParams.set("agent", opts.agent);
  if (opts.live) u.searchParams.set("live", "1");
  return u.toString();
}

/**
 * Open the tile page in a second window (not a tab: a background tab is
 * "hidden" and the stage pauses) at the stage size, and wait until it drew.
 */
export async function openTileWindow(
  browser: Browser,
  context: BrowserContext,
  url: string,
  opts: { width?: number; height?: number; loadTimeoutMs?: number } = {},
): Promise<{ tilePage: Page; tile: CDPSession }> {
  const width = opts.width ?? 1280;
  const height = opts.height ?? 720;
  const browserSession = await browser.newBrowserCDPSession();
  const pageEvent = context.waitForEvent("page", { timeout: 15_000 });
  await browserSession.send("Target.createTarget", { url, newWindow: true, background: true, width, height });
  const tilePage = await pageEvent;
  await browserSession.detach().catch(() => undefined);
  try {
    const tile: CDPSession = await context.newCDPSession(tilePage);
    await tile.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await tilePage.waitForFunction(() => (window as unknown as { __tileReady?: boolean }).__tileReady === true, null, {
      timeout: opts.loadTimeoutMs ?? 45_000,
    });
    return { tilePage, tile };
  } catch (e) {
    await tilePage.close().catch(() => undefined);
    throw e;
  }
}

export async function startTileFeed(browser: Browser, meetPage: Page, opts: TileFeedOptions): Promise<TileFeed> {
  const url = tileUrl(opts.url, { name: opts.name, state: opts.state ?? "joining", agent: opts.agent });
  const { tilePage, tile } = await openTileWindow(browser, meetPage.context(), url, opts);
  (opts.onEvent ?? (() => undefined))("tile_ready", { url: opts.url });
  return screencastTile(tilePage, tile, meetPage, opts);
}

/** CDP-screencast an open tile page into the meeting page's canvas camera. */
export async function screencastTile(
  tilePage: Page,
  tile: CDPSession,
  meetPage: Page,
  opts: Pick<TileFeedOptions, "width" | "height" | "frameWidth" | "frameHeight" | "fps" | "onEvent">,
): Promise<TileFeed> {
  const width = opts.width ?? 1280;
  const height = opts.height ?? 720;
  const frameWidth = opts.frameWidth ?? width;
  const frameHeight = opts.frameHeight ?? height;
  const fps = opts.fps ?? 15;
  const frameMs = Math.round(1000 / fps);
  const log = opts.onEvent ?? (() => undefined);
  const context = meetPage.context();

  try {

    // Frames go to the meeting page with a raw Runtime.evaluate; one in flight,
    // a newer frame replaces a waiting one.
    const meet: CDPSession = await context.newCDPSession(meetPage);
    let framesIn = 0;
    let framesSent = 0;
    let framesDropped = 0;
    let inFlight = false;
    let waiting: string | null = null;
    let stopped = false;
    const send = (data: string) => {
      if (inFlight) {
        if (waiting) framesDropped++;
        waiting = data;
        return;
      }
      inFlight = true;
      framesSent++;
      meet
        .send("Runtime.evaluate", { expression: `window.__gizziTileFrame && window.__gizziTileFrame("${data}")`, returnByValue: true })
        .catch(() => undefined)
        .finally(() => {
          inFlight = false;
          if (waiting && !stopped) {
            const next = waiting;
            waiting = null;
            send(next);
          }
        });
    };

    // Chrome keeps two screencast frames unacked; spacing acks frameMs apart holds the source at FPS.
    let nextAckAt = 0;
    tile.on("Page.screencastFrame", (f: { data: string; sessionId: number }) => {
      framesIn++;
      send(f.data);
      const now = Date.now();
      nextAckAt = Math.max(now, nextAckAt + frameMs);
      setTimeout(() => {
        if (!stopped) void tile.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => undefined);
      }, nextAckAt - now);
    });
    await tile.send("Page.startScreencast", { format: "jpeg", quality: 80, maxWidth: frameWidth, maxHeight: frameHeight, everyNthFrame: 1 });
    log("tile_screencast_started", { width: frameWidth, height: frameHeight, fps });

    return {
      tilePage,
      async setState(state) {
        // Bounded: the tile page's main thread has been seen blocked for minutes
        // (live Meet pod 2026-11-14: tile_state ok:false 5 min late), and the
        // runner awaits this before a turn.
        const ok = await within(
          tilePage.evaluate((s) => (window as unknown as { __setTileState?: (x: string) => boolean }).__setTileState?.(s) ?? false, state),
          2000,
          false,
        );
        log("tile_state", { state, ok });
      },
      setLevel(level) {
        void tile
          .send("Runtime.evaluate", { expression: `window.__setTileLevel && window.__setTileLevel(${Number(level) || 0})` })
          .catch(() => undefined);
      },
      stats: () => ({ framesIn, framesSent, framesDropped }),
      async stop() {
        stopped = true;
        await within(tile.send("Page.stopScreencast"), 2000, undefined);
        await within(tile.detach(), 2000, undefined);
        await within(meet.detach(), 2000, undefined);
        await within(tilePage.close({ runBeforeUnload: false }), 3000, undefined);
      },
    };
  } catch (e) {
    await tilePage.close().catch(() => undefined);
    throw e;
  }
}

// ---- Node side: the pod media controller ---------------------------------------------

export interface PodMediaOptions {
  displayName: string;
  tileUrl: string;
  agent?: string;
  /** Debug: also record the call mix to this .webm file. */
  recordFile?: string;
  /** Live mode (live-tile.ts): the tile is an Allternit Live voice session with real video. */
  live?: boolean;
  onEvent?: (kind: string, detail?: unknown) => void;
}

/**
 * The runner's handle on the meeting page's media. Created by
 * `installPodMedia`, which browser adapters call through their `installMedia`
 * option before the meeting page loads.
 */
export class PodMedia {
  page: Page | null = null;
  tile: TileFeed | null = null;
  /** Live mode's hub; null when live mode is off or the tile has none. */
  live: LiveHub | null = null;
  private tileState: TileState = "joining";
  private recordBytes = 0;
  private chain: Promise<unknown> = Promise.resolve();
  private cdp: CDPSession | null = null;
  private pcmSubs = new Set<(a: { sampleRate: number; channels: number; pcm: Uint8Array; t: number }) => void>();
  private micQueue: Uint8Array[] = [];
  private micBytes = 0;
  private micRate = 48_000;
  private micTimer: ReturnType<typeof setTimeout> | null = null;
  private micInFlight = false;
  readonly pcmStats = { chunksIn: 0, micChunksOut: 0 };
  constructor(readonly opts: PodMediaOptions) {}

  // ---- CallAudioPort (livekit-node.ts) -----------------------------------------------

  /** The call mix from the page's PCM tap: 20 ms chunks, 48 kHz mono s16le. */
  onAudio(cb: (a: { sampleRate: number; channels: number; pcm: Uint8Array; t: number }) => void): () => void {
    this.pcmSubs.add(cb);
    return () => this.pcmSubs.delete(cb);
  }

  /** Reply PCM into the page's virtual mic, batched to ~100 ms per CDP call. */
  sendAudio(pcm: Uint8Array, sampleRate: number, channels = 1): boolean {
    if (!this.page || this.page.isClosed() || !this.cdp || channels !== 1) return false;
    if (sampleRate !== this.micRate) {
      this.micQueue = [];
      this.micBytes = 0;
      this.micRate = sampleRate;
    }
    this.micQueue.push(pcm.slice());
    this.micBytes += pcm.length;
    if (this.micBytes >= (sampleRate / 10) * 2) this.flushMic();
    else this.micTimer ??= setTimeout(() => this.flushMic(), 100);
    return true;
  }

  private flushMic(): void {
    if (this.micTimer) clearTimeout(this.micTimer);
    this.micTimer = null;
    if (this.micInFlight || !this.micBytes || !this.cdp) return;
    const all = Buffer.concat(this.micQueue);
    this.micQueue = [];
    this.micBytes = 0;
    this.micInFlight = true;
    this.pcmStats.micChunksOut++;
    void this.cdp
      .send("Runtime.evaluate", { expression: `window.__gizziMicPcm && window.__gizziMicPcm("${all.toString("base64")}", ${this.micRate})` })
      .catch(() => undefined)
      .finally(() => {
        this.micInFlight = false;
        if (this.micBytes) this.flushMic();
      });
  }

  private log(kind: string, detail?: unknown): void {
    this.opts.onEvent?.(kind, detail);
  }

  /** The `installMedia` hook browser adapters call: init scripts, CSP bypass, tile window. */
  async install(ctx: BrowserContext, browser: Browser, page: Page): Promise<void> {
    this.page = page;
    let tilePrefix = "";
    try {
      const u = new URL(this.opts.tileUrl);
      tilePrefix = u.origin + u.pathname;
    } catch {
      /* no tile */
    }
    const skipPrefixes = tilePrefix ? [tilePrefix] : [];
    // LiveKit runs inside the meeting page (livekit.ts). The meeting site's
    // CSP (connect-src, Trusted Types) would block its websocket and script,
    // so CSP is bypassed for the meeting page only. Init scripts are not
    // affected by CSP either way.
    const cdp = await ctx.newCDPSession(page);
    this.cdp = cdp;
    await cdp.send("Page.setBypassCSP", { enabled: true }).catch((e: unknown) => this.log("csp_bypass_failed", String(e)));
    // The page's PCM tap calls this with 20 ms of the call mix (48 kHz mono s16le, base64).
    await ctx
      .exposeBinding("__gizziPcmOut", (_src, b64: string) => {
        if (typeof b64 !== "string" || !this.pcmSubs.size) return;
        this.pcmStats.chunksIn++;
        const pcm = Buffer.from(b64, "base64");
        const t = Date.now();
        for (const cb of this.pcmSubs) cb({ sampleRate: 48_000, channels: 1, pcm, t });
      })
      .catch((e: unknown) => this.log("pcm_binding_failed", String(e).slice(0, 200)));
    let live = Boolean(this.opts.live && this.opts.tileUrl);
    if (live) {
      const hub = new LiveHub(page, { onEvent: (k, d) => this.log(k, d) });
      try {
        await hub.bind(ctx);
        this.live = hub;
      } catch (e) {
        live = false;
        this.log("live_unavailable", { reason: "bindings", error: String(e).slice(0, 200) });
      }
    }
    await ctx.addInitScript(installGizziMedia, { name: this.opts.displayName, state: "Joining", skipPrefixes, live });
    await ctx.addInitScript(installPeerCapture, { timesliceMs: 1000, record: Boolean(this.opts.recordFile), skipPrefixes });
    if (this.live && this.opts.tileUrl) {
      // No screencast: the tile sends its stage as video and runs the voice session.
      this.tile = await this.live
        .open(browser, this.opts.tileUrl, { name: this.opts.displayName, agent: this.opts.agent })
        .catch((e: unknown) => {
          this.log("live_unavailable", { reason: "tile", error: String(e).slice(0, 300) });
          this.live?.disable();
          this.live = null;
          return null;
        });
    }
    if (this.opts.tileUrl && !this.tile) {
      this.tile = await startTileFeed(browser, page, {
        url: this.opts.tileUrl,
        name: this.opts.displayName,
        state: "joining",
        agent: this.opts.agent,
        onEvent: (k, d) => this.log(k, d),
      }).catch((e: unknown) => {
        // The call still works with Meet's initial instead of our tile.
        this.log("tile_unavailable", { url: this.opts.tileUrl, error: String(e).slice(0, 300) });
        return null;
      });
    }
  }

  async setTile(state: TileState): Promise<void> {
    if (state === this.tileState) return;
    this.tileState = state;
    await this.tile?.setState(state);
  }

  get currentTile(): TileState {
    return this.tileState;
  }

  setLevel(level: number): void {
    this.tile?.setLevel(level);
  }

  /** Live's voice is up: replies are spoken by it, not by the worker or in chat. */
  get liveReady(): boolean {
    return this.live?.voiceReady ?? false;
  }

  /** Speak through Live; false = not delivered (the caller falls back to chat). */
  liveSay(text: string): Promise<boolean> {
    return this.live ? this.live.say(text) : Promise.resolve(false);
  }

  /** The bot's own voice level (TTS on the mic), 0..1. */
  async outLevel(): Promise<number> {
    if (!this.page || this.page.isClosed()) return 0;
    return this.page.evaluate(() => window.__gizziOutLevel?.() ?? 0).catch(() => 0);
  }

  async captureStatus(): Promise<PeerCaptureState | null> {
    if (!this.page || this.page.isClosed()) return null;
    return this.page.evaluate(() => window.__gizziPeerCap?.status() ?? null).catch(() => null);
  }

  async takeLevels(): Promise<CaptureLevels | null> {
    if (!this.page || this.page.isClosed()) return null;
    return this.page.evaluate(() => window.__gizziPeerCap?.takeLevels() ?? null).catch(() => null);
  }

  async setCapturePaused(on: boolean): Promise<void> {
    if (!this.page || this.page.isClosed()) return;
    await this.page.evaluate((v) => window.__gizziPeerCap?.setPaused(v), on).catch(() => undefined);
  }

  /** Pull track events (and recorder chunks when recording). Serialised so chunks keep their order. */
  drain(flush = false): Promise<unknown> {
    const page = this.page;
    if (!page) return Promise.resolve();
    this.chain = this.chain
      .then(async () => {
        if (page.isClosed()) return;
        const got = await page
          .evaluate(async (f) => {
            const cap = window.__gizziPeerCap;
            if (!cap) return { chunks: [] as string[], events: [] as PeerCaptureEvent[] };
            if (f) await cap.flush();
            return cap.drain();
          }, flush)
          .catch(() => ({ chunks: [] as string[], events: [] as PeerCaptureEvent[] }));
        for (const e of got.events) this.log(e.event, e.detail);
        if (this.opts.recordFile) {
          for (const b64 of got.chunks) {
            const buf = Buffer.from(b64, "base64");
            appendFileSync(this.opts.recordFile, buf);
            this.recordBytes += buf.length;
          }
        }
      })
      .catch(() => undefined);
    return this.chain;
  }

  get recordedBytes(): number {
    return this.recordBytes;
  }

  async stop(): Promise<void> {
    if (this.tile) this.log("tile_stats", this.tile.stats());
    await this.tile?.stop();
    this.tile = null;
  }
}
