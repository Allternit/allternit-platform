// Allternit Meetings pod entrypoint (lane 2.1). One process = one meeting session.
//
//   node dist/runner.js            (config from the environment, see podConfigFromEnv)
//
// Lifecycle (CONTRACT section 3): launching -> joining -> waiting_admit -> live
// <-> paused -> ended, each move reported to cloud-api in an ingest batch with
// the pod token. On admit: the consent notice + meeting page link in chat
// (again for people who join later). Leave rules: removed, empty room 5 min,
// 8 h max, not admitted in 10 min, another vendor's notetaker joins, "stop" in
// chat, or a `leave` control.
//
// Media (browser platforms): media.ts installs the camera tile + mic override
// and the call capture before the meeting page loads; once live, livekit.ts
// publishes the call mix to room meeting-<sid> and plays the worker's `tts`
// track into the mic. STT segments come back on the LiveKit data channel; the
// runner labels the speaker from the adapter's active-speaker timeline,
// assigns seq and ingests them (cloud's ingest is pod-only).

import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import type {
  EndReason,
  JoinPhase,
  Participant,
  Platform,
  PlatformAdapter,
  PodConfig,
  TileState,
} from "./adapter.js";
import { createMeetAdapter } from "./adapters/meet.js";
import { CloudClient, ControlStream, Ingest, SpeakerTimeline, type ControlMessage, type SessionState } from "./ingest.js";
import { LiveKitBridge, type SttSegment, type WakeTurn } from "./livekit.js";
import { TurnClient, type TurnMeeting, type TurnSegment } from "./turns.js";
import { PodMedia } from "./media.js";
import type { Browser, BrowserContext, Page } from "playwright-core";

// ---- config --------------------------------------------------------------------

export const DEFAULT_LIMITS: PodConfig["limits"] = {
  admitTimeoutMs: 10 * 60_000,
  prejoinTimeoutMs: 90_000,
  emptyRoomMs: 5 * 60_000,
  maxDurationMs: 8 * 60 * 60_000,
  screenNoteMinIntervalMs: 30_000,
};

/**
 * PodConfig from the environment the launcher sets:
 *   ALLTERNIT_POD_CONFIG       PodConfig as JSON, or base64 of that JSON
 *   ALLTERNIT_POD_CONFIG_FILE  or a path to the JSON (a mounted secret)
 *   ALLTERNIT_POD_TOKEN        the pod token (overrides podToken; keeps it out of the JSON)
 *   ALLTERNIT_LIVEKIT_TOKEN    the LiveKit participant token (overrides livekit.token)
 * Missing limits take the contract defaults.
 */
export function podConfigFromEnv(env: NodeJS.ProcessEnv = process.env): PodConfig {
  let raw = env.ALLTERNIT_POD_CONFIG ?? "";
  if (!raw && env.ALLTERNIT_POD_CONFIG_FILE) raw = readFileSync(env.ALLTERNIT_POD_CONFIG_FILE, "utf8");
  if (!raw) throw new Error("ALLTERNIT_POD_CONFIG or ALLTERNIT_POD_CONFIG_FILE is required");
  const text = raw.trim().startsWith("{") ? raw : Buffer.from(raw, "base64").toString("utf8");
  const cfg = JSON.parse(text) as PodConfig;
  if (env.ALLTERNIT_POD_TOKEN) cfg.podToken = env.ALLTERNIT_POD_TOKEN;
  if (env.ALLTERNIT_LIVEKIT_TOKEN) cfg.livekit = { ...cfg.livekit, token: env.ALLTERNIT_LIVEKIT_TOKEN };
  cfg.limits = { ...DEFAULT_LIMITS, ...(cfg.limits ?? {}) };
  cfg.livekit ??= { url: "", room: `meeting-${cfg.sessionId}`, token: "" };
  cfg.livekit.room ||= `meeting-${cfg.sessionId}`;
  const missing = (["sessionId", "meetingId", "platform", "joinUrl", "displayName", "identity", "cloudApiUrl", "podToken"] as const).filter((k) => !cfg[k]);
  if (missing.length) throw new Error(`pod config is missing: ${missing.join(", ")}`);
  if (!["meet", "zoom", "teams"].includes(cfg.platform)) throw new Error(`unknown platform ${cfg.platform}`);
  return cfg;
}

