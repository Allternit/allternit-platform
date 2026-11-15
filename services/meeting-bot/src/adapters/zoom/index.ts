// Zoom adapter (plan step 2.3, bot side).
//
// Zoom has required the Meeting SDK with an OBF (on-behalf-of) token or a ZAK for
// apps joining meetings hosted by other accounts since 2026-03-02; anonymous bots
// are refused (https://developers.zoom.us/docs/meeting-sdk/obf-faq/). So this
// adapter does not drive a browser. It runs the Meeting SDK for Linux in a native
// sidecar (./sidecar, C++) and talks to it over fd 3 with the framed protocol in
// ./protocol.ts. The OBF token belongs to the user who connected Zoom; Zoom lets
// the bot in only while that user is in the meeting and ends the bot's session
// when they leave (https://developers.zoom.us/docs/meeting-sdk/auth/obf/).
// While Zoom says that user is not in the meeting yet, join() retries the OBF
// join every 20 s within admitTimeoutMs and reports waiting_admit (CONTRACT.md
// "Change 2026-11-13: Zoom cloud side"); the runner still makes one join() call.
//
// Beyond PlatformAdapter it exposes the media path the runner needs, because the
// SDK hands audio to native code instead of a page:
//   onAudio(cb)        mixed call audio, s16le PCM (Zoom sends 32 kHz mono)
//   sendAudio(pcm, hz) PCM into the bot's virtual mic (spoken replies)
//   clearAudio()       drop queued mic audio (barge-in)
//   sendVideoFrame(f)  I420 frames for the bot's camera (the Bot Mode tile)
//   cameraOn/onCamera  whether Zoom has the camera started (sidecar onStartSend/onStopSend)
// The runner wires these in zoom-media.ts (tile) and livekit-node.ts (audio).

import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import type { Duplex } from "node:stream";
import type {
  ActiveSpeakerEvent,
  AdapterFactory,
  AdmitResult,
  BotIdentity,
  CaptionEvent,
  ChatMessage,
  EndReason,
  JoinFailure,
  JoinPhase,
  JoinRequest,
  JoinResult,
  Participant,
  PlatformAdapter,
  PodConfig,
  TileState,
  Unsubscribe,
} from "../../adapter.js";
import { resolveSdkJwt } from "./auth.js";
import {
  FrameDecoder,
  asSidecarEvent,
  encodeAudioIn,
  encodeJson,
  encodeVideoIn,
  type AudioFrame,
  type SidecarCommandInput,
  type SidecarEvent,
  type VideoFrame,
  type WireMeetingStatus,
  type WireParticipant,
} from "./protocol.js";
import { parseZoomJoinUrl } from "./url.js";

export { parseZoomJoinUrl, type ZoomJoinTarget } from "./url.js";
export { signSdkJwt, resolveSdkJwt } from "./auth.js";

export type LogLevel = "debug" | "info" | "warn" | "error";

export interface ZoomAdapterOptions {
  /** Sidecar binary. Default: env ZOOM_SIDECAR_BIN, else /opt/allternit/zoom/zoom-sidecar (the image path, see sidecar/README.md). */
  sidecarPath?: string;
  sidecarArgs?: string[];
  /** Extra env for the sidecar (merged over process.env). Never put tokens here: they travel over fd 3. */
  sidecarEnv?: Record<string, string | undefined>;
  /** Override how BotIdentity.zoom_obf becomes an SDK JWT. Default: resolveSdkJwt (jwt:/env: refs). */
  sdkJwt?: (identity: Extract<BotIdentity, { kind: "zoom_obf" }>) => Promise<string> | string;
  /**
   * Fetch short-lived tokens right before join. Zoom recommends fetching the OBF token
   * just before the meeting, and the local-recording join token lives 120 s, which is
   * shorter than a pod launch. Values returned here win over the identity's.
   */
  freshTokens?: () => Promise<{ obfToken?: string; appPrivilegeToken?: string }>;
  /** Static local-recording join token (else env ZOOM_APP_PRIVILEGE_TOKEN). */
  appPrivilegeToken?: string;
  /** Deliver the bot's own chat lines to onChat too (default false, per the interface). */
  includeSelfChat?: boolean;
  /** Active-speaker emit interval (default 250 ms, per the interface). */
  activeSpeakerIntervalMs?: number;
  /** Time for the sidecar to start and send `ready` (default 30 s, capped by prejoinTimeoutMs). */
  readyTimeoutMs?: number;
  /** Wait between OBF join attempts while Zoom says the authorising user is not in the meeting (default 20 s, CONTRACT.md "Zoom owner-present retry"). */
  ownerRetryMs?: number;
  /** Per-command reply timeout (default 10 s). */
  commandTimeoutMs?: number;
  /** Sidecar stdout/stderr lines and adapter notes. Default: console.error with a prefix. */
  log?: (level: LogLevel, msg: string) => void;
  /** Clock, for tests. */
  now?: () => number;
}

