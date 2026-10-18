/**
 * Call bridge (CONTRACT-linked-phone-voice-2026-10-18 C5): a bot talks on a
 * real carrier call on its own phone.
 *
 * The phone's headset port is wired to a USB sound card on this Mac. The
 * Mac's input carries the caller, its output carries the bot into the phone's
 * mic. For every phone with "Bot answers calls" on, this watcher reads the
 * carrier line every second. On a ring it checks the cable, waits one ring
 * (3.5 s) and answers; then it asks cloud-api for a LiveKit room
 * (POST /api/v1/voice/linked-calls) and the hidden bridge window joins it as
 * the caller (phone-call-bridge-window.ts). The allternit-voice worker runs
 * the bot in that room exactly like a Telnyx SIP call.
 *
 *   idle ──ring──▶ ringing ──3.5 s, still ringing──▶ answering ──room joined──▶ live
 *     ▲               │ stops ringing / answered by hand        │ carrier idle → leave room
 *     │               ▼                                          │ room gone → end carrier call
 *     └──────────── idle ◀──────────── ending ◀─────────────────┘
 *   error: cable missing, no bot, cloud or join failed. Clears when the line is idle.
 *
 * Outbound: an approved phone.call.dial on an enabled phone arms the bridge;
 * when the line goes off-hook the bridge joins with direction "outbound".
 *
 * Android reads `dumpsys telephony.registry` and answers with KEYCODE_CALL
 * (platformSupport 'full'). iPhone reads the incoming-call screen through the
 * iCTRL helper's screen_text and taps Accept / End (platformSupport
 * 'best_effort': the helper can't read the screen while the iPhone is locked).
 *
 * Transport-agnostic: carrier lines, cloud, media window, clock and notices
 * are injected, so the state machine is tested with fakes and fake timers.
 */

import { EventEmitter } from 'node:events';
import { callBridgeMissing, type CallBridgeConfig } from './phone-call-bridge-config.js';
import type { ParsedTelephonyState, CommsResult } from './phone-user-comms.js';

// ── Shapes shared with cloud-api and the UI (contract C1, C3.1, C5) ────────

export type CallBridgePlatform = 'android' | 'ios';

export interface LinkedPhoneLine {
  kind: 'linked_phone';
  deviceId: string;
  serial?: string;
  label: string;
  platform: CallBridgePlatform;
}

export interface LinkedCallRequest {
  botId: string;
  line: LinkedPhoneLine;
  direction: 'inbound' | 'outbound';
  from: string | null;
  to: string | null;
}

export interface LinkedCallRoom {
  room: string;
  livekitUrl: string;
  token: string;
  identity: string;
}

export type CallBridgeState = 'idle' | 'ringing' | 'answering' | 'live' | 'ending' | 'error';
export type CallBridgePlatformSupport = 'full' | 'best_effort';

export interface CallBridgeStatus {
  serial: string;
  deviceId: string;
  label: string;
  platform: CallBridgePlatform;
  enabled: boolean;
  online: boolean;
  botId: string | null;
  state: CallBridgeState;
  room?: string;
  direction?: 'inbound' | 'outbound';
  /** The other party when the phone reveals it (E.164-ish), else absent. */
  remoteNumber?: string;
  error?: string;
  platformSupport: CallBridgePlatformSupport;
  /** One line on what the platform can and can't do. */
  platformNote: string;
}

export type CallBridgeEvent =
  | { type: 'status'; status: CallBridgeStatus }
  | { type: 'notice'; serial: string; title: string; message: string };

// ── Injected collaborators ──────────────────────────────────────────────────

export interface CallBridgePhone {
  /** The key the phone mirror uses: adb serial (Android) or `link:<deviceId>` (iPhone). */
  serial: string;
  /** Android: hardware serial. iPhone: udid. */
  deviceId: string;
  label: string;
  platform: CallBridgePlatform;
  online: boolean;
  config: CallBridgeConfig | null;
}

