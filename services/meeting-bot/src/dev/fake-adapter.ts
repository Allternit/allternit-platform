// Fake-adapter harness (plan: "2.2-2.4 build against the interface and use
// 2.1's fake-adapter harness"). A PlatformAdapter over a local two-peer WebRTC
// call, so the runner, media, LiveKit and ingest paths run end to end on a Mac
// with no meeting platform and no account:
//
//   bot side    the pod's own Chrome (chrome.ts + the runner's installMedia),
//               on a plain page served by the caller. The page's
//               RTCPeerConnection goes through media.ts's capture wrapper, and
//               it sends the bot's mic (the getUserMedia override) to the tester.
//   tester      a second, separate Chrome (its own temp profile) that plays a WAV
//               into the call on a loop and measures what it hears from the bot
//               (proves the TTS -> mic path).
//
// Scripted meeting: waiting room for `admitAfterMs`, then live; one participant
// "Jordan Tester" who speaks (active-speaker events every 250 ms while the WAV
// plays), sends one chat message after `chatAfterMs`, and leaves after
// `leaveAfterMs` (so the runner's empty-room rule ends the session).
// Debug and test use only; the runner never picks it by itself.

import type { Page } from "playwright-core";
import type {
  ActiveSpeakerEvent,
  AdmitResult,
  ChatMessage,
  EndReason,
  JoinPhase,
  Participant,
  PlatformAdapter,
  PodConfig,
  TileState,
} from "../adapter.js";
import { launchChrome, type ChromeSession } from "../chrome.js";
import type { BrowserAdapterHooks } from "../runner.js";

export interface FakeCallOptions {
  /** A plain page for the bot side (any same-origin HTML; the harness drives it). */
  botPageUrl: string;
  /** A plain page for the tester (same server). */
  testerPageUrl: string;
  /** WAV the tester plays into the call, fetched by the tester page. */
  wavUrl: string;
  testerName?: string;
  admitAfterMs?: number;
  chatAfterMs?: number;
  chatText?: string;
  /** Scripted chat from the tester (replaces chatAfterMs/chatText when given). */
  chat?: Array<{ afterMs: number; text: string }>;
  /** The tester leaves (stops and hangs up) after this long live; 0 = never. */
  leaveAfterMs?: number;
}

export interface FakeAdapter extends PlatformAdapter {
  /** Messages the bot posted in chat. */
  readonly posted: string[];
  /** Peak RMS the tester heard from the bot since the last call (0..1). */
  testerHeardPeak(): Promise<number>;
}

type CallWin = Window & {
  __pc?: RTCPeerConnection;
  __ac?: AudioContext;
  __dest?: MediaStreamAudioDestinationNode;
  __heard?: number;
  __playing?: boolean;
};

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
// Plain 127.0.0.1 host candidates come from BASE_CHROME_ARGS (a second
// --disable-features here would replace its list). No real devices are opened.
const LOCAL_ICE = "--use-fake-device-for-media-stream";

/** One non-trickle offer/answer round (each side waits for ICE gathering), as in spike/local-test. */
async function negotiate(offerer: Page, answerer: Page): Promise<void> {
  const offer = await offerer.evaluate(async () => {
    const pc = (window as CallWin).__pc as RTCPeerConnection;
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise<void>((r) => {
      if (pc.iceGatheringState === "complete") return r();
      pc.addEventListener("icegatheringstatechange", () => pc.iceGatheringState === "complete" && r());
      setTimeout(r, 3000);
    });
    return (pc.localDescription as RTCSessionDescription).toJSON() as RTCSessionDescriptionInit;
  });
  const answer = await answerer.evaluate(async (o) => {
    const pc = (window as CallWin).__pc as RTCPeerConnection;
    await pc.setRemoteDescription(o);
    await pc.setLocalDescription(await pc.createAnswer());
    await new Promise<void>((r) => {
      if (pc.iceGatheringState === "complete") return r();
      pc.addEventListener("icegatheringstatechange", () => pc.iceGatheringState === "complete" && r());
      setTimeout(r, 3000);
    });
    return (pc.localDescription as RTCSessionDescription).toJSON() as RTCSessionDescriptionInit;
  }, offer);
  await offerer.evaluate(async (a) => {
    await ((window as CallWin).__pc as RTCPeerConnection).setRemoteDescription(a);
  }, answer);
}