type EndedReason = Extract<EndReason, "removed" | "left_by_user" | "error">;
type FailReason = JoinFailure["reason"];

interface Pending {
  resolve: (r: { ok: boolean; error?: string; data?: unknown }) => void;
  timer: ReturnType<typeof setTimeout>;
}

const ACTIVE_STATES: ReadonlySet<WireMeetingStatus> = new Set(["connecting", "waiting_for_host", "in_waiting_room", "in_meeting", "reconnecting"]);

/** Map a join failure (status failed/ended before live) to a contract end reason and a plain detail. */
export function mapJoinFailure(ev: { status: WireMeetingStatus; code: number; name?: string }, wasWaiting: boolean): { reason: FailReason; detail: string } {
  const name = ev.name ?? "";
  const tag = name ? `${name} (${ev.code})` : `code ${ev.code}`;
  if (ev.status === "ended") {
    // Ended while in the waiting room or waiting for the host: the host declined or closed the lobby.
    if (wasWaiting) return { reason: "not_admitted", detail: `not admitted: ${tag}` };
    return { reason: "join_failed", detail: `meeting ended during join: ${tag}` };
  }
  // Owner-absent (AUTHORIZED_USER_NOT_INMEETING) never gets here: join() retries it (isOwnerAbsent).
  if (/ON_BEHALF|OBF|AUTHORIZED_USER/i.test(name)) {
    return { reason: "guest_blocked", detail: `Zoom refused the OBF token; reconnecting Zoom may fix it: ${tag}` };
  }
  if (/ANONYMOUS|RESTRICTED|BLOCKED|SIGN_IN|PRIVILEGE_TOKEN|EMAIL_NOT_MATCH/i.test(name)) {
    return { reason: "guest_blocked", detail: `Zoom refused the bot's identity: ${tag}` };
  }
  if (/PASSWORD/i.test(name)) return { reason: "join_failed", detail: `passcode wrong or missing from the link: ${tag}` };
  return { reason: "join_failed", detail: `join failed: ${tag}` };
}

/** Zoom's OBF refusal because the authorising user is not in the meeting yet (MEETING_FAIL_AUTHORIZED_USER_NOT_INMEETING). Retried. */
export function isOwnerAbsent(ev: { status: WireMeetingStatus; name?: string }): boolean {
  return ev.status === "failed" && /AUTHORIZED_USER_NOT_IN_?MEETING/i.test(ev.name ?? "");
}

/** Map a status that ends a live call to the onEnded reason. */
export function mapLiveEnd(ev: { status: WireMeetingStatus; code: number; name?: string }, leaving: boolean): EndedReason {
  if (leaving) return "left_by_user";
  if (ev.status === "failed") return "error";
  if (/NetworkBroken/i.test(ev.name ?? "")) return "error";
  // KickByHost, EndByHost, the OBF owner leaving, JBH timeout, no attendee: the call is over for the bot.
  return "removed";
}

export class ZoomAdapter implements PlatformAdapter {
  readonly platform = "zoom" as const;

  private child: ChildProcess | null = null;
  private ipc: Duplex | null = null;
  private decoder = new FrameDecoder();
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private exited = false;
  private exitWaiters: Array<() => void> = [];

  private status: WireMeetingStatus = "idle";
  private statusWaiters = new Set<(ev: Extract<SidecarEvent, { type: "status" }> | null) => void>();
  private everWaiting = false;
  private live = false;
  private leaving = false;
  private endedFired = false;
  private closed = false;

  private roster = new Map<string, WireParticipant>();
  private speakingIds: string[] = [];
  private speakerTimer: ReturnType<typeof setInterval> | null = null;