export interface Point {
  x: number;
  y: number;
}

/** One read of the carrier line. `null` from read() = could not read this time. */
export interface CarrierRead {
  state: 'idle' | 'ringing' | 'offhook' | 'unknown';
  number?: string;
}

export interface CarrierLine {
  read(phone: CallBridgePhone): Promise<CarrierRead | null>;
  /** Answer a ringing call. True once the line is off-hook (or the platform can't tell and the key went through). */
  answer(phone: CallBridgePhone): Promise<boolean>;
  end(phone: CallBridgePhone): Promise<boolean>;
}

export interface CallMediaJoin extends LinkedCallRoom {
  inputDeviceId: string | null;
  inputLabel: string | null;
  outputDeviceId: string | null;
  outputLabel: string | null;
}

/** The hidden bridge window (phone-call-bridge-window.ts). */
export interface CallMedia {
  /**
   * Are both sides of the cable present on this Mac right now? `problem`
   * names a cause other than a missing device (e.g. microphone access denied).
   */
  checkDevices(cfg: CallBridgeConfig): Promise<{ input: boolean; output: boolean; problem?: string }>;
  /** Join the room as the caller; rejects when the join fails. */
  join(serial: string, join: CallMediaJoin): Promise<void>;
  leave(serial: string): Promise<void>;
}

export interface CallBridgeDeps {
  phones: () => CallBridgePhone[];
  carrier: Record<CallBridgePlatform, CarrierLine>;
  cloud: { createRoom(req: LinkedCallRequest): Promise<LinkedCallRoom> };
  media: CallMedia;
  now?: () => number;
  log?: (message: string) => void;
}

// ── Tunables ────────────────────────────────────────────────────────────────

export const CALL_BRIDGE_POLL_MS = 1_000;
/** One ring, then answer. */
export const ONE_RING_MS = 3_500;
/** An approved dial arms the bridge for this long. */
export const OUTBOUND_ARM_MS = 60_000;
/** iPhone: the call screen must be gone this many reads in a row before the call counts as over. */
export const IOS_IDLE_READS = 3;
/** Ending: if the carrier call is still up after this, press End again (once). */
export const END_RETRY_MS = 8_000;

const SUPPORT: Record<CallBridgePlatform, { platformSupport: CallBridgePlatformSupport; platformNote: string }> = {
  android: {
    platformSupport: 'full',
    platformNote: 'Answers after one ring over adb and hangs up when the bot leaves.',
  },
  ios: {
    platformSupport: 'best_effort',
    platformNote: "Answers from the iPhone's incoming-call screen. It can't see a call while the iPhone is locked; keep it unlocked on the charger.",
  },
};

/** checkDevices problems about microphone access start with this (kept visible like a cable error). */
export const MIC_PROBLEM_PREFIX = 'Allow Allternit to use the microphone';

export function cableMissingMessage(label: string): string {
  return `Call audio cable not found for ${label}`;
}

interface Run {
  state: CallBridgeState;
  room?: string;
  direction?: 'inbound' | 'outbound';
  number?: string;
  error?: string;
  /** A call someone else handled (answered by hand, refused): leave it alone until the line is idle. */
  ignoreUntilIdle: boolean;
  answerTimer?: ReturnType<typeof setTimeout>;
  /** Bumped to cancel an answer/join flow in flight. */
  gen: number;
  idleReads: number;
  endingSince?: number;
  endRetried: boolean;
  noticeSent: boolean;
  polling: boolean;
}

// ── The watcher ─────────────────────────────────────────────────────────────

export class CallBridge extends EventEmitter {
  private readonly runs = new Map<string, Run>();
  private readonly outbound = new Map<string, { number: string; at: number }>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private readonly now: () => number;

  constructor(private readonly deps: CallBridgeDeps) {
    super();
    this.now = deps.now ?? Date.now;
  }

  start(): void {
    if (this.timer) return;
    this.timer = setInterval(() => void this.tick(), CALL_BRIDGE_POLL_MS);
    this.timer.unref?.();
  }

