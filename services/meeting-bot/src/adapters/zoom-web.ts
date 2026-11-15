// Zoom through the browser: the Meeting SDK for Web (Component View) on our
// own page, allternit-ai /meeting-zoom (same origin as the tile), in the bot's
// Chrome like Meet and Teams. It runs wherever the user's Allternit runtime
// runs (Mac, Windows, Linux). The native Linux sidecar (./zoom/) stays behind
// MEETING_BOT_ZOOM_NATIVE=1.
//
// Identity: zoom_obf, as for the sidecar. The SDK JWT comes from the
// identity's sdkSecretRef (jwt:/env:, ./zoom/auth.ts) and the OBF token from
// the identity; `freshTokens` (cloud-api POST /meeting-sessions/:sid/zoom-tokens)
// replaces both right before each join attempt when the runner provides it.
// Zoom documents obfToken on client.join (developers.zoom.us/docs/meeting-sdk/auth/obf/).
//
// Join decisions are the sidecar's (./zoom/index.ts mapJoinFailure): while
// Zoom says the user who connected Zoom is not in the meeting yet, retry every
// 20 s within the admit budget, reporting waiting_admit; a waiting room
// (the SDK's own user on hold) waits for the host inside the same budget.
//
// Media: the runner's installMedia (media.ts) runs before /meeting-zoom loads,
// so the SDK's getUserMedia gets the live tile's video and Allternit Live's
// voice, and the call capture hears the call, exactly as on Meet.

import type { Browser, BrowserContext, Page } from "playwright-core";
import type {
  ActiveSpeakerEvent,
  AdmitResult,
  ChatMessage,
  EndReason,
  JoinPhase,
  JoinRequest,
  JoinResult,
  Participant,
  PlatformAdapter,
  PodConfig,
  TileState,
  Unsubscribe,
} from "../adapter.js";
import { launchChrome, type ChromeSession } from "../chrome.js";
import { resolveSdkJwt } from "./zoom/auth.js";
import { mapJoinFailure } from "./zoom/index.js";
import { parseZoomJoinUrl } from "./zoom/url.js";

export interface ZoomTokens {
  sdkJwt?: string;
  obfToken?: string;
  appPrivilegeToken?: string;
}

export interface ZoomWebAdapterOptions {
  /** allternit-ai /meeting-zoom (same origin as the tile). */
  pageUrl: string;
  chromePath?: string;
  headless?: boolean;
  profileDir?: string;
  installMedia?: (ctx: BrowserContext, browser: Browser, page: Page) => Promise<void>;
  setTile?: (state: TileState) => Promise<void>;
  chromeArgs?: string[];
  onEvent?: (kind: string, detail?: unknown) => void;
  /** Fresh SDK JWT / OBF token right before each join attempt. */
  freshTokens?: () => Promise<ZoomTokens>;
  /** Wait between OBF joins while the authorising user is not in the meeting (default 20 s). */
  ownerRetryMs?: number;
}

type FailReason = Extract<EndReason, "not_admitted" | "guest_blocked" | "join_failed" | "error">;

/**
 * A Web SDK join error as the sidecar's decision. Zoom's web errors carry a
 * code and words, not the native enum names, so both are read.
 */
export function webJoinFailure(code: number | string | null, reason: string): { ownerAbsent: true } | { ownerAbsent: false; reason: FailReason; detail: string } {
  const words = `${reason} ${code ?? ""}`;
  if (/AUTHORIZED_USER_NOT_IN_?MEETING|authori[sz]ed user\b.*\bnot\b.*\bin\b.*\bmeeting|owner.*not.*(in|joined).*meeting/i.test(words)) return { ownerAbsent: true };
  // The native names the sidecar maps, from the web wording.
  const name = /on.?behalf|\bobf\b/i.test(words)
    ? "MEETING_FAIL_ON_BEHALF_TOKEN"
    : /passcode|password/i.test(words)
      ? "MEETING_FAIL_PASSWORD_ERR"
      : /sign.?in|login|authenticat|anonymous|blocked|restricted/i.test(words)
        ? "MEETING_FAIL_BLOCKED_BY_ACCOUNT_SIGN_IN"
        : String(reason || "join failed");
  const m = mapJoinFailure({ status: "failed", code: Number(code) || 0, name }, false);
  return { ownerAbsent: false, reason: m.reason, detail: `${m.detail} (${String(reason).slice(0, 160)})` };
}

