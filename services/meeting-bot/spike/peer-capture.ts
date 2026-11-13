// Call-audio capture from the WebRTC peer connections themselves.
//
// installPeerCapture() is an init script (context.addInitScript): it runs in
// every frame before page scripts and wraps window.RTCPeerConnection, so every
// remote audio track the page receives is collected, whether the page plays it
// through an <audio> element or not (Meet does not reliably expose remote audio
// as element srcObjects, which is why the old element-based capture recorded
// nothing). Sources of tracks:
//   - the 'track' event on every connection made through the wrapper
//   - receivers already present after setRemoteDescription (transceivers that
//     exist before our listener saw them)
//   - a 1 s rescan of every connection's receivers (tracks added later)
// Only receiver tracks are taken, so the bot's own outbound media (the
// gizzi-media.ts mic and camera, which live on senders) is never recorded.
//
// All remote audio tracks are mixed into one MediaStreamAudioDestinationNode
// and recorded with MediaRecorder (audio/webm;codecs=opus, 1 s timeslice).
// Chrome quirk: remote WebRTC audio fed to WebAudio is silent unless the
// stream is also attached to a media element, so each track also gets a muted
// HTMLAudioElement. Node pulls chunks and track events with drainPeerCapture()
// and appends them to <out>/call.webm and <out>/events.jsonl.
//
// Playwright serialises installPeerCapture with toString(): it must stay
// self-contained (no imports, no closures over module scope).

import { appendFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "playwright-core";

export interface PeerCaptureOptions {
  timesliceMs: number;
}

export interface PeerCaptureEvent {
  t: number;
  event: string;
  detail?: Record<string, unknown>;
}

export interface PeerCaptureState {
  tracks: number;
  live: number;
  recording: boolean;
  startedAt: number | null;
  bytes: number;
}

interface PeerCaptureApi {
  drain: () => { chunks: string[]; events: PeerCaptureEvent[] };
  flush: () => Promise<void>;
  status: () => PeerCaptureState;
}

declare global {
  interface Window {
    __gizziPeerCap?: PeerCaptureApi;
  }
}

export function installPeerCapture(opts: PeerCaptureOptions): void {
  const w = window as Window & { RTCPeerConnection?: typeof RTCPeerConnection; webkitRTCPeerConnection?: unknown };
  if (!w.RTCPeerConnection || w.__gizziPeerCap) return;
  const Orig: typeof RTCPeerConnection = w.RTCPeerConnection;

  const events: PeerCaptureEvent[] = [];
  const chunks: string[] = [];
  const pcs = new Set<RTCPeerConnection>();
  const tracks = new Map<string, { track: MediaStreamTrack; el: HTMLAudioElement; src: MediaStreamAudioSourceNode | null; ended: boolean }>();
  let ctx: AudioContext | null = null;
  let dest: MediaStreamAudioDestinationNode | null = null;
  let rec: MediaRecorder | null = null;
  let startedAt: number | null = null;
  let bytes = 0;
  let queue: Promise<void> = Promise.resolve();
  let pcSeq = 0;
  const pcIds = new WeakMap<RTCPeerConnection, number>();

  const log = (event: string, detail?: Record<string, unknown>) => {
    events.push({ t: Date.now(), event, ...(detail ? { detail } : {}) });
  };

  function toB64(buf: Uint8Array): string {
    let bin = "";
    const step = 0x8000;
    for (let i = 0; i < buf.length; i += step) bin += String.fromCharCode(...buf.subarray(i, i + step));
    return btoa(bin);
  }

  function ensureMixer(): MediaStreamAudioDestinationNode {
    if (ctx && dest) return dest;
    ctx = new AudioContext({ sampleRate: 48000 });
    dest = ctx.createMediaStreamDestination();
    // A zero source keeps the destination producing samples between talkers,
    // so the recording's timeline stays continuous.
    const silence = ctx.createConstantSource();
    silence.offset.value = 0;
    silence.connect(dest);
    silence.start();
    void ctx.resume().catch(() => undefined);
    rec = new MediaRecorder(dest.stream, { mimeType: "audio/webm;codecs=opus" });
    rec.ondataavailable = (e: BlobEvent) => {
      if (!e.data || e.data.size === 0) return;
      // Chain so chunks keep their order even though arrayBuffer() is async.
      queue = queue.then(async () => {
        const buf = new Uint8Array(await e.data.arrayBuffer());
        bytes += buf.length;
        chunks.push(toB64(buf));
      });
    };
    rec.onerror = (e: Event) => log("peer_recorder_error", { error: String((e as unknown as { error?: unknown }).error ?? e.type) });
    rec.start(opts.timesliceMs);
    startedAt = Date.now();
    log("peer_recorder_started", { mimeType: rec.mimeType, sampleRate: ctx.sampleRate, ctxState: ctx.state });
    return dest;
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
    const d = ensureMixer();
    const stream = new MediaStream([track]);
    // The Chrome quirk: without a media element pulling the remote stream,
    // MediaStreamAudioSourceNode gets silence.
    const el = document.createElement("audio");
    el.muted = true;
    el.autoplay = true;
    el.srcObject = stream;
    void el.play().catch(() => undefined);
    let src: MediaStreamAudioSourceNode | null = null;
    try {
      src = (ctx as AudioContext).createMediaStreamSource(stream);
      src.connect(d);
    } catch (e) {
      log("peer_track_mix_error", { id: track.id, error: String(e) });
    }
    tracks.set(track.id, { track, el, src, ended: false });
    track.addEventListener("ended", () => markEnded(track.id, "ended_event"));
    track.addEventListener("mute", () => log("peer_track_muted", { id: track.id }));
    track.addEventListener("unmute", () => log("peer_track_unmuted", { id: track.id }));
    log("peer_track_added", { id: track.id, label: track.label, muted: track.muted, pc: pcIds.get(pc) ?? 0, via });
  }

  // Receivers of transceivers that negotiated a receiving direction. A
  // transceiver that exists only to send (e.g. our mic before an offer
  // reused it) has a receiver track that never carries audio: skip it.
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
    pc.addEventListener("connectionstatechange", () => {
      if (pc.connectionState === "closed" || pc.connectionState === "failed") {
        log("peer_connection_" + pc.connectionState, { pc: pcIds.get(pc) ?? 0 });
      }
    });
  }

  // The wrapper: a subclass keeps instanceof, statics (generateCertificate)
  // and the prototype chain intact.
  class GizziRTCPeerConnection extends Orig {
    constructor(...a: ConstructorParameters<typeof RTCPeerConnection>) {
      super(...a);
      watch(this);
    }
    setRemoteDescription(...a: unknown[]): Promise<void> {
      const p = (Orig.prototype.setRemoteDescription as (...x: unknown[]) => Promise<void>).apply(this, a);
      void Promise.resolve(p).then(
        () => scan(this, "receivers_after_srd"),
        () => undefined,
      );
      return p;
    }
    close(): void {
      Orig.prototype.close.call(this);
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
    // Ask the recorder for what it holds now and wait until it is queued.
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
    }),
  };
}

/**
 * Node side: pull the chunks and track events the page holds, append chunks to
 * <outDir>/<file> and hand each event to onEvent. With flush, the recorder is
 * asked for its partial chunk first (use when leaving). Returns bytes written.
 */
export async function drainPeerCapture(
  page: Page,
  outDir: string,
  onEvent: (e: PeerCaptureEvent) => void,
  opts: { flush?: boolean; file?: string } = {},
): Promise<number> {
  if (page.isClosed()) return 0;
  const got = await page
    .evaluate(async (flush) => {
      const cap = window.__gizziPeerCap;
      if (!cap) return { chunks: [] as string[], events: [] as PeerCaptureEvent[] };
      if (flush) await cap.flush();
      return cap.drain();
    }, Boolean(opts.flush))
    .catch(() => ({ chunks: [] as string[], events: [] as PeerCaptureEvent[] }));
  for (const e of got.events) onEvent(e);
  let n = 0;
  for (const b64 of got.chunks) {
    const buf = Buffer.from(b64, "base64");
    appendFileSync(join(outDir, opts.file ?? "call.webm"), buf);
    n += buf.length;
  }
  return n;
}