  /** Stop watching and leave every room (Desktop quitting). The carrier calls are left alone. */
  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    for (const [serial, run] of this.runs) {
      if (run.answerTimer) clearTimeout(run.answerTimer);
      run.gen += 1;
      if (run.room || run.state === 'live' || run.state === 'answering') void this.deps.media.leave(serial).catch(() => {});
    }
    this.runs.clear();
  }

  get running(): boolean {
    return this.timer !== null;
  }

  // ── queries ───────────────────────────────────────────────────────────────

  status(): CallBridgeStatus[] {
    return this.deps.phones().map((p) => this.statusOf(p));
  }

  statusFor(serial: string): CallBridgeStatus | null {
    const phone = this.deps.phones().find((p) => p.serial === serial || p.deviceId === serial);
    return phone ? this.statusOf(phone) : null;
  }

  private statusOf(phone: CallBridgePhone): CallBridgeStatus {
    const run = this.runs.get(phone.serial);
    const cfg = phone.config;
    const missing = cfg?.enabled ? callBridgeMissing(cfg) : null;
    const error = run?.error ?? missing ?? undefined;
    return {
      serial: phone.serial,
      deviceId: phone.deviceId,
      label: phone.label,
      platform: phone.platform,
      enabled: cfg?.enabled === true,
      online: phone.online,
      botId: cfg?.botId ?? null,
      state: run?.state ?? 'idle',
      ...(run?.room ? { room: run.room } : {}),
      ...(run?.direction && run.state !== 'idle' ? { direction: run.direction } : {}),
      ...(run?.number && run.state !== 'idle' ? { remoteNumber: run.number } : {}),
      ...(error ? { error } : {}),
      ...SUPPORT[phone.platform],
    };
  }

  // ── outbound ──────────────────────────────────────────────────────────────

  /**
   * An approved phone.call.dial just went out on `serial`. When the phone has
   * the bridge set up, arm it: the next off-hook joins the bot as outbound.
   * Returns true when the bot will be on the call.
   */
  noteOutboundDial(serial: string, number: string): boolean {
    const phone = this.deps.phones().find((p) => p.serial === serial);
    if (!phone || callBridgeMissing(phone.config) !== null) return false;
    this.outbound.set(serial, { number, at: this.now() });
    return true;
  }

  // ── cable check (settings sheet) ─────────────────────────────────────────

  /** Check the cable now; sets or clears the cable error on an idle phone. */
  async verifyCable(serial: string): Promise<CallBridgeStatus | null> {
    const phone = this.deps.phones().find((p) => p.serial === serial || p.deviceId === serial);
    if (!phone) return null;
    const run = this.run(phone.serial);
    if (phone.config?.enabled && callBridgeMissing(phone.config) === null && (run.state === 'idle' || run.state === 'error')) {
      const cable = await this.cablePresent(phone.config);
      const msg = cableMissingMessage(phone.label);
      if (!cable.ok) run.error = cable.problem ?? msg;
      else if (run.error === msg || run.error?.startsWith(MIC_PROBLEM_PREFIX)) run.error = undefined;
      this.emitStatus(phone);
    }
    return this.statusOf(phone);
  }

  // ── the media window told us the room is gone ─────────────────────────────

  /** The room disconnected (hangup from the UI deletes it, the bot left, network): end the carrier call. */
  mediaDisconnected(serial: string, reason: string): void {
    const phone = this.deps.phones().find((p) => p.serial === serial);
    const run = this.runs.get(serial);
    if (!phone || !run) return;
    if (run.state !== 'live' && run.state !== 'answering') return;
    this.deps.log?.(`[CallBridge] ${phone.label}: room disconnected (${reason}); ending the carrier call`);
    run.gen += 1;
    run.room = undefined;
    this.beginEnding(phone, run);
    void this.deps.carrier[phone.platform].end(phone).catch(() => false);
  }

  /** The media window itself went away (renderer crash): every live call loses its room. */
  mediaLost(reason: string): void {
    for (const [serial, run] of this.runs) {
      if (run.state === 'live' || run.state === 'answering') this.mediaDisconnected(serial, reason);
    }
  }

  // ── the loop ──────────────────────────────────────────────────────────────

  async tick(): Promise<void> {
    const phones = this.deps.phones();
    const seen = new Set<string>();
    await Promise.all(
      phones.map(async (phone) => {
        seen.add(phone.serial);
        const run = this.runs.get(phone.serial);
        const ready = phone.online && callBridgeMissing(phone.config) === null;
        if (!ready && (!run || run.state === 'idle' || run.state === 'error')) {
          if (run && run.state !== 'idle') this.setIdle(phone, run);
          return;
        }
        await this.poll(phone);
      }),
    );
    for (const [serial, run] of this.runs) {
      if (seen.has(serial)) continue;
      if (run.answerTimer) clearTimeout(run.answerTimer);
      run.gen += 1;
      if (run.room) void this.deps.media.leave(serial).catch(() => {});
      this.runs.delete(serial);
    }
  }

  private run(serial: string): Run {
    let run = this.runs.get(serial);
    if (!run) {
      run = { state: 'idle', ignoreUntilIdle: false, gen: 0, idleReads: 0, endRetried: false, noticeSent: false, polling: false };
      this.runs.set(serial, run);
    }
    return run;
  }

  private async poll(phone: CallBridgePhone): Promise<void> {
    const run = this.run(phone.serial);
    if (run.polling) return;
    run.polling = true;
    try {
      const read = await this.deps.carrier[phone.platform].read(phone).catch(() => null);
      if (!read) return;
      this.apply(phone, run, read);
    } finally {
      run.polling = false;
    }
  }

  /** Is the carrier line over? iPhone needs several screen reads in a row (the call screen can flicker). */
  private lineIdle(phone: CallBridgePhone, run: Run, read: CarrierRead): boolean {
    if (read.state !== 'idle') {
      run.idleReads = 0;
      return false;
    }
    run.idleReads += 1;
    return phone.platform === 'android' || run.idleReads >= IOS_IDLE_READS;
  }

  private apply(phone: CallBridgePhone, run: Run, read: CarrierRead): void {
    const idle = this.lineIdle(phone, run, read);
    switch (run.state) {
      case 'idle': {
        if (read.state === 'idle') {
          run.ignoreUntilIdle = false;
          run.noticeSent = false;
          return;
        }
        if (run.ignoreUntilIdle) return;
        if (read.state === 'ringing') {
          this.onRing(phone, run, read);
          return;
        }
        if (read.state === 'offhook') {
          const armed = this.outbound.get(phone.serial);
          this.outbound.delete(phone.serial);
          if (armed && this.now() - armed.at <= OUTBOUND_ARM_MS) {
            void this.bridge(phone, run, 'outbound', { from: null, to: armed.number });
          } else {
            run.ignoreUntilIdle = true; // a call taken by hand: not ours
          }
        }
        return;
      }
      case 'error':
        if (idle) {
          run.ignoreUntilIdle = false;
          run.noticeSent = false;
          this.setIdle(phone, run, true);
        }
        return;
      case 'ringing':
        if (read.state === 'ringing') {
          if (read.number && read.number !== run.number) {
            run.number = read.number;
            this.emitStatus(phone);
          }
          return;
        }
        // Stopped ringing before we answered: missed, or answered by hand.
        if (read.state === 'offhook') run.ignoreUntilIdle = true;
        if (read.state === 'idle' || read.state === 'offhook') this.setIdle(phone, run);
        return;
      case 'answering':
        if (idle) {
          // The call ended (caller hung up, callee never picked up) before the bot joined.
          run.gen += 1;
          this.leaveThenIdle(phone, run);
        }
        return;
      case 'live':
        if (idle) {
          this.deps.log?.(`[CallBridge] ${phone.label}: carrier call ended; leaving the room`);
          run.gen += 1;
          this.leaveThenIdle(phone, run);
        }
        return;
      case 'ending':
        if (idle) {
          this.setIdle(phone, run);
          return;
        }
        if (!run.endRetried && run.endingSince !== undefined && this.now() - run.endingSince >= END_RETRY_MS) {
          run.endRetried = true;
          void this.deps.carrier[phone.platform].end(phone).catch(() => false);
        }
        return;
    }
  }

  private onRing(phone: CallBridgePhone, run: Run, read: CarrierRead): void {
    const cfg = phone.config!;
    run.state = 'ringing';
    run.number = read.number;
    run.direction = 'inbound';
    run.error = undefined;
    const gen = ++run.gen;
    this.emitStatus(phone);
    this.deps.log?.(`[CallBridge] ${phone.label}: ringing; answering after one ring`);
    // Check the cable during the ring so a missing one never gets answered.
    const cable = this.cablePresent(cfg);
    run.answerTimer = setTimeout(() => {
      run.answerTimer = undefined;
      void cable.then((found) => {
        if (gen !== run.gen || run.state !== 'ringing') return;
        if (!found.ok) {
          this.refuseForCable(phone, run, found.problem);
          return;
        }
        void this.answer(phone, run, gen);
      });
    }, ONE_RING_MS);
  }

  private async cablePresent(cfg: CallBridgeConfig): Promise<{ ok: boolean; problem?: string }> {
    try {
      const found = await this.deps.media.checkDevices(cfg);
      return found.input && found.output && !found.problem ? { ok: true } : { ok: false, ...(found.problem ? { problem: found.problem } : {}) };
    } catch (error) {
      this.deps.log?.(`[CallBridge] device check failed: ${error instanceof Error ? error.message : String(error)}`);
      return { ok: false };
    }
  }

  /** Don't answer; raise the needs-you notice once for this ring. The phone keeps ringing for a person. */
  private refuseForCable(phone: CallBridgePhone, run: Run, problem?: string): void {
    const message = problem ?? cableMissingMessage(phone.label);
    run.state = 'error';
    run.error = message;
    run.ignoreUntilIdle = true;
    this.emitStatus(phone);
    if (!run.noticeSent) {
      run.noticeSent = true;
      this.emit('event', {
        type: 'notice',
        serial: phone.serial,
        title: message,
        message: problem
          ? `The bot didn't answer a call on ${phone.label}. ${problem}.`
          : `The bot didn't answer a call on ${phone.label}. Plug the call audio cable into this Mac, or check the input and output in Call audio.`,
      } satisfies CallBridgeEvent);
    }
  }

  private async answer(phone: CallBridgePhone, run: Run, gen: number): Promise<void> {
    const line = this.deps.carrier[phone.platform];
    // Still ringing? (The caller may have hung up during the ring.)
    const fresh = await line.read(phone).catch(() => null);
    if (gen !== run.gen) return;
    if (fresh && fresh.state !== 'ringing') {
      if (fresh.state === 'offhook') run.ignoreUntilIdle = true;
      this.setIdle(phone, run);
      return;
    }
    run.state = 'answering';
    this.emitStatus(phone);
    const answered = await line.answer(phone).catch(() => false);
    if (gen !== run.gen) return;
    if (!answered) {
      this.fail(phone, run, `Couldn't answer the call on ${phone.label}.`, false);
      return;
    }
    await this.bridge(phone, run, 'inbound', { from: run.number ?? null, to: null }, gen);
  }

  /** Room from cloud-api, then the window joins it. Any failure ends the carrier call (no dead air). */
  private async bridge(
    phone: CallBridgePhone,
    run: Run,
    direction: 'inbound' | 'outbound',
    parties: { from: string | null; to: string | null },
    gen = ++run.gen,
  ): Promise<void> {
    const cfg = phone.config!;
    const changed = run.state !== 'answering';
    run.state = 'answering';
    run.direction = direction;
    run.error = undefined;
    if (direction === 'outbound') run.number = parties.to ?? undefined;
    if (changed) this.emitStatus(phone);
    let room: LinkedCallRoom;
    try {
      room = await this.deps.cloud.createRoom({
        botId: cfg.botId!,
        line: lineFor(phone),
        direction,
        from: parties.from,
        to: parties.to,
      });
    } catch (error) {
      if (gen !== run.gen) return;
      this.fail(phone, run, `Couldn't start the bot on the call: ${errText(error)}`, true);
      return;
    }
    if (gen !== run.gen) return;
    run.room = room.room;
    try {
      await this.deps.media.join(phone.serial, {
        ...room,
        inputDeviceId: cfg.inputDeviceId,
        inputLabel: cfg.inputLabel,
        outputDeviceId: cfg.outputDeviceId,
        outputLabel: cfg.outputLabel,
      });
    } catch (error) {
      if (gen !== run.gen) return;
      void this.deps.media.leave(phone.serial).catch(() => {});
      run.room = undefined;
      this.fail(phone, run, `Couldn't connect the call audio: ${errText(error)}`, true);
      return;
    }
    if (gen !== run.gen) {
      // The call ended while we joined.
      void this.deps.media.leave(phone.serial).catch(() => {});
      return;
    }
    run.state = 'live';
    this.emitStatus(phone);
    this.deps.log?.(`[CallBridge] ${phone.label}: bot is on the call (${direction}, room ${room.room})`);
  }

  private fail(phone: CallBridgePhone, run: Run, message: string, endCarrier: boolean): void {
    this.deps.log?.(`[CallBridge] ${phone.label}: ${message}`);
    run.state = 'error';
    run.error = message;
    run.ignoreUntilIdle = true;
    run.room = undefined;
    this.emitStatus(phone);
    if (endCarrier) void this.deps.carrier[phone.platform].end(phone).catch(() => false);
  }

  private beginEnding(phone: CallBridgePhone, run: Run): void {
    run.state = 'ending';
    run.endingSince = this.now();
    run.endRetried = false;
    this.emitStatus(phone);
  }

  private leaveThenIdle(phone: CallBridgePhone, run: Run): void {
    this.beginEnding(phone, run);
    run.room = undefined;
    void this.deps.media
      .leave(phone.serial)
      .catch(() => {})
      .then(() => {
        if (run.state === 'ending') this.setIdle(phone, run);
      });
  }

  private setIdle(phone: CallBridgePhone, run: Run, clearError = false): void {
    if (run.answerTimer) clearTimeout(run.answerTimer);
    run.answerTimer = undefined;
    run.state = 'idle';
    run.room = undefined;
    run.number = undefined;
    run.endingSince = undefined;
    run.idleReads = 0;
    // A cable/answer error stays readable on the idle phone until the next call works.
    if (clearError && !run.error?.startsWith('Call audio cable') && !run.error?.startsWith(MIC_PROBLEM_PREFIX)) run.error = undefined;
    this.emitStatus(phone);
  }

  private emitStatus(phone: CallBridgePhone): void {
    this.emit('event', { type: 'status', status: this.statusOf(phone) } satisfies CallBridgeEvent);
  }
}