type PageEvent =
  | { type: "connection"; state: string; reason?: string }
  | { type: "chat"; id: string; sender: string; senderId: number; text: string; t: number }
  | { type: "speaker"; names: string[] }
  | { type: "people" };

type ZoomWin = Window & {
  __zoomReady?: boolean;
  __zoomJoin?: (p: unknown) => Promise<{ ok: boolean; code?: number | string | null; reason?: string }>;
  __zoomState?: () => { joined: boolean; hold: boolean | null; connection: string; closed: string | null };
  __zoomStartMedia?: () => Promise<{ unmuted: boolean; video: boolean }>;
  __zoomChat?: (text: string) => Promise<boolean>;
  __zoomPeople?: () => Array<{ id: string; name: string; isHost: boolean; hold: boolean }>;
  __zoomLeave?: () => Promise<void>;
};

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export function createZoomWebAdapter(cfg: Pick<PodConfig, "displayName">, opts: ZoomWebAdapterOptions): PlatformAdapter {
  const log = opts.onEvent ?? (() => undefined);
  let session: ChromeSession | null = null;
  let page: Page | null = null;
  let live = false;
  let leaving = false;
  let closed = false;
  let people: Participant[] = [];
  const timers = new Set<ReturnType<typeof setInterval>>();
  const chatSubs = new Set<(m: ChatMessage) => void>();
  const peopleSubs = new Set<(all: Participant[]) => void>();
  const speakerSubs = new Set<(e: ActiveSpeakerEvent) => void>();
  const endedSubs = new Set<(r: Extract<EndReason, "removed" | "left_by_user" | "error">) => void>();
  let endedSent = false;
  const ended = (r: Extract<EndReason, "removed" | "left_by_user" | "error">) => {
    if (endedSent) return;
    endedSent = true;
    for (const cb of endedSubs) cb(r);
  };

  // Runs in the page: `fn` gets only `arg` and reads `window` itself.
  const call = <T>(fn: (arg: any) => T | Promise<T>, arg?: unknown, fallback?: T): Promise<T> => // eslint-disable-line @typescript-eslint/no-explicit-any
    page && !page.isClosed()
      ? page.evaluate(fn as never, arg).then((v) => v as T, () => fallback as T)
      : Promise.resolve(fallback as T);

  async function readPeople(): Promise<void> {
    const raw = await call(() => (window as ZoomWin).__zoomPeople?.() ?? [], undefined, [] as Array<{ id: string; name: string; isHost: boolean; hold: boolean }>);
    const next: Participant[] = raw.filter((p) => !p.hold && p.name !== cfg.displayName).map((p) => ({ id: p.id, name: p.name }));
    if (next.map((p) => p.id).join() === people.map((p) => p.id).join()) return;
    people = next;
    for (const cb of peopleSubs) cb(people);
  }

  function onPageEvent(ev: PageEvent): void {
    if (!ev || typeof ev !== "object") return;
    if (ev.type === "connection") {
      log("zoom_web_connection", { state: ev.state, reason: ev.reason });
      if (live && (ev.state === "Closed" || ev.state === "Fail")) ended(leaving ? "left_by_user" : ev.state === "Fail" ? "error" : "removed");
    } else if (ev.type === "chat" && live) {
      if (ev.sender === cfg.displayName) return; // our own messages echo back
      const m: ChatMessage = { id: ev.id, sender: ev.sender, text: ev.text, t: ev.t, self: false };
      for (const cb of chatSubs) cb(m);
    } else if (ev.type === "speaker" && live) {
      const e: ActiveSpeakerEvent = { t: Date.now(), names: ev.names.filter((n) => n !== cfg.displayName) };
      for (const cb of speakerSubs) cb(e);
    } else if (ev.type === "people" && live) {
      void readPeople();
    }
  }

  async function waitHold(timeoutMs: number): Promise<AdmitResult> {
    const end = Date.now() + timeoutMs;
    while (Date.now() < end && !closed) {
      const s = await call(() => (window as ZoomWin).__zoomState?.() ?? null, undefined, null);
      if (!s || s.closed) return { admitted: false, reason: "not_admitted" };
      if (s.joined && s.hold === false) return { admitted: true };
      await sleep(1000);
    }
    return { admitted: false, reason: "not_admitted" };
  }

  const adapter: PlatformAdapter = {
    platform: "zoom",

    async join(req: JoinRequest, onPhase?: (p: JoinPhase) => void): Promise<JoinResult> {
      const fail = (reason: FailReason, detail: string): JoinResult => ({ ok: false, reason, detail });
      if (req.identity.kind !== "zoom_obf") return fail("join_failed", "Zoom needs a connected Zoom account (zoom_obf identity); anonymous joins are refused by Zoom");
      const target = parseZoomJoinUrl(req.url);
      if (!target) return fail("join_failed", "not a Zoom join link (CONTRACT.md section 7)");
      if (!target.meetingNumber) return fail("join_failed", "Zoom personal-room links (/my/<name>) need the meeting number for the web SDK; paste the numbered link");
      const identity = req.identity;
      let tokens: ZoomTokens;
      try {
        tokens = { sdkJwt: resolveSdkJwt(identity.sdkKey, identity.sdkSecretRef), obfToken: identity.obfToken };
      } catch (e) {
        tokens = { obfToken: identity.obfToken };
        if (!opts.freshTokens) return fail("join_failed", `Zoom credentials: ${(e as Error).message}`);
      }
      const refresh = async () => {
        if (!opts.freshTokens) return;
        try {
          const f = await opts.freshTokens();
          tokens = { sdkJwt: f.sdkJwt || tokens.sdkJwt, obfToken: f.obfToken || tokens.obfToken, appPrivilegeToken: f.appPrivilegeToken || tokens.appPrivilegeToken };
        } catch (e) {
          log("zoom_web_token_refresh_failed", String(e).slice(0, 200));
        }
      };

      try {
        onPhase?.("prejoin");
        session = await launchChrome({ chromePath: opts.chromePath, headless: opts.headless, profileDir: opts.profileDir, chromeArgs: opts.chromeArgs, onEvent: log });
        page = session.page;
        await session.context.exposeBinding("__gizziZoomEvent", (_src, ev: PageEvent) => onPageEvent(ev));
        if (opts.installMedia) await opts.installMedia(session.context, session.browser, page);
      } catch (e) {
        return fail("error", `could not start Chrome for Zoom: ${String(e).slice(0, 200)}`);
      }

      let admitDeadline: number | null = null;
      const enterWaiting = () => {
        if (admitDeadline !== null) return;
        admitDeadline = Date.now() + req.admitTimeoutMs;
        onPhase?.("waiting_admit");
      };
      const admitLeft = () => (admitDeadline === null ? req.admitTimeoutMs : Math.max(0, admitDeadline - Date.now()));
      const retryMs = opts.ownerRetryMs ?? 20_000;

      for (let attempt = 1; !closed; attempt++) {
        // A fresh page per attempt: the SDK client can be created and joined once per page.
        await page.goto(opts.pageUrl, { waitUntil: "domcontentloaded", timeout: req.prejoinTimeoutMs }).catch(() => undefined);
        const loaded = await page
          .waitForFunction(() => (window as ZoomWin).__zoomReady === true, null, { timeout: req.prejoinTimeoutMs })
          .then(() => true, () => false);
        if (!loaded) return fail("join_failed", `the Zoom page did not load (${opts.pageUrl})`);
        await refresh();
        if (!tokens.sdkJwt) return fail("join_failed", "no Zoom SDK JWT (sdkSecretRef or zoom-tokens)");
        if (!tokens.obfToken) return fail("join_failed", "no OBF token; the user must connect Zoom first");
        const params = {
          signature: tokens.sdkJwt,
          meetingNumber: target.meetingNumber,
          password: target.passcode ?? "",
          userName: req.displayName,
          obfToken: tokens.obfToken,
          ...(tokens.appPrivilegeToken ? { recordingToken: tokens.appPrivilegeToken } : {}),
        };
        log("zoom_web_join", { attempt, meeting: target.meetingNumber, passcode: Boolean(target.passcode), recordingToken: Boolean(tokens.appPrivilegeToken) });
        if (attempt === 1) onPhase?.("join_clicked");
        const timeout = attempt === 1 ? req.prejoinTimeoutMs : Math.max(5000, Math.min(req.prejoinTimeoutMs, admitLeft()));
        const res = await Promise.race([
          call((p) => (window as ZoomWin).__zoomJoin?.(p) ?? { ok: false, reason: "no __zoomJoin" }, params, { ok: false, reason: "page closed" } as { ok: boolean; code?: number | string | null; reason?: string }),
          sleep(timeout).then(() => ({ ok: false, code: null, reason: "timeout" })),
        ]);
        if (res.ok) break;
        if (res.reason === "timeout") return fail("join_failed", "Zoom did not answer the join within the pre-join timeout");
        const f = webJoinFailure(res.code ?? null, res.reason ?? "");
        log("zoom_web_join_failed", { attempt, code: res.code ?? null, reason: (res.reason ?? "").slice(0, 200), ownerAbsent: f.ownerAbsent });
        if (f.ownerAbsent) {
          enterWaiting();
          const wait = Math.min(retryMs, admitLeft());
          if (wait <= 0) return fail("not_admitted", `the person who connected Zoom did not join the meeting within ${Math.round(req.admitTimeoutMs / 60_000)} min`);
          await sleep(wait);
          if (admitLeft() <= 0) return fail("not_admitted", `the person who connected Zoom did not join the meeting within ${Math.round(req.admitTimeoutMs / 60_000)} min`);
          continue;
        }
        return fail(f.reason, f.detail);
      }
      if (closed) return fail("error", "closed during join");

      const st = await call(() => (window as ZoomWin).__zoomState?.() ?? null, undefined, null);
      if (st?.hold) {
        enterWaiting();
        const admit = await waitHold(admitLeft());
        if (!admit.admitted) return fail("not_admitted", "not admitted from the Zoom waiting room (declined or waited too long)");
      }
      live = true;
      const media = await call(() => (window as ZoomWin).__zoomStartMedia?.() ?? { unmuted: false, video: false }, undefined, { unmuted: false, video: false });
      log("zoom_web_media", media);
      await readPeople();
      timers.add(setInterval(() => void readPeople(), 2000));
      onPhase?.("live");
      return { ok: true, liveAt: Date.now() };
    },

    async waitForAdmit(timeoutMs: number): Promise<AdmitResult> {
      return waitHold(timeoutMs);
    },
    async postChat(text: string): Promise<boolean> {
      return call((t) => (window as ZoomWin).__zoomChat?.(t) ?? false, text, false);
    },
    onChat(cb): Unsubscribe {
      chatSubs.add(cb);
      return () => chatSubs.delete(cb);
    },
    async participants(): Promise<Participant[]> {
      await readPeople();
      return people;
    },
    onParticipants(cb): Unsubscribe {
      peopleSubs.add(cb);
      return () => peopleSubs.delete(cb);
    },
    onActiveSpeaker(cb): Unsubscribe {
      speakerSubs.add(cb);
      return () => speakerSubs.delete(cb);
    },
    async captions(): Promise<boolean> {
      return false; // the worker's STT is the transcript; Zoom web captions are not read
    },
    async setTile(state: TileState): Promise<void> {
      await opts.setTile?.(state);
    },
    async leave(): Promise<void> {
      if (leaving) return;
      leaving = true;
      await Promise.race([call(() => (window as ZoomWin).__zoomLeave?.(), undefined, undefined), sleep(5000)]);
      ended("left_by_user");
    },
    onEnded(cb): Unsubscribe {
      endedSubs.add(cb);
      return () => endedSubs.delete(cb);
    },
    async close(): Promise<void> {
      if (closed) return;
      closed = true;
      for (const t of timers) clearInterval(t);
      timers.clear();
      await session?.close();
    },
  };
  return adapter;
}