/**
 * Optional fields the launcher may add to the pod config JSON beside PodConfig
 * (adapter.ts stays frozen): the meeting title and owner for the turn prompt.
 */
export interface PodConfigExtras {
  meetingTitle?: string;
  ownerName?: string;
}

/**
 * Where the in-pod allternit-api serves the turn routes, and its service token.
 *   ALLTERNIT_MEETING_TURN_URL        default http://127.0.0.1:8013 (the pinned production port)
 *   ALLTERNIT_INTERNAL_SERVICE_TOKEN  sent as x-allternit-internal-token
 * The launcher's ALLTERNIT_MEETING_CLOUD_API_URL / ALLTERNIT_MEETING_POD_TOKEN go
 * to that allternit-api, not to the runner (image: meeting-pod.env is loaded by
 * both units).
 */
export function turnTargetFromEnv(env: NodeJS.ProcessEnv = process.env): { url: string; token?: string } {
  return { url: env.ALLTERNIT_MEETING_TURN_URL || "http://127.0.0.1:8013", token: env.ALLTERNIT_INTERNAL_SERVICE_TOKEN || undefined };
}

export const PLATFORM_LABEL: Record<Platform, string> = { meet: "Google Meet", zoom: "Zoom", teams: "Microsoft Teams" };

/** "Gizzi, what did we decide?" / "@Gizzi can you ..." / "... thanks Gizzi?" */
export const GIZZI_MENTION = /\bgizzi\b/i;
export function mentionQuestion(text: string): string {
  return text.replace(/^\s*@?gizzi[\s,:!-]*/i, "").trim() || text.trim();
}

export function meetingPageUrl(cfg: Pick<PodConfig, "tileUrl" | "meetingId">): string {
  let origin = "https://ai.allternit.com";
  try {
    if (cfg.tileUrl) origin = new URL(cfg.tileUrl).origin;
  } catch {
    /* keep prod */
  }
  return `${origin}/meetings/${cfg.meetingId}`;
}

// ---- leave-rule matchers ---------------------------------------------------------------

/**
 * Display names of other vendors' notetaker bots. Kept strict on purpose: a
 * person named Jamie or Grace must never make Gizzi leave.
 */
export const OTHER_NOTETAKER =
  /\b(otter\.ai|otter (assistant|pilot|notetaker)|fireflies(\.ai)?|fred from fireflies|read\.ai|tl;?dv|fathom|meetgeek|avoma|sembly|supernormal|airgram|circleback|notta|tactiq|bluedot|recall\.ai|note ?taker|notetaking bot|meeting bot)\b/i;

export function isOtherNotetaker(name: string, ownName: string): boolean {
  const n = name.trim();
  if (!n || n === ownName || /\ballternit\b|\bgizzi\b/i.test(n)) return false;
  return OTHER_NOTETAKER.test(n);
}

/** "stop", "Gizzi, stop", "@Gizzi leave", "please leave", "stop recording" ... */
export const STOP_COMMAND = /^\s*(@?gizzi[\s,:!-]*)?(please\s+)?(stop|leave|stop (recording|transcribing|taking notes|listening)|leave (the|this) (call|meeting))(\s+please)?\s*[.!]*\s*$/i;

// ---- adapters -------------------------------------------------------------------------

/** What a browser adapter gets from the runner (same shape the Teams adapter takes). */
export interface BrowserAdapterHooks {
  installMedia?: (ctx: BrowserContext, browser: Browser, page: Page) => Promise<void>;
  setTile?: (state: TileState) => Promise<void>;
  headless?: boolean;
  chromePath?: string;
  screenshotDir?: string;
  onEvent?: (kind: string, detail?: unknown) => void;
}