export function lineFor(phone: CallBridgePhone): LinkedPhoneLine {
  return { kind: 'linked_phone', deviceId: phone.deviceId, serial: phone.serial, label: phone.label, platform: phone.platform };
}

function errText(error: unknown): string {
  return (error instanceof Error ? error.message : String(error)).slice(0, 200);
}

// ── Android carrier line (adb) ──────────────────────────────────────────────

export interface AndroidCallLane {
  telephonyState(serial: string): Promise<CommsResult<ParsedTelephonyState>>;
  callAnswer(serial: string, key?: 'KEYCODE_CALL' | 'KEYCODE_HEADSETHOOK'): Promise<CommsResult<{}>>;
  callEnd(serial: string): Promise<CommsResult<{}>>;
}

/** How long a dialer takes to go off-hook after the answer key. */
export const ANSWER_SETTLE_MS = 1_200;

export function androidCarrierLine(
  lane: () => Promise<AndroidCallLane>,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): CarrierLine {
  const read = async (phone: CallBridgePhone): Promise<CarrierRead | null> => {
    const res = await (await lane()).telephonyState(phone.serial);
    if (!res.ok) return null;
    return res.number ? { state: res.state, number: res.number } : { state: res.state };
  };
  return {
    read,
    async answer(phone) {
      const comms = await lane();
      if (!(await comms.callAnswer(phone.serial, 'KEYCODE_CALL')).ok) return false;
      await sleep(ANSWER_SETTLE_MS);
      let after = await read(phone);
      // Some dialers ignore KEYCODE_CALL; with a headset plugged in, the headset button answers.
      if (after?.state === 'ringing') {
        await comms.callAnswer(phone.serial, 'KEYCODE_HEADSETHOOK');
        await sleep(ANSWER_SETTLE_MS);
        after = await read(phone);
      }
      return after === null ? true : after.state === 'offhook';
    },
    async end(phone) {
      return (await (await lane()).callEnd(phone.serial)).ok;
    },
  };
}

