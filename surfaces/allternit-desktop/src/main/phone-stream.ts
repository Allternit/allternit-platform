/**
 * Phone stream manager: one scrcpy session per phone serial, shared by every
 * consumer that asks for it (the Settings renderer today, a future relay
 * consumer — e.g. a bot watching along — attaches through the same
 * StreamConsumer interface, no IPC involved).
 *
 * Lifecycle: start() is explicit (renderer request only — a stream never
 * starts by itself against Eoj's phone). The session fans meta/chunks/end
 * out to subscribers; stop() is idempotent. A device that goes away mid-stream
 * is surfaced as `ended` (the device manager's reconnect loop handles
 * re-pairing; the stream can simply be started again).
 *
 * Audio (both lanes, best-effort): sessions may emit `audioMeta` (format)
 * and `audioChunk` (one encoded packet). The manager keeps the last
 * audioMeta per serial and replays it to a consumer that subscribes late
 * (the relay attaches after start()); chunks only flow while the stream is
 * live and has an audioMeta. A throwing audio consumer is isolated — audio
 * never ends or stalls the video fan-out.
 *
 * The session factory is injected so this manager is unit-tested with a fake
 * session and no adb/phone.
 */

import type { PhoneAudioMeta, PhoneInputEvent, PhoneStreamMeta, PhoneStreamOptions, PhoneStreamStatus, ScrcpyDeviceMessage } from './phone-scrcpy.js';

/** What the manager needs from a session (ScrcpySession satisfies this). */
export interface StreamSessionHandle {
  start(): Promise<void>;
  on(event: 'meta', cb: (meta: PhoneStreamMeta) => void): () => void;
  on(event: 'videoChunk', cb: (chunk: Buffer) => void): () => void;
  on(event: 'audioMeta', cb: (meta: PhoneAudioMeta) => void): () => void;
  on(event: 'audioChunk', cb: (chunk: Buffer) => void): () => void;
  on(event: 'deviceMessage', cb: (message: ScrcpyDeviceMessage) => void): () => void;
  on(event: 'ended', cb: (info: { reason: string }) => void): () => void;
  sendControl(events: PhoneInputEvent[]): { ok: boolean; message?: string };
  setDisplayPower(on: boolean): { ok: boolean; message?: string };
  close(reason?: string): void;
}

/** Anything that wants a phone's stream: the renderer IPC surface, a future relay, tests. */
export interface StreamConsumer {
  onMeta?(serial: string, meta: PhoneStreamMeta): void;
  onChunk?(serial: string, chunk: Buffer): void;
  /** Audio format; replayed on subscribe when the stream already has one. */
  onAudioMeta?(serial: string, meta: PhoneAudioMeta): void;
  /** One encoded audio packet (opus or AAC, per the last onAudioMeta). */
  onAudioChunk?(serial: string, chunk: Buffer): void;
  onDeviceMessage?(serial: string, message: ScrcpyDeviceMessage): void;
  onEnd?(serial: string, info: { reason: string }): void;
}

export interface PhoneStreamManagerDeps {
  createSession(serial: string, opts: PhoneStreamOptions): StreamSessionHandle;
}

interface StreamRecord {
  session: StreamSessionHandle | null;
  status: PhoneStreamStatus;
  consumers: Set<StreamConsumer>;
  /** True while a start() is in flight; stop() during this still cleans up. */
  starting: boolean;
  /** The current session's last audio format (null = no audio yet / none). */
  audioMeta: PhoneAudioMeta | null;
}

function safely(fn: () => void): void {
  try {
    fn();
  } catch {
    // An audio consumer failure stays with that consumer.
  }
}

export class PhoneStreamManager {
  private readonly streams = new Map<string, StreamRecord>();

  constructor(private readonly deps: PhoneStreamManagerDeps) {}

  private record(serial: string): StreamRecord {
    let rec = this.streams.get(serial);
    if (!rec) {
      rec = { session: null, status: { state: 'idle' }, consumers: new Set(), starting: false, audioMeta: null };
      this.streams.set(serial, rec);
    }
    return rec;
  }