export type RunnerAdapterFactory = (cfg: PodConfig, hooks: BrowserAdapterHooks) => PlatformAdapter | Promise<PlatformAdapter>;

/** Platforms whose adapter drives a Chrome page (media.ts applies). Zoom runs the SDK sidecar. */
export const BROWSER_PLATFORMS: readonly Platform[] = ["meet", "teams"];

async function importAdapter(spec: string): Promise<Record<string, unknown>> {
  // A variable specifier: other lanes own these files and build them in parallel.
  return (await import(spec)) as Record<string, unknown>;
}

export const defaultAdapterFactory: RunnerAdapterFactory = async (cfg, hooks) => {
  switch (cfg.platform) {
    case "meet":
      return createMeetAdapter(cfg, hooks);
    case "teams": {
      const mod = await importAdapter("./adapters/teams.js");
      const create = mod.createTeamsAdapter as (c: PodConfig, o: BrowserAdapterHooks) => PlatformAdapter;
      return create(cfg, hooks);
    }
    case "zoom": {
      const mod = await importAdapter("./adapters/zoom/index.js");
      const create = (mod.createZoomAdapter ?? mod.zoomFactory) as (c: PodConfig, o?: unknown) => PlatformAdapter;
      return create(cfg, { onEvent: hooks.onEvent });
    }
  }
};

// ---- the run --------------------------------------------------------------------------

export interface RunOptions {
  adapterFactory?: RunnerAdapterFactory;
  headless?: boolean;
  chromePath?: string;
  /** Debug: also record the call mix to this .webm file. */
  recordFile?: string;
  screenshotDir?: string;
  /** One JSON line per event; default stdout. */
  logLine?: (line: string) => void;
  /** In-pod allternit-api turn routes; default from turnTargetFromEnv(). Null turns turns off. */
  turns?: { url: string; token?: string } | null;
}

export interface RunResult {
  endReason: EndReason;
  detail?: string;
  exitCode: number;
  stats: Ingest["stats"];
}