// ── iPhone carrier line (iCTRL screen_text + tap) ──────────────────────────

export interface IosCallScreen {
  state: 'idle' | 'ringing' | 'offhook' | 'unknown';
  accept?: Point;
  slide?: Point;
  end?: Point;
  number?: string;
}

interface ScreenItem {
  text: string;
  x: number;
  y: number;
}

function screenItems(value: unknown): ScreenItem[] | null {
  const v = (value ?? {}) as { items?: unknown };
  if (!Array.isArray(v.items)) return null;
  const out: ScreenItem[] = [];
  for (const raw of v.items) {
    const it = (raw ?? {}) as { label?: unknown; text?: unknown; center_x?: unknown; center_y?: unknown };
    const text = typeof it.label === 'string' ? it.label : typeof it.text === 'string' ? it.text : '';
    const x = Number(it.center_x);
    const y = Number(it.center_y);
    if (text.trim()) out.push({ text: text.trim(), x: Number.isFinite(x) ? x : -1, y: Number.isFinite(y) ? y : -1 });
  }
  return out;
}

const at = (item: ScreenItem | undefined): Point | undefined => (item && item.x >= 0 && item.y >= 0 ? { x: Math.round(item.x), y: Math.round(item.y) } : undefined);

/**
 * Heuristics for the iPhone call screens, from the helper's screen_text items:
 * - incoming (unlocked, full screen or banner): "Accept" + "Decline"
 *   ("Remind Me" / "Message" on the full screen); locked: "slide to answer";
 * - in a call: an "End call" button, or two or more of the in-call controls
 *   (mute, keypad, speaker/audio, add call, FaceTime, contacts, hold).
 * The caller shows as a number item when the contact is unknown.
 */