export function createFakeAdapter(cfg: Pick<PodConfig, "displayName">, hooks: BrowserAdapterHooks, opts: FakeCallOptions): FakeAdapter {
  const log = hooks.onEvent ?? (() => undefined);
  const name = opts.testerName ?? "Jordan Tester";
  let session: ChromeSession | null = null;
  let bot: Page | null = null;
  let testerSession: ChromeSession | null = null;
  let tester: Page | null = null;
  let closed = false;
  let leftByUs = false;
  let speaking = false;
  let people: Participant[] = [];
  const posted: string[] = [];
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const chatSubs = new Set<(m: ChatMessage) => void>();
  const peopleSubs = new Set<(all: Participant[]) => void>();
  const speakerSubs = new Set<(e: ActiveSpeakerEvent) => void>();
  const endedSubs = new Set<(r: Extract<EndReason, "removed" | "left_by_user" | "error">) => void>();

  const later = (ms: number, fn: () => void) => {
    const t = setTimeout(() => {
      timers.delete(t);
      if (!closed) fn();
    }, ms);
    timers.add(t);
  };
  const setPeople = (next: Participant[]) => {
    people = next;
    for (const cb of peopleSubs) cb(people);
  };

  async function testerLeaves(): Promise<void> {
    log("fake_tester_leaves", { name });
    speaking = false;
    await tester?.evaluate(() => {
      const w = window as CallWin;
      w.__playing = false;
      w.__pc?.close();
    }).catch(() => undefined);
    setPeople([]);
  }

  const adapter: FakeAdapter = {
    platform: "meet",
    posted,

    async join(req, onPhase?: (p: JoinPhase) => void) {
      try {
        onPhase?.("prejoin");
        session = await launchChrome({ chromePath: hooks.chromePath, headless: hooks.headless, chromeArgs: [LOCAL_ICE], onEvent: log });
        bot = session.page;
        bot.on("pageerror", (e) => log("fake_bot_pageerror", String(e).slice(0, 200)));
        if (hooks.installMedia) await hooks.installMedia(session.context, session.browser, bot);
        await bot.goto(opts.botPageUrl, { waitUntil: "domcontentloaded" });

        // Spawned like the pod's own Chrome, so close() always ends the process.
        testerSession = await launchChrome({ chromePath: hooks.chromePath, headless: hooks.headless ?? true, chromeArgs: [LOCAL_ICE] });
        tester = testerSession.page;
        await tester.goto(opts.testerPageUrl, { waitUntil: "domcontentloaded" });
        onPhase?.("join_clicked");

        // The tester sends a WAV track and listens to the bot's mic.
        await tester.evaluate(async () => {
          const w = window as CallWin;
          w.__pc = new RTCPeerConnection();
          w.__ac = new AudioContext({ sampleRate: 48000 });
          await w.__ac.resume();
          w.__dest = w.__ac.createMediaStreamDestination();
          w.__pc.addTrack(w.__dest.stream.getAudioTracks()[0], w.__dest.stream);
          w.__heard = 0;
          const an = w.__ac.createAnalyser();
          an.fftSize = 2048;
          const buf = new Float32Array(an.fftSize);
          w.__pc.addEventListener("track", (e) => {
            const s = new MediaStream([e.track]);
            const el = document.createElement("audio");
            el.muted = true;
            el.srcObject = s;
            void el.play().catch(() => undefined);
            (w.__ac as AudioContext).createMediaStreamSource(s).connect(an);
          });
          setInterval(() => {
            an.getFloatTimeDomainData(buf);
            let sum = 0;
            for (const x of buf) sum += x * x;
            w.__heard = Math.max(w.__heard ?? 0, Math.sqrt(sum / buf.length));
          }, 100);
        });
        // The bot page makes its own connection (like Meet does), with its mic on it.
        await bot.evaluate(async () => {
          const w = window as CallWin;
          w.__pc = new RTCPeerConnection();
          const s = await navigator.mediaDevices.getUserMedia({ audio: true });
          for (const t of s.getAudioTracks()) w.__pc.addTrack(t, s);
        });

        onPhase?.("waiting_admit");
        await sleep(opts.admitAfterMs ?? 2000);
        if (closed) return { ok: false, reason: "error", detail: "closed while waiting" };
        await negotiate(tester, bot);
        const connected = await bot
          .waitForFunction(() => (window as CallWin).__pc?.connectionState === "connected", null, { timeout: 15_000 })
          .then(() => true, () => false);
        if (!connected) return { ok: false, reason: "join_failed", detail: "fake call did not connect" };

        // Talk on a loop until the tester leaves.
        void tester.evaluate(async (url) => {
          const w = window as CallWin;
          const ac = w.__ac as AudioContext;
          const buf = await ac.decodeAudioData(await (await fetch(url)).arrayBuffer());
          w.__playing = true;
          while (w.__playing) {
            const src = ac.createBufferSource();
            src.buffer = buf;
            src.connect(w.__dest as MediaStreamAudioDestinationNode);
            await new Promise<void>((r) => {
              src.onended = () => r();
              src.start();
            });
            await new Promise((r) => setTimeout(r, 400));
          }
        }, opts.wavUrl).catch(() => undefined);
        speaking = true;
        const liveAt = Date.now();
        onPhase?.("live");
        setPeople([{ id: "tester-1", name }]);
        const speak = setInterval(() => {
          const e: ActiveSpeakerEvent = { t: Date.now(), names: speaking ? [name] : [] };
          for (const cb of speakerSubs) cb(e);
        }, 250);
        timers.add(speak as unknown as ReturnType<typeof setTimeout>);
        const script = opts.chat ?? [{ afterMs: opts.chatAfterMs ?? 3000, text: opts.chatText ?? "Hi Gizzi, please take notes." }];
        script.forEach((c, i) =>
          later(c.afterMs, () => {
            const m: ChatMessage = { id: `fake-msg-${i + 1}`, sender: name, text: c.text, t: Date.now(), self: false };
            for (const cb of chatSubs) cb(m);
          }),
        );
        if (opts.leaveAfterMs) later(opts.leaveAfterMs, () => void testerLeaves());
        return { ok: true, liveAt };
      } catch (e) {
        return { ok: false, reason: "error", detail: String(e).slice(0, 300) };
      }
    },

    async waitForAdmit(): Promise<AdmitResult> {
      return { admitted: true };
    },
    async postChat(text) {
      posted.push(text);
      return true;
    },
    onChat(cb) {
      chatSubs.add(cb);
      return () => chatSubs.delete(cb);
    },
    async participants() {
      return people;
    },
    onParticipants(cb) {
      peopleSubs.add(cb);
      return () => peopleSubs.delete(cb);
    },
    onActiveSpeaker(cb) {
      speakerSubs.add(cb);
      return () => speakerSubs.delete(cb);
    },
    async captions() {
      return false;
    },
    async setTile(state: TileState) {
      await hooks.setTile?.(state);
    },
    async leave() {
      if (leftByUs) return;
      leftByUs = true;
      await bot?.evaluate(() => (window as CallWin).__pc?.close()).catch(() => undefined);
      for (const cb of endedSubs) cb("left_by_user");
    },
    onEnded(cb) {
      endedSubs.add(cb);
      return () => endedSubs.delete(cb);
    },
    async testerHeardPeak() {
      if (!tester || tester.isClosed()) return 0;
      return tester
        .evaluate(() => {
          const w = window as CallWin;
          const v = w.__heard ?? 0;
          w.__heard = 0;
          return v;
        })
        .catch(() => 0);
    },
    async close() {
      if (closed) return;
      closed = true;
      for (const t of timers) clearTimeout(t);
      timers.clear();
      const t = Date.now();
      await testerSession?.close();
      const tTester = Date.now();
      await session?.close();
      log("fake_closed", { testerMs: tTester - t, botMs: Date.now() - tTester });
    },
  };
  return adapter;
}