const TILE_FOR: Partial<Record<SessionState, TileState>> = {
  launching: "joining",
  joining: "joining",
  waiting_admit: "joining",
  live: "listening",
  paused: "paused",
};

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export async function runPod(cfg: PodConfig, opts: RunOptions = {}): Promise<RunResult> {
  const write = opts.logLine ?? ((l: string) => process.stdout.write(l + "\n"));
  const log = (kind: string, detail?: unknown) =>
    write(JSON.stringify({ t: new Date().toISOString(), ev: kind, ...(detail === undefined ? {} : { detail }) }));
  log("pod_start", { session: cfg.sessionId, platform: cfg.platform, room: cfg.livekit.room, limits: cfg.limits });

  const cloud = new CloudClient(cfg.cloudApiUrl, cfg.sessionId, cfg.podToken, log);
  const ingest = new Ingest(cloud, log);
  ingest.start();
  const timeline = new SpeakerTimeline();
  const browser = BROWSER_PLATFORMS.includes(cfg.platform);
  const media = browser
    ? new PodMedia({ displayName: cfg.displayName, tileUrl: cfg.tileUrl, recordFile: opts.recordFile, onEvent: log })
    : null;
  const pageUrl = meetingPageUrl(cfg);
  const extras = cfg as PodConfig & PodConfigExtras;
  const turnTarget = opts.turns === undefined ? turnTargetFromEnv() : opts.turns;
  const turnClient = turnTarget ? new TurnClient(turnTarget.url, turnTarget.token, log) : null;
  /** Labelled transcript kept in the pod: the turn window and the end summary. */
  const segments: TurnSegment[] = [];
  const pendingTurns: Array<Parameters<typeof runTurn>[0]> = [];
  let turnBusy = false;

  // `as` keeps TS from narrowing these at declaration (they change inside closures).
  let state = "" as SessionState | ""; // nothing reported yet, so "launching" goes out first
  let liveAt = 0;
  let adapter = null as PlatformAdapter | null;
  let lk = null as LiveKitBridge | null;
  let ending = false;
  let wantPaused = false;
  let lastSttAt = 0;
  /** Stop-listening windows (epoch ms); `to` = 0 while still paused. */
  const pauses: Array<{ from: number; to: number }> = [];
  let emptySince: number | null = null;
  let people: Participant[] = [];
  const noticed = new Set<string>();
  let lastNoticeAt = 0;
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  const timers = new Set<ReturnType<typeof setInterval>>();
  const unsubs: Array<() => void> = [];
  let resolveEnd!: (r: { reason: EndReason; detail?: string }) => void;
  const ended = new Promise<{ reason: EndReason; detail?: string }>((r) => (resolveEnd = r));

  async function setState(next: SessionState, reason: EndReason | null = null, detail?: string): Promise<void> {
    if (next === state && next !== "ended") return;
    state = next;
    log("state", { state: next, end_reason: reason, detail });
    const tile = TILE_FOR[next];
    if (tile) void media?.setTile(tile);
    await ingest.setState(next, reason, detail);
  }

  function end(reason: EndReason, detail?: string): void {
    if (ending) return;
    ending = true;
    log("leave_rule", { reason, detail });
    resolveEnd({ reason, detail });
  }

  function every(ms: number, fn: () => Promise<void> | void): void {
    let busy = false;
    const t = setInterval(() => {
      if (busy || ending) return;
      busy = true;
      void Promise.resolve(fn())
        .catch((e: unknown) => log("tick_error", String(e).slice(0, 200)))
        .finally(() => (busy = false));
    }, ms);
    timers.add(t);
  }

  // ---- chat + notice -------------------------------------------------------------------
  async function say(text: string, key?: string): Promise<boolean> {
    if (!adapter) return false;
    const ok = await adapter.postChat(text).catch(() => false);
    if (ok) ingest.addChat({ idempotency_key: key ?? `out:${cfg.sessionId}:${Date.now()}:${text.length}`, direction: "out", sender: cfg.displayName, text, at: Date.now(), visibility: "chat" });
    log(ok ? "chat_posted" : "chat_post_failed", { chars: text.length });
    return ok;
  }

  const noticeText = cfg.noticeText.includes(pageUrl) ? cfg.noticeText : `${cfg.noticeText} Meeting page: ${pageUrl}`;
  async function postNotice(why: string): Promise<void> {
    lastNoticeAt = Date.now();
    if (await say(noticeText)) {
      ingest.setNoticeAt(Date.now());
      for (const p of people) noticed.add(p.id);
      log("notice_posted", { why, present: people.length });
    }
  }
  // Late joiners did not see the first notice (Meet shows chat from join time on).
  function scheduleNotice(): void {
    if (noticeTimer || ending) return;
    const wait = Math.max(15_000, 60_000 - (Date.now() - lastNoticeAt));
    noticeTimer = setTimeout(() => {
      noticeTimer = null;
      if (!ending && people.some((p) => !noticed.has(p.id))) void postNotice("late_joiner");
    }, wait);
  }

  // ---- control stream ----------------------------------------------------------------------
  async function applyPause(on: boolean): Promise<void> {
    await media?.setCapturePaused(on);
    await lk?.setMuted(on);
    const open = pauses.at(-1);
    if (on && !(open && open.to === 0)) pauses.push({ from: Date.now(), to: 0 });
    if (!on && open && open.to === 0) open.to = Date.now();
    if (state === "live" || state === "paused") await setState(on ? "paused" : "live");
  }

  function onControl(m: ControlMessage): void {
    log("control", m.type === "speak" ? { type: m.type, chars: m.text.length } : m);
    switch (m.type) {
      case "leave":
        end("left_by_user", "control_leave");
        break;
      case "stop_listening":
        wantPaused = true;
        void applyPause(true);
        break;
      case "resume":
        wantPaused = false;
        void applyPause(false);
        break;
      case "speak": {
        // The owner's "say this" from the meeting page: chat unless guest-quiet,
        // and spoken through the worker's TTS (cloud only sends it with speak_replies on).
        if (state !== "live") break;
        const id = `speak-${randomUUID()}`;
        if (!cfg.guestQuiet) void say(m.text, `speak:${id}`);
        if (cfg.speakReplies) void lk?.speak(id, m.text).then((ok) => log(ok ? "speak_sent" : "speak_not_sent", { turn_id: id }));
        break;
      }
      case "set_speak_replies":
        cfg.speakReplies = m.on;
        break;
      case "set_guest_quiet":
        cfg.guestQuiet = m.on;
        break;
      case "approval":
        break; // consumed by the in-pod agent session through CloudClient.toolCall polling
    }
  }
  const control = new ControlStream(cloud, onControl, log);
  control.start();

  // ---- signals ------------------------------------------------------------------------------
  const onSignal = (sig: string) => () => end("error", `pod_terminated_${sig}`);
  const sigterm = onSignal("sigterm");
  const sigint = onSignal("sigint");
  process.once("SIGTERM", sigterm);
  process.once("SIGINT", sigint);

  // ---- join -------------------------------------------------------------------------------------
  void setState("launching");
  const run = (async () => {
    const hooks: BrowserAdapterHooks = {
      installMedia: media ? (c, b, p) => media.install(c, b, p) : undefined,
      setTile: media ? (s) => media.setTile(s) : undefined,
      headless: opts.headless,
      chromePath: opts.chromePath,
      screenshotDir: opts.screenshotDir,
      onEvent: log,
    };
    adapter = await (opts.adapterFactory ?? defaultAdapterFactory)(cfg, hooks);
    if (ending) return;
    unsubs.push(adapter.onEnded((r) => end(r, "adapter")));

    // Guard on top of the adapter's own timeouts: never sit in the lobby forever.
    const joinDeadline = setTimeout(() => end("not_admitted", "join_guard"), cfg.limits.prejoinTimeoutMs + cfg.limits.admitTimeoutMs + 60_000);
    const result = await adapter.join(
      {
        url: cfg.joinUrl,
        displayName: cfg.displayName,
        identity: cfg.identity,
        admitTimeoutMs: cfg.limits.admitTimeoutMs,
        prejoinTimeoutMs: cfg.limits.prejoinTimeoutMs,
      },
      (phase: JoinPhase) => {
        if (ending) return;
        if (phase === "prejoin" || phase === "join_clicked") void setState("joining");
        else if (phase === "waiting_admit") void setState("waiting_admit");
      },
    );
    clearTimeout(joinDeadline);
    if (ending) return;
    if (!result.ok) {
      end(result.reason, [result.detail, result.screenshot].filter(Boolean).join(" ") || undefined);
      return;
    }
    liveAt = result.liveAt;
    await goLive(result.detail);
  })().catch((e: unknown) => end("error", String(e).slice(0, 300)));

  async function goLive(detail?: string): Promise<void> {
    if (!adapter) return;
    const a = adapter;
    await setState("live", null, detail);
    await postNotice("admitted");

    unsubs.push(
      a.onChat((m) => {
        if (state === "paused") return; // stop listening covers chat too
        ingest.addChat({ idempotency_key: `${cfg.platform}:${m.id}`, direction: "in", sender: m.sender, text: m.text, at: m.t });
        if (STOP_COMMAND.test(m.text)) {
          log("chat_stop", { sender: m.sender });
          end("left_by_user", "chat_stop");
        } else if (GIZZI_MENTION.test(m.text)) {
          queueTurn({ kind: "chat_mention", text: mentionQuestion(m.text), asker: m.sender, confidence: 1 });
        }
      }),
    );
    unsubs.push(
      a.onParticipants((all) => {
        people = all;
        ingest.setParticipants(all);
        log("participants", { count: all.length });
        emptySince = all.length === 0 ? (emptySince ?? Date.now()) : null;
        const other = all.find((p) => isOtherNotetaker(p.name, cfg.displayName));
        if (other) end("other_notetaker", other.name);
        else if (all.some((p) => !noticed.has(p.id))) scheduleNotice();
      }),
    );
    unsubs.push(a.onActiveSpeaker((e) => timeline.add(e)));
    void a
      .captions((c) => {
        if (!c.final) return;
        timeline.addCaption(c.t, c.speaker);
        // Fallback transcript: no STT from the worker for a minute.
        if (state === "live" && Date.now() - lastSttAt > 60_000 && c.text) {
          addSegment({ speaker: c.speaker || null, confidence: 0.5, source: "captions", start_ms: Math.max(0, c.t - liveAt), end_ms: Math.max(0, Date.now() - liveAt), text: c.text });
        }
      })
      .then((on) => log("captions", { on }));

    people = await a.participants().catch(() => []);
    ingest.setParticipants(people);
    if (people.length === 0) emptySince = Date.now();

    if (wantPaused) await applyPause(true);
    if (media?.page) {
      const connect = async () => {
        if (lk || ending || !media.page || !cfg.livekit.url || !cfg.livekit.token) return;
        lk = await LiveKitBridge.start(media.page, cfg.livekit, log).catch((e: unknown) => {
          log("livekit_unavailable", String(e).slice(0, 300));
          return null;
        });
        if (lk && wantPaused) await lk.setMuted(true);
      };
      if (!cfg.livekit.url || !cfg.livekit.token) log("livekit_not_configured");
      await connect();
      every(30_000, connect);
    } else if (!browser) {
      log("livekit_bridge_unsupported", { platform: cfg.platform, note: "zoom audio arrives as PCM from the SDK sidecar; not bridged to LiveKit by this runner" });
    }

    // Leave rules + drains, once a second.
    let tick = 0;
    every(1000, async () => {
      const now = Date.now();
      if (emptySince !== null && now - emptySince >= cfg.limits.emptyRoomMs) return end("empty_room");
      if (now - liveAt >= cfg.limits.maxDurationMs) return end("max_duration");
      await media?.drain();
      if (lk) {
        const d = await lk.drain();
        for (const e of d.events) log(e.event, e.detail);
        for (const s of d.data) ingestStt(s, lk.publishedAt);
        for (const w of d.turns) wakeTurn(w);
      }
      if (++tick % 5 === 0 && media) {
        const lv = await media.takeLevels();
        const st = await media.captureStatus();
        const lks = await lk?.status();
        log("capture_level", { peak: lv?.peak ?? 0, mean: lv?.mean ?? 0, tracks: st?.live ?? 0, paused: st?.paused ?? false, livekit: lks ? { state: lks.state, published: lks.published, remotes: lks.remoteParticipants.length, tts: lks.ttsAttached } : null });
      }
    });

    // The bot's own voice drives the tile (speaking + level), then back to listening.
    if (media) {
      let quietSince = 0;
      every(150, async () => {
        if (state !== "live") return;
        const level = await media.outLevel();
        if (level > 0.02) {
          quietSince = 0;
          if (media.currentTile !== "speaking") await media.setTile("speaking");
          media.setLevel(level);
        } else if (media.currentTile === "speaking") {
          quietSince ||= Date.now();
          if (Date.now() - quietSince > 700) {
            media.setLevel(0);
            await media.setTile("listening");
          }
        }
      });
    }
  }

  function ingestStt(s: SttSegment, publishedAt: number): void {
    if (!s.text.trim()) return;
    lastSttAt = Date.now();
    const startAt = Number.isFinite(s.start_at) ? (s.start_at as number) : publishedAt + s.start_ms;
    // Stop listening: nothing said inside a paused window is kept. STT lags,
    // so segments arrive late: the test is when it was said, not when it came.
    if (pauses.some((w) => startAt >= w.from && (w.to === 0 || startAt < w.to))) return;
    const endAt = startAt + Math.max(0, s.end_ms - s.start_ms);
    const who = timeline.label(startAt, endAt);
    const conf = typeof s.confidence === "number" ? Math.min(who.confidence || 1, Math.max(0, Math.min(1, s.confidence))) : who.confidence;
    addSegment({
      speaker: who.speaker,
      confidence: who.speaker ? conf : 0,
      source: "audio",
      start_ms: Math.max(0, Math.round(startAt - liveAt)),
      end_ms: Math.max(0, Math.round(endAt - liveAt)),
      text: s.text.trim(),
    });
  }

  function addSegment(seg: Omit<TurnSegment, "seq"> & { source: "audio" | "captions" }): void {
    const seq = ingest.addSegment(seg);
    const { source: _source, ...rest } = seg;
    segments.push({ seq, ...rest });
    if (segments.length > 20_000) segments.splice(0, segments.length - 20_000);
  }

  // ---- turns (in-pod allternit-api) ------------------------------------------------------------
  function turnMeeting(): TurnMeeting {
    return {
      title: extras.meetingTitle ?? "",
      platform: PLATFORM_LABEL[cfg.platform],
      participants: ingest.present(),
      owner_name: extras.ownerName ?? null,
    };
  }

  /** About the last 3 minutes of labelled transcript. */
  function recentWindow(): TurnSegment[] {
    const cut = Date.now() - liveAt - 3 * 60_000;
    return segments.filter((g) => g.end_ms >= cut);
  }

  function wakeTurn(w: WakeTurn): void {
    // The worker hears "Gizzi, ..." on the call-audio clock; a question asked in a paused window is dropped.
    const at = (lk?.publishedAt ?? liveAt) + (Number(w.start_ms) || 0);
    if (pauses.some((p) => at >= p.from && (p.to === 0 || at < p.to))) return;
    const label = w.asker_label ?? timeline.label(at, at + 1500).speaker ?? "";
    queueTurn({ kind: "wake_word", text: w.text, asker: label, confidence: Number(w.confidence) || 0, turnId: w.turn_id });
  }

  function queueTurn(t: { kind: "wake_word" | "chat_mention"; text: string; asker: string; confidence: number; turnId?: string }): void {
    if (!turnClient || state !== "live" || ending || !t.text.trim()) return;
    // One turn at a time per meeting; two more may wait, older extras are dropped.
    if (pendingTurns.length >= 2) {
      log("turn_dropped", { kind: t.kind, reason: "queue_full" });
      return;
    }
    pendingTurns.push(t);
    void pumpTurns();
  }

  async function pumpTurns(): Promise<void> {
    if (turnBusy) return;
    turnBusy = true;
    try {
      while (pendingTurns.length && !ending) await runTurn(pendingTurns.shift()!);
    } finally {
      turnBusy = false;
    }
  }

  async function runTurn(t: { kind: "wake_word" | "chat_mention"; text: string; asker: string; confidence: number; turnId?: string }): Promise<void> {
    if (!turnClient) return;
    const turnId = t.turnId ?? `turn-${randomUUID()}`;
    const started = Date.now();
    log("turn_started", { turn_id: turnId, kind: t.kind, asker: t.asker || null });
    if (media && state === "live") await media.setTile("thinking");
    try {
      const reply = await turnClient.turn({
        turn_id: turnId,
        session_id: cfg.sessionId,
        trigger: { kind: t.kind, text: t.text.slice(0, 4000) },
        asker: { label: t.asker, participant_class: t.asker ? ingest.classOf(t.asker) : "unknown", confidence: Math.max(0, Math.min(1, t.confidence)) },
        meeting: turnMeeting(),
        transcript: recentWindow(),
        artifacts: [],
        speak_replies: cfg.speakReplies,
        guest_quiet: cfg.guestQuiet,
      });
      if (ending) return;
      // Same key the in-pod allternit-api records the reply under, so cloud keeps one row.
      const key = `turn:${turnId}`;
      if (reply.pageOnly) {
        ingest.addChat({ idempotency_key: key, direction: "out", sender: cfg.displayName, text: reply.text, at: Date.now(), visibility: "page_only" });
        if (reply.chat && reply.chat !== reply.text) await say(reply.chat, `${key}:notice`);
      } else if (reply.chat) {
        await say(reply.chat, key);
      }
      if (reply.speak && cfg.speakReplies) log((await lk?.speak(turnId, reply.speak)) ? "speak_sent" : "speak_not_sent", { turn_id: turnId });
      log("turn_done", { turn_id: turnId, ms: Date.now() - started, page_only: reply.pageOnly, spoke: Boolean(reply.speak) });
    } catch (e) {
      log("turn_failed", { turn_id: turnId, error: String(e).slice(0, 200) });
    } finally {
      if (media && state === "live" && media.currentTile === "thinking") await media.setTile("listening");
    }
  }

  // ---- end ----------------------------------------------------------------------------------------
  const { reason, detail } = await ended;
  for (const t of timers) clearInterval(t);
  timers.clear();
  if (noticeTimer) clearTimeout(noticeTimer);
  for (const u of unsubs.splice(0)) u();
  const wasLive = state === "live" || state === "paused";
  try {
    if (wasLive && adapter && (reason === "other_notetaker" || reason === "left_by_user" || reason === "max_duration")) {
      const bye =
        reason === "other_notetaker"
          ? `Another notetaker joined, so ${cfg.displayName} is leaving. The recap stays on the meeting page.`
          : `${cfg.displayName} is leaving now. The recap will be on the meeting page.`;
      await say(bye);
    }
    await lk?.disconnect();
    await media?.drain(true);
    if (media?.page && opts.recordFile) log("capture_stats", { file: opts.recordFile, bytes: media.recordedBytes });
    if (adapter && state !== "launching" && state !== "") await adapter.leave().catch(() => undefined);
  } catch (e) {
    log("leave_error", String(e).slice(0, 200));
  }
  // The end summary rides in the final batch with the ended state (asked before
  // participants are closed, so the prompt still lists who was there).
  if (wasLive && turnClient) {
    const s = await turnClient
      .summary({ session_id: cfg.sessionId, meeting: turnMeeting(), transcript: segments })
      .catch((e: unknown) => {
        log("summary_failed", String(e).slice(0, 200));
        return null;
      });
    if (s) {
      ingest.setSummary(s);
      log("summary_ready", { chars: s.summary.length, decisions: s.decisions.length, action_items: s.action_items.length });
    }
  }
  ingest.closeParticipants();
  await setState("ended", reason, detail);
  control.stop();
  const t = Date.now();
  await ingest.stop();
  const tIngest = Date.now();
  await media?.stop().catch(() => undefined);
  const tMedia = Date.now();
  await adapter?.close().catch(() => undefined);
  log("shutdown", { ingestMs: tIngest - t, mediaMs: tMedia - tIngest, adapterMs: Date.now() - tMedia });
  await run;
  process.removeListener("SIGTERM", sigterm);
  process.removeListener("SIGINT", sigint);
  const exitCode = reason === "error" ? 1 : ["not_admitted", "guest_blocked", "join_failed"].includes(reason) ? 3 : 0;
  log("pod_exit", { reason, detail, exitCode, liveSeconds: liveAt ? Math.round((Date.now() - liveAt) / 1000) : 0, ingest: ingest.stats });
  return { endReason: reason, detail, exitCode, stats: ingest.stats };
}

// ---- main -------------------------------------------------------------------------------------------

async function main(): Promise<void> {
  const cfg = podConfigFromEnv();
  const r = await runPod(cfg, {
    headless: process.env.MEETING_BOT_HEADLESS === "1",
    chromePath: process.env.CHROME_PATH,
    recordFile: process.env.MEETING_BOT_RECORD_FILE,
    screenshotDir: process.env.MEETING_BOT_SCREENSHOT_DIR,
  });
  await sleep(100);
  process.exit(r.exitCode);
}

const entry = process.argv[1];
if (entry && existsSync(entry) && import.meta.url === pathToFileURL(entry).href) {
  main().catch((e: unknown) => {
    process.stderr.write(`runner: ${String(e)}\n`);
    process.exit(1);
  });
}