  private chatSubs = new Set<(m: ChatMessage) => void>();
  private partSubs = new Set<(all: Participant[]) => void>();
  private speakerSubs = new Set<(e: ActiveSpeakerEvent) => void>();
  private endedSubs = new Set<(r: EndedReason) => void>();
  private audioSubs = new Set<(a: AudioFrame & { t: number }) => void>();
  private cameraSubs = new Set<(on: boolean) => void>();
  private camera = false;
  private recordingState: string | null = null;

  constructor(private readonly opts: ZoomAdapterOptions = {}) {}

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  private log(level: LogLevel, msg: string): void {
    if (this.opts.log) this.opts.log(level, msg);
    else console.error(`[zoom ${level}] ${msg}`);
  }

  // ---- sidecar process ----------------------------------------------------------

  private startSidecar(): void {
    const bin = this.opts.sidecarPath ?? process.env.ZOOM_SIDECAR_BIN ?? "/opt/allternit/zoom/zoom-sidecar";
    const child = spawn(bin, this.opts.sidecarArgs ?? [], {
      stdio: ["ignore", "pipe", "pipe", "pipe"],
      env: { ...process.env, QT_LOGGING_RULES: "*.debug=false;*.warning=false", ZOOM_IPC_FD: "3", ...this.opts.sidecarEnv },
    });
    this.child = child;
    const ipc = child.stdio[3] as Duplex | null | undefined;
    if (!ipc) throw new Error("sidecar fd 3 missing");
    this.ipc = ipc;

    const pipeLines = (stream: NodeJS.ReadableStream | null, level: LogLevel) => {
      if (!stream) return;
      let rest = "";
      stream.setEncoding("utf8");
      stream.on("data", (s: string) => {
        rest += s;
        let i: number;
        while ((i = rest.indexOf("\n")) >= 0) {
          const line = rest.slice(0, i).trimEnd();
          rest = rest.slice(i + 1);
          if (line) this.log(level, `sidecar: ${line}`);
        }
        if (rest.length > 8192) rest = rest.slice(-8192);
      });
    };
    pipeLines(child.stdout, "debug");
    pipeLines(child.stderr, "info");

    ipc.on("data", (chunk: Buffer) => {
      let frames;
      try {
        frames = this.decoder.push(chunk);
      } catch (err) {
        this.log("error", `corrupt sidecar stream: ${(err as Error).message}; killing sidecar`);
        child.kill("SIGKILL");
        return;
      }
      for (const f of frames) {
        if (f.kind === "json") {
          const ev = asSidecarEvent(f.msg);
          if (ev) this.onEvent(ev);
        } else if (f.kind === "audio_out") {
          const t = this.now();
          for (const cb of this.audioSubs) cb({ ...f.audio, t });
        }
      }
    });
    ipc.on("error", (err: Error) => this.log("warn", `ipc error: ${err.message}`));

    child.on("error", (err) => {
      this.log("error", `sidecar spawn error: ${err.message}`);
      this.onExit();
    });
    child.on("exit", (code, signal) => {
      this.log(code === 0 ? "info" : "warn", `sidecar exited code=${code} signal=${signal ?? ""}`);
      this.onExit();
    });
  }