export function parseIosCallScreen(value: unknown): IosCallScreen {
  const items = screenItems(value);
  if (!items) return { state: 'unknown' };
  const find = (re: RegExp) => items.find((i) => re.test(i.text));
  const accept = find(/^(accept|answer)$/i);
  const slide = find(/slide to answer/i);
  const decline = find(/^decline$/i);
  const fullScreenExtras = find(/^(remind me|message)$/i);
  const end = find(/^end( call)?$/i);
  const controls = items.filter((i) => /^(mute|keypad|speaker|audio|add call|facetime|contacts|hold)$/i.test(i.text)).length;
  const numberItem = items.find((i) => /^\+?[\d\s().-]{7,20}$/.test(i.text) && (i.text.match(/\d/g)?.length ?? 0) >= 7);
  const number = numberItem ? (numberItem.text.trim().startsWith('+') ? '+' : '') + numberItem.text.replace(/\D/g, '') : undefined;
  const base = number ? { number } : {};
  if (accept || slide || (decline && fullScreenExtras)) {
    return { state: 'ringing', ...base, ...(at(accept) ? { accept: at(accept) } : {}), ...(at(slide) ? { slide: at(slide) } : {}) };
  }
  if (end || controls >= 2) return { state: 'offhook', ...base, ...(at(end) ? { end: at(end) } : {}) };
  return { state: 'idle' };
}