  /** Idempotent per serial: a second start while live/starting just shares the session. */
  async start(serial: string, opts: PhoneStreamOptions = {}): Promise<{ ok: true } | { ok: false; message: string }> {
    const rec = this.record(serial);
    if (rec.status.state === 'live' || rec.status.state === 'starting') return { ok: true };
    rec.session?.close('replaced by a new stream');
    rec.status = { state: 'starting' };
    rec.starting = true;
    rec.audioMeta = null;

    const session = this.deps.createSession(serial, opts);
    rec.session = session;
    session.on('meta', (meta) => {
      rec.status = { state: 'live', meta };
      for (const consumer of rec.consumers) consumer.onMeta?.(serial, meta);
    });
    session.on('videoChunk', (chunk) => {
      if (rec.status.state !== 'live') return;
      for (const consumer of rec.consumers) consumer.onChunk?.(serial, chunk);
    });
    session.on('audioMeta', (meta) => {
      if (rec.session !== session) return;
      rec.audioMeta = meta;
      for (const consumer of rec.consumers) safely(() => consumer.onAudioMeta?.(serial, meta));
    });
    session.on('audioChunk', (chunk) => {
      if (rec.session !== session || rec.status.state !== 'live' || !rec.audioMeta) return;
      for (const consumer of rec.consumers) safely(() => consumer.onAudioChunk?.(serial, chunk));
    });
    session.on('deviceMessage', (message) => {
      for (const consumer of rec.consumers) consumer.onDeviceMessage?.(serial, message);
    });
    session.on('ended', (info) => {
      rec.starting = false;
      if (rec.session === session) rec.audioMeta = null;
      // A failed start keeps its `error` status: the start() return already
      // told the caller, and a late cleanup `ended` must not mask it.
      if (rec.status.state === 'error') return;
      if (rec.status.state === 'ended') return;
      rec.status = { state: 'ended', reason: info.reason };
      for (const consumer of rec.consumers) consumer.onEnd?.(serial, info);
    });

    try {
      await session.start();
      rec.starting = false;
      return { ok: true };
    } catch (error) {
      rec.starting = false;
      const message = error instanceof Error ? error.message : String(error);
      rec.status = { state: 'error', message };
      session.close('start failed');
      return { ok: false, message };
    }
  }

  /** Idempotent: no stream → resolves quietly. */
  async stop(serial: string): Promise<void> {
    const rec = this.streams.get(serial);
    if (!rec) return;
    rec.session?.close('stopped');
  }

  stopAll(): void {
    for (const rec of this.streams.values()) rec.session?.close('desktop is shutting down');
  }

  status(serial: string): PhoneStreamStatus {
    return this.streams.get(serial)?.status ?? { state: 'idle' };
  }

  /** Serials with a live or starting stream, for the owner to sync against device state. */
  activeSerials(): string[] {
    return [...this.streams.entries()]
      .filter(([, rec]) => rec.status.state === 'live' || rec.status.state === 'starting')
      .map(([serial]) => serial);
  }

  /** The live stream's current audio format, or null (no stream, no audio, or not reported yet). */
  audioMeta(serial: string): PhoneAudioMeta | null {
    const rec = this.streams.get(serial);
    return rec && (rec.status.state === 'live' || rec.status.state === 'starting') ? rec.audioMeta : null;
  }

  /** Attach a consumer; the returned function detaches. Detaching never stops the stream. */
  subscribe(serial: string, consumer: StreamConsumer): () => void {
    const rec = this.record(serial);
    rec.consumers.add(consumer);
    const audioMeta = this.audioMeta(serial);
    if (audioMeta) safely(() => consumer.onAudioMeta?.(serial, audioMeta));
    return () => rec.consumers.delete(consumer);
  }

  sendInput(serial: string, events: PhoneInputEvent[]): { ok: boolean; message?: string } {
    const rec = this.streams.get(serial);
    if (!rec?.session || rec.status.state !== 'live') return { ok: false, message: 'The stream is not live.' };
    return rec.session.sendControl(events);
  }

  setDisplayPower(serial: string, on: boolean): { ok: boolean; message?: string } {
    const rec = this.streams.get(serial);
    if (!rec?.session || rec.status.state !== 'live') return { ok: false, message: 'The stream is not live.' };
    return rec.session.setDisplayPower(on);
  }

  /** Called by the owner (phone-service) when the device manager reports the phone gone. */
  deviceDisconnected(serial: string): void {
    const rec = this.streams.get(serial);
    if (rec && (rec.status.state === 'live' || rec.status.state === 'starting')) {
      rec.session?.close('the phone disconnected');
    }
  }
}