  private onExit(): void {
    if (this.exited) return;
    this.exited = true;
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.resolve({ ok: false, error: "sidecar exited" });
    }
    this.pending.clear();
    for (const w of this.statusWaiters) w(null);
    this.statusWaiters.clear();
    for (const w of this.exitWaiters) w();
    this.exitWaiters = [];
    if (this.live) this.fireEnded(this.leaving ? "left_by_user" : "error");
    this.live = false;
    this.stopSpeakerTimer();
    this.setCamera(false);
  }

  private setCamera(on: boolean): void {
    if (on === this.camera) return;
    this.camera = on;
    for (const cb of this.cameraSubs) cb(on);
  }

  private write(bytes: Uint8Array): boolean {
    if (!this.ipc || this.exited || this.ipc.destroyed) return false;
    this.ipc.write(bytes);
    return true;
  }

  private command(cmd: SidecarCommandInput, timeoutMs = this.opts.commandTimeoutMs ?? 10_000): Promise<{ ok: boolean; error?: string; data?: unknown }> {
    if (this.exited || !this.ipc) return Promise.resolve({ ok: false, error: "sidecar not running" });
    const id = this.nextId++;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve({ ok: false, error: "timeout" });
      }, timeoutMs);
      this.pending.set(id, { resolve, timer });
      this.write(encodeJson({ ...cmd, id }));
    });
  }

  private waitReady(timeoutMs: number): Promise<boolean> {
    return new Promise((resolve) => {
      if (this.exited) return resolve(false);
      const timer = setTimeout(() => done(false), timeoutMs);
      const done = (ok: boolean) => {
        clearTimeout(timer);
        this.readyWaiter = null;
        resolve(ok);
      };
      this.readyWaiter = done;
      this.exitWaiters.push(() => done(false));
    });
  }
  private readyWaiter: ((ok: boolean) => void) | null = null;

  /** Resolves with the first status event matching pred, null on sidecar exit, "timeout" after timeoutMs. */
  private waitStatus(
    pred: (s: WireMeetingStatus) => boolean,
    timeoutMs: number,
  ): Promise<Extract<SidecarEvent, { type: "status" }> | null | "timeout"> {
    return new Promise((resolve) => {
      if (this.exited) return resolve(null);
      const timer = setTimeout(() => {
        this.statusWaiters.delete(w);
        resolve("timeout");
      }, timeoutMs);
      const w = (ev: Extract<SidecarEvent, { type: "status" }> | null) => {
        if (ev && !pred(ev.status)) return;
        clearTimeout(timer);
        this.statusWaiters.delete(w);
        resolve(ev);
      };
      this.statusWaiters.add(w);
    });
  }

  // ---- events ---------------------------------------------------------------------

  private onEvent(ev: SidecarEvent): void {
    switch (ev.type) {
      case "ready":
        this.readyWaiter?.(true);
        break;
      case "result": {
        const p = this.pending.get(ev.id);
        if (p) {
          clearTimeout(p.timer);
          this.pending.delete(ev.id);
          p.resolve({ ok: ev.ok, error: ev.error, data: ev.data });
        }
        break;
      }
      case "auth":
        if (!ev.ok) {
          // Surface as a failed status so join() resolves through one path.
          this.onEvent({ type: "status", status: "failed", code: ev.code, name: `SDK_AUTH_${ev.name ?? ev.code}` });
        }
        break;
      case "status":
        this.onStatus(ev);
        break;
      case "chat":
        this.onChatEvent(ev);
        break;
      case "participants":
        this.setRoster(ev.list);
        break;
      case "active_speaker":
        this.speakingIds = ev.ids;
        break;
      case "recording":
        this.recordingState = ev.state;
        this.log(ev.state === "denied" || ev.state === "failed" || ev.state === "timeout" ? "warn" : "info", `raw recording ${ev.state}${ev.code !== undefined ? ` (${ev.code})` : ""}`);
        break;
      case "mic":
        this.log("debug", `virtual mic ${ev.state}`);
        break;
      case "log":
        // TileCamera::onStartSend / onStopSend (sidecar/src/bot.cpp) report as log lines.
        if (/^camera started\b/.test(ev.msg)) this.setCamera(true);
        else if (/^camera stopped\b/.test(ev.msg)) this.setCamera(false);
        this.log(ev.level, `sidecar: ${ev.msg}`);
        break;
    }
  }

  private onStatus(ev: Extract<SidecarEvent, { type: "status" }>): void {
    this.status = ev.status;
    if (ev.status === "in_waiting_room" || ev.status === "waiting_for_host") this.everWaiting = true;
    if (ev.status === "in_meeting") this.live = true;
    for (const w of [...this.statusWaiters]) w(ev);
    if (this.live && (ev.status === "ended" || ev.status === "failed" || ev.status === "idle")) {
      this.live = false;
      this.stopSpeakerTimer();
      this.fireEnded(mapLiveEnd(ev, this.leaving));
    }
    if (this.live) this.maybeStartSpeakerTimer();
  }

  private onChatEvent(ev: Extract<SidecarEvent, { type: "chat" }>): void {
    if (ev.self && !this.opts.includeSelfChat) return;
    const id = ev.msgId
      ? `zoom:${ev.msgId}`
      : `zoom:h:${createHash("sha256").update(`${ev.senderId}\u0000${ev.ts}\u0000${ev.text}`).digest("hex").slice(0, 24)}`;
    const m: ChatMessage = { id, sender: ev.sender, text: ev.text, t: this.now(), self: ev.self };
    for (const cb of this.chatSubs) cb(m);
  }

  private setRoster(list: WireParticipant[]): void {
    const next = new Map(list.map((p) => [p.id, p] as const));
    const same = next.size === this.roster.size && [...next].every(([id, p]) => this.roster.get(id)?.name === p.name);
    this.roster = next;
    if (same) return;
    const all = this.others();
    for (const cb of this.partSubs) cb(all);
  }

  private others(): Participant[] {
    return [...this.roster.values()].filter((p) => !p.self).map((p) => ({ id: p.id, name: p.name }));
  }

  private fireEnded(r: EndedReason): void {
    if (this.endedFired) return;
    this.endedFired = true;
    for (const cb of this.endedSubs) cb(r);
  }

  private maybeStartSpeakerTimer(): void {
    if (this.speakerTimer || !this.live || this.speakerSubs.size === 0) return;
    this.speakerTimer = setInterval(() => {
      const names = this.speakingIds
        .map((id) => this.roster.get(id))
        .filter((p): p is WireParticipant => !!p && !p.self)
        .map((p) => p.name);
      const e: ActiveSpeakerEvent = { t: this.now(), names };
      for (const cb of this.speakerSubs) cb(e);
    }, this.opts.activeSpeakerIntervalMs ?? 250);
  }

  private stopSpeakerTimer(): void {
    if (this.speakerTimer) clearInterval(this.speakerTimer);
    this.speakerTimer = null;
  }

  // ---- PlatformAdapter ------------------------------------------------------------

  async join(req: JoinRequest, onPhase?: (phase: JoinPhase) => void): Promise<JoinResult> {
    const fail = (reason: FailReason, detail: string): JoinResult => ({ ok: false, reason, detail });
    if (this.child) return fail("error", "join() already called on this adapter");
    if (req.identity.kind !== "zoom_obf") {
      return fail("join_failed", "Zoom needs a connected Zoom account (zoom_obf identity); anonymous joins are refused by Zoom");
    }
    const target = parseZoomJoinUrl(req.url);
    if (!target) return fail("join_failed", "not a Zoom join link (CONTRACT.md section 7)");

    let sdkJwt: string;
    let obfToken = req.identity.obfToken;
    let appPrivilegeToken = this.opts.appPrivilegeToken ?? process.env.ZOOM_APP_PRIVILEGE_TOKEN;
    try {
      sdkJwt = this.opts.sdkJwt ? await this.opts.sdkJwt(req.identity) : resolveSdkJwt(req.identity.sdkKey, req.identity.sdkSecretRef);
      if (this.opts.freshTokens) {
        const fresh = await this.opts.freshTokens();
        obfToken = fresh.obfToken ?? obfToken;
        appPrivilegeToken = fresh.appPrivilegeToken ?? appPrivilegeToken;
      }
    } catch (err) {
      return fail("join_failed", `Zoom credentials: ${(err as Error).message}`);
    }
    if (!obfToken) return fail("join_failed", "no OBF token; the user must connect Zoom first");

    const prejoinDeadline = this.now() + req.prejoinTimeoutMs;
    const left = () => Math.max(0, prejoinDeadline - this.now());
    try {
      this.startSidecar();
    } catch (err) {
      return fail("error", `could not start the Zoom sidecar: ${(err as Error).message}`);
    }
    onPhase?.("prejoin");
    if (!(await this.waitReady(Math.min(this.opts.readyTimeoutMs ?? 30_000, left())))) {
      await this.close();
      return fail("error", "Zoom sidecar did not start");
    }

    // One admit budget covers both waits: for the authorising user to join (OBF retry)
    // and for the host to admit from the waiting room.
    let admitDeadline: number | null = null;
    const enterWaiting = () => {
      if (admitDeadline !== null) return;
      admitDeadline = this.now() + req.admitTimeoutMs;
      onPhase?.("waiting_admit");
    };
    const admitLeft = () => (admitDeadline === null ? req.admitTimeoutMs : Math.max(0, admitDeadline - this.now()));
    const retryMs = this.opts.ownerRetryMs ?? 20_000;
    let lastFetch = this.now();

    for (let attempt = 1; ; attempt++) {
      if (attempt > 1 && this.opts.freshTokens && this.now() - lastFetch >= 100_000) {
        // The local-recording join token lives 120 s; refetch on later attempts.
        try {
          const fresh = await this.opts.freshTokens();
          obfToken = fresh.obfToken ?? obfToken;
          appPrivilegeToken = fresh.appPrivilegeToken ?? appPrivilegeToken;
          lastFetch = this.now();
        } catch (err) {
          this.log("warn", `token refresh before retry failed: ${(err as Error).message}`);
        }
      }
      // Subscribe before sending join so a fast status is not missed.
      const answerMs = attempt === 1 ? left() : Math.max(1000, Math.min(req.prejoinTimeoutMs, admitLeft()));
      const firstMove = this.waitStatus(
        (s) => s === "in_meeting" || s === "in_waiting_room" || s === "waiting_for_host" || s === "failed" || s === "ended",
        answerMs,
      );
      const res = await this.command({
        op: "join",
        sdkJwt,
        meetingNumber: target.meetingNumber,
        vanityId: target.vanityId,
        passcode: target.passcode,
        displayName: req.displayName,
        obfToken,
        appPrivilegeToken: appPrivilegeToken || undefined,
      });
      if (!res.ok) {
        await this.close();
        return fail("join_failed", `SDK refused the join call: ${res.error ?? "unknown"}`);
      }
      if (attempt === 1) onPhase?.("join_clicked");

      const ev = await firstMove;
      if (ev === "timeout") {
        await this.close();
        return fail("join_failed", "Zoom did not answer the join within the pre-join timeout");
      }
      if (ev === null) return fail("error", "Zoom sidecar exited during join");

      if (isOwnerAbsent(ev)) {
        // Zoom admits an OBF bot only while the user who authorised it is in the meeting.
        enterWaiting();
        const wait = Math.min(retryMs, admitLeft());
        if (wait <= 0 || !(await this.sleepUnlessExit(wait)) || admitLeft() <= 0) {
          await this.close();
          return fail("not_admitted", `the person who connected Zoom did not join the meeting within ${Math.round(req.admitTimeoutMs / 60_000)} min`);
        }
        this.log("info", `owner not in the meeting yet; retrying the OBF join (attempt ${attempt + 1})`);
        continue;
      }

      if (ev.status === "in_waiting_room" || ev.status === "waiting_for_host") {
        enterWaiting();
        const admit = await this.waitForAdmit(admitLeft());
        if (!admit.admitted) {
          await this.close();
          const reason = admit.reason ?? "not_admitted";
          return fail(reason, reason === "not_admitted" ? "not admitted (declined or waited too long)" : this.lastFailDetail ?? reason);
        }
      } else if (ev.status !== "in_meeting") {
        const m = mapJoinFailure(ev, this.everWaiting);
        await this.close();
        return fail(m.reason, m.detail);
      }
      break;
    }
    onPhase?.("live");
    this.maybeStartSpeakerTimer();
    return { ok: true, liveAt: this.now() };
  }
  private lastFailDetail: string | null = null;

  /** Resolves true after ms, false early if the sidecar exits. */
  private sleepUnlessExit(ms: number): Promise<boolean> {
    if (this.exited) return Promise.resolve(false);
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(true), ms);
      this.exitWaiters.push(() => {
        clearTimeout(timer);
        resolve(false);
      });
    });
  }

  async waitForAdmit(timeoutMs: number): Promise<AdmitResult> {
    if (this.status === "in_meeting") return { admitted: true };
    const ev = await this.waitStatus((s) => s === "in_meeting" || s === "failed" || s === "ended" || s === "idle", timeoutMs);
    if (ev === "timeout") return { admitted: false, reason: "not_admitted" };
    if (ev === null) return { admitted: false, reason: "error" };
    if (ev.status === "in_meeting") return { admitted: true };
    const m = mapJoinFailure(ev, true);
    this.lastFailDetail = m.detail;
    return { admitted: false, reason: m.reason };
  }

  async postChat(text: string): Promise<boolean> {
    if (!this.live || !text.trim()) return false;
    const r = await this.command({ op: "chat_send", text });
    if (!r.ok) this.log("warn", `chat send refused: ${r.error ?? "unknown"}`);
    return r.ok;
  }

  onChat(cb: (m: ChatMessage) => void): Unsubscribe {
    this.chatSubs.add(cb);
    return () => this.chatSubs.delete(cb);
  }

  async participants(): Promise<Participant[]> {
    if (this.live) {
      const r = await this.command({ op: "participants" });
      const list = (r.data as { list?: WireParticipant[] } | undefined)?.list;
      if (r.ok && Array.isArray(list)) this.setRoster(list);
    }
    return this.others();
  }

  onParticipants(cb: (all: Participant[]) => void): Unsubscribe {
    this.partSubs.add(cb);
    return () => this.partSubs.delete(cb);
  }

  onActiveSpeaker(cb: (e: ActiveSpeakerEvent) => void): Unsubscribe {
    this.speakerSubs.add(cb);
    this.maybeStartSpeakerTimer();
    return () => {
      this.speakerSubs.delete(cb);
      if (this.speakerSubs.size === 0) this.stopSpeakerTimer();
    };
  }

  /**
   * Zoom live captions are not wired: the Linux SDK's live-transcription controller
   * is not documented for Linux, and turning captions on is a host action. The STT
   * path (onAudio -> voice service) is the transcript; resolves false.
   */
  async captions(_cb: (e: CaptionEvent) => void): Promise<boolean> {
    return false;
  }

  async setTile(state: TileState): Promise<void> {
    if (this.exited || !this.child) return;
    await this.command({ op: "tile_state", state });
  }

  async leave(): Promise<void> {
    if (this.leaving) return;
    this.leaving = true;
    if (!this.child || this.exited || !ACTIVE_STATES.has(this.status)) return;
    const ended = this.waitStatus((s) => s === "ended" || s === "idle" || s === "failed", 5000);
    const r = await this.command({ op: "leave" }, 5000);
    if (r.ok) await ended;
    if (this.live) {
      this.live = false;
      this.stopSpeakerTimer();
      this.fireEnded("left_by_user");
    }
  }

  onEnded(cb: (reason: EndedReason) => void): Unsubscribe {
    this.endedSubs.add(cb);
    return () => this.endedSubs.delete(cb);
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    await this.leave();
    this.stopSpeakerTimer();
    const child = this.child;
    if (!child || this.exited) return;
    const gone = new Promise<void>((resolve) => this.exitWaiters.push(resolve));
    await this.command({ op: "shutdown" }, 2000);
    const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
    await gone;
    clearTimeout(timer);
  }

  // ---- media (Zoom-specific) ------------------------------------------------------

  /** Mixed call audio from the SDK (onMixedAudioRawDataReceived), s16le PCM. Starts once Zoom grants raw recording. */
  onAudio(cb: (a: AudioFrame & { t: number }) => void): Unsubscribe {
    this.audioSubs.add(cb);
    return () => this.audioSubs.delete(cb);
  }

  /** Queue s16le PCM for the bot's virtual mic. The sidecar paces it in real time. Returns false when not live. */
  sendAudio(pcm: Uint8Array, sampleRate: number, channels = 1): boolean {
    if (!this.live) return false;
    // Split into frames of at most 1 s so one reply never makes a huge frame.
    const step = sampleRate * 2 * channels;
    for (let off = 0; off < pcm.length; off += step) {
      if (!this.write(encodeAudioIn({ sampleRate, channels, pcm: pcm.subarray(off, Math.min(pcm.length, off + step)) }))) return false;
    }
    return true;
  }

  /** Drop mic audio the sidecar has queued but not yet sent (someone talked over Gizzi). */
  async clearAudio(): Promise<void> {
    if (this.live) await this.command({ op: "audio_clear" });
  }

  /** One I420 frame for the bot's camera. Returns false when not live. */
  sendVideoFrame(f: VideoFrame): boolean {
    return this.live ? this.write(encodeVideoIn(f)) : false;
  }

  /** True while Zoom has the bot's camera started; frames sent otherwise are dropped by the sidecar. */
  get cameraOn(): boolean {
    return this.camera;
  }

  /** Camera started (true) / stopped (false), as the sidecar reports it. */
  onCamera(cb: (on: boolean) => void): Unsubscribe {
    this.cameraSubs.add(cb);
    return () => this.cameraSubs.delete(cb);
  }

  /** Last raw-recording state the sidecar reported (granted / requested / denied / ...), or null. */
  get rawRecordingState(): string | null {
    return this.recordingState;
  }
}

export function createZoomAdapter(_cfg: Pick<PodConfig, "displayName">, opts: ZoomAdapterOptions = {}): ZoomAdapter {
  return new ZoomAdapter(opts);
}

export const zoomFactory: AdapterFactory = (cfg) => createZoomAdapter(cfg);