/** Calls the linked iPhone's helper (iCTRL JSON-RPC); rejects on any helper error (locked, unreachable). */
export type IosHelperCall = (phone: CallBridgePhone, method: string, params?: Record<string, unknown>) => Promise<unknown>;

export function iosCarrierLine(
  helper: IosHelperCall,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): CarrierLine {
  const screen = async (phone: CallBridgePhone): Promise<IosCallScreen | null> => {
    try {
      return parseIosCallScreen(await helper(phone, 'screen_text'));
    } catch {
      return null; // locked or unreachable: unknown this time
    }
  };
  return {
    async read(phone) {
      const s = await screen(phone);
      if (!s) return null;
      return s.number ? { state: s.state, number: s.number } : { state: s.state };
    },
    async answer(phone) {
      const s = await screen(phone);
      if (!s || s.state !== 'ringing') return false;
      if (s.accept) {
        await helper(phone, 'tap', { x: s.accept.x, y: s.accept.y });
      } else if (s.slide) {
        await helper(phone, 'drag', { from_x: Math.max(20, s.slide.x - 130), from_y: s.slide.y, to_x: s.slide.x + 150, to_y: s.slide.y, duration: 0.3 });
      } else {
        return false;
      }
      await sleep(ANSWER_SETTLE_MS);
      const after = await screen(phone);
      return after === null ? true : after.state === 'offhook';
    },
    async end(phone) {
      const s = await screen(phone);
      if (!s?.end) return false;
      await helper(phone, 'tap', { x: s.end.x, y: s.end.y });
      return true;
    },
  };
}

// ── cloud-api: POST /api/v1/voice/linked-calls (contract C3.1) ─────────────

export interface LinkedCallAuth {
  baseUrl: () => string;
  /** The paired runtime's device token (preferred: the route maps it to runtime + owner). */
  deviceToken: () => string | null;
  /** Fallback: the signed-in person's Clerk session token. */
  clerkToken: () => Promise<string | null>;
  fetch: typeof fetch;
}

export const LINKED_CALL_TIMEOUT_MS = 10_000;

export async function requestLinkedCallRoom(auth: LinkedCallAuth, req: LinkedCallRequest): Promise<LinkedCallRoom> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const device = auth.deviceToken();
  if (device) {
    headers.authorization = `Bearer ${device}`;
    headers['x-allternit-desktop-access-token'] = device;
  } else {
    const clerk = await auth.clerkToken();
    if (!clerk) throw new Error('Sign in to Allternit Desktop so the bot can take calls');
    headers.authorization = `Bearer ${clerk}`;
  }
  const res = await auth.fetch(`${auth.baseUrl().replace(/\/$/, '')}/api/v1/voice/linked-calls`, {
    method: 'POST',
    headers,
    body: JSON.stringify(req),
    signal: AbortSignal.timeout(LINKED_CALL_TIMEOUT_MS),
  });
  const body = (await res.json().catch(() => ({}))) as Partial<LinkedCallRoom> & { error?: unknown; message?: unknown };
  if (!res.ok) {
    const why = typeof body.message === 'string' ? body.message : typeof body.error === 'string' ? body.error : `cloud-api answered ${res.status}`;
    throw new Error(why);
  }
  if (typeof body.room !== 'string' || typeof body.livekitUrl !== 'string' || typeof body.token !== 'string' || typeof body.identity !== 'string') {
    throw new Error('cloud-api returned an incomplete call room');
  }
  return { room: body.room, livekitUrl: body.livekitUrl, token: body.token, identity: body.identity };
}
