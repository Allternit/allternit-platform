// Live mode (browser platforms): the bot IS an Allternit Live voice session.
//
// The tile tab runs allternit-ai /meeting-tile?live=1 (src/lib/voice/
// meeting-live.ts there): a Voice Session on the app's default browser
// engine (in-page sherpa-onnx, models from runtime.allternit.com; no keys, no
// metering), whose mic is the call and whose voice and stage video go back
// into the call. Tile tab and meeting tab talk over a WebRTC loopback in the
// pod's Chrome; this file relays its SDP through Playwright bindings, so
// there is no signalling server.
//
//   tile tab --offer--> Node --> meeting tab (media.ts answers, adds the call mix)
//   meeting tab --answer--> Node --> tile tab
//   hello from either side (a page (re)loaded) --> Node asks the tile to offer again
//
// The tile tab also sends every utterance it heard (`__gizziLiveHeard`) and
// its state (`__gizziLiveStatus`). The runner decides what to answer and
// speaks with `say()`. When Live is not ready (old tile page, engine still
// loading, an error) `voiceReady` is false and the runner keeps today's
// chat replies and the worker's voice.

import type { Browser, BrowserContext, CDPSession, Page } from "playwright-core";
import type { TileState } from "./adapter.js";
import { openTileWindow, screencastTile, tileUrl, within, type LoopMessage, type TileFeed } from "./media.js";

/** What the tile tab reports (allternit-ai meeting-live.ts LiveStatus). */
export interface LiveStatus {
  phase: string;
  engine: string | null;
  error: string | null;
  video: boolean;
  frames: number;
  loop: string;
}

const READY_PHASES = new Set(["listening", "hearing", "thinking", "speaking"]);

export interface LiveHubOptions {
  onEvent: (kind: string, detail?: unknown) => void;
  /** A finished utterance from the call, as Live heard it. */
  onHeard?: (text: string) => void;
}

export class LiveHub {
  tilePage: Page | null = null;
  status: LiveStatus | null = null;
  private helloMeet = false;
  private helloTile = false;
  private tileHello: (() => void) | null = null;
  private lastPhase = "";
  private disabled = false;
  onHeard: ((text: string) => void) | undefined;

  constructor(
    readonly meetPage: Page,
    private readonly opts: LiveHubOptions,
  ) {
    this.onHeard = opts.onHeard;
  }

  private log(kind: string, detail?: unknown): void {
    this.opts.onEvent(kind, detail);
  }

  /** True while the Live session is connected and listening (replies can be spoken). */
  get voiceReady(): boolean {
    return Boolean(this.tilePage && !this.tilePage.isClosed() && this.status && READY_PHASES.has(this.status.phase));
  }

  /** Context bindings, before the meeting page loads (they reach every page of the context). */
  async bind(ctx: BrowserContext): Promise<void> {
    await ctx.exposeBinding("__gizziLoopSignal", (src, msg: LoopMessage) => this.route(src.page, msg));
    await ctx.exposeBinding("__gizziLiveHeard", (src, text: string) => {
      if (src.page !== this.tilePage || typeof text !== "string" || !text.trim()) return;
      this.log("live_heard", { chars: text.length });
      this.onHeard?.(text);
    });
    await ctx.exposeBinding("__gizziLiveStatus", (src, s: LiveStatus) => {
      if (src.page !== this.tilePage || !s || typeof s !== "object") return;
      this.status = s;
      if (s.phase !== this.lastPhase) {
        this.lastPhase = s.phase;
        this.log("live_state", { phase: s.phase, engine: s.engine, error: s.error, video: s.video, loop: s.loop });
        // Chat replies and the worker's voice take over (runner liveActive()).
        if (s.phase === "error" || s.phase === "ended") this.log("live_unavailable", { reason: "session", error: s.error });
      } else {
        this.log("live_stats", { phase: s.phase, frames: s.frames, loop: s.loop, video: s.video });
      }
    });
  }

  /** No live tile this session: the meeting page uses its canvas camera (now, and after any reload). */
  disable(): void {
    this.disabled = true;
    this.tilePage = null;
    if (this.helloMeet) void this.deliver(this.meetPage, { type: "off" });
  }

  private route(from: Page, msg: LoopMessage): void {
    if (!msg || typeof msg !== "object") return;
    if (this.disabled) {
      if (msg.type === "hello" && from === this.meetPage) {
        this.helloMeet = true;
        void this.deliver(this.meetPage, { type: "off" });
      }
      return;
    }
    const fromTile = from === this.tilePage;
    if (msg.type === "hello") {
      if (fromTile) {
        this.helloTile = true;
        this.tileHello?.();
      } else if (from === this.meetPage) {
        this.helloMeet = true;
      } else return;
      this.log("live_loop_hello", { side: fromTile ? "tile" : "meet" });
      if (this.helloTile && this.helloMeet) void this.deliver(this.tilePage, { type: "restart" });
      return;
    }
    if (fromTile && msg.type === "offer") void this.deliver(this.meetPage, msg);
    else if (from === this.meetPage && msg.type === "answer") void this.deliver(this.tilePage, msg);
  }

  private async deliver(to: Page | null, msg: LoopMessage): Promise<void> {
    if (!to || to.isClosed()) return;
    const ok = await to.evaluate((m) => {
      const w = window as unknown as { __gizziLoopIn?: (x: unknown) => void };
      if (!w.__gizziLoopIn) return false;
      w.__gizziLoopIn(m);
      return true;
    }, msg).catch(() => false);
    this.log("live_loop_signal", { type: msg.type, to: to === this.tilePage ? "tile" : "meet", ok });
  }

  /**
   * Open the live tile. Resolves with its TileFeed once the page drew and
   * said hello (it has a live mode). Throws when it has none, so the caller
   * falls back to the screencast tile. A tile without HTML-in-canvas keeps
   * its voice session and is screencast into the meeting's canvas camera.
   */
  async open(browser: Browser, url: string, o: { name: string; agent?: string; helloTimeoutMs?: number }): Promise<TileFeed> {
    const full = tileUrl(url, { name: o.name, state: "joining", agent: o.agent, live: true });
    const helloSeen = new Promise<boolean>((resolve) => {
      this.tileHello = () => resolve(true);
    });
    const opened = openTileWindow(browser, this.meetPage.context(), full);
    // The page is known before it says hello: the binding checks the source page.
    const pageEvent = this.meetPage.context().waitForEvent("page", { timeout: 15_000 }).then((p) => (this.tilePage = p)).catch(() => null);
    const { tilePage, tile } = await opened;
    await pageEvent;
    this.tilePage = tilePage;
    this.log("tile_ready", { url, live: true });
    const hello = this.helloTile || (await within(helloSeen, o.helloTimeoutMs ?? 10_000, false));
    if (!hello) {
      this.tilePage = null;
      await tilePage.close({ runBeforeUnload: false }).catch(() => undefined);
      throw new Error("tile page has no live mode (no hello)");
    }
    const video = await within(
      tilePage.evaluate(() => (window as unknown as { __gizziLive?: { status: () => { video: boolean } } }).__gizziLive?.status().video ?? false),
      3000,
      false,
    );
    if (!video) {
      this.log("live_video_unavailable", { note: "no HTML-in-canvas in this Chrome; screencasting the live tile" });
      const feed = await screencastTile(tilePage, tile, this.meetPage, { onEvent: this.opts.onEvent });
      return this.feed(tilePage, tile, feed);
    }
    this.log("live_video_started", { width: 640, height: 360, fps: 24, via: "drawElementImage+captureStream+loopback" });
    return this.feed(tilePage, tile, null);
  }

  private feed(tilePage: Page, tile: CDPSession, cast: TileFeed | null): TileFeed {
    return {
      tilePage,
      setState: async (state: TileState) => {
        const ok = await within(
          tilePage.evaluate((s) => (window as unknown as { __setTileState?: (x: string) => boolean }).__setTileState?.(s) ?? false, state),
          2000,
          false,
        );
        this.log("tile_state", { state, ok });
      },
      // Live drives its own voice swell; the level is only used without Live's voice.
      setLevel: (level: number) => {
        if (this.voiceReady) return;
        void tile.send("Runtime.evaluate", { expression: `window.__setTileLevel && window.__setTileLevel(${Number(level) || 0})` }).catch(() => undefined);
      },
      stats: () => cast?.stats() ?? { framesIn: this.status?.frames ?? 0, framesSent: this.status?.frames ?? 0, framesDropped: 0 },
      stop: async () => {
        if (cast) await cast.stop();
        else {
          await within(tile.detach(), 2000, undefined);
          await within(tilePage.close({ runBeforeUnload: false }), 3000, undefined);
        }
        this.tilePage = null;
      },
    };
  }

  /** Speak through Live. False when Live is not ready or the tile refused (the caller falls back to chat). */
  async say(text: string): Promise<boolean> {
    const page = this.tilePage;
    if (!page || page.isClosed() || !this.voiceReady) return false;
    return within(
      page.evaluate((t) => (window as unknown as { __gizziLiveSay?: (x: string) => boolean }).__gizziLiveSay?.(t) ?? false, text),
      3000,
      false,
    );
  }
}

// ---- wake word on what Live heard -------------------------------------------------------
//
// The same rules as the worker (services/voice/src/meeting_worker/wake.rs):
// the name as STT spells it (Gizzi, Gizzy, Jizzy, Gissy, Giz-ee ...), as the
// first word, right after a pause mark, or after a greeting; never "Gizzi's".

const NAME = /^[gj]i[sz]{1,2}(y|i|ie|ee|ey)$/;
const GREETINGS = new Set(["hey", "hi", "okay", "ok", "yo", "hello"]);

export interface LiveWake {
  heard: string;
  /** What follows the name; empty = the question comes in the next utterance. */
  question: string;
}

export function liveWake(text: string): LiveWake | null {
  const re = /[\p{L}\p{N}]+(?:['’-][\p{L}\p{N}]+)*/gu;
  let m: RegExpExecArray | null;
  let prevEnd = 0;
  let prev = "";
  let first = true;
  const words: Array<{ raw: string; norm: string; start: number; end: number; afterPause: boolean; possessive: boolean }> = [];
  while ((m = re.exec(text))) {
    const gap = text.slice(prevEnd, m.index);
    const lower = m[0].toLowerCase().replace(/’/g, "'");
    words.push({
      raw: m[0],
      norm: lower.replace(/'s$/, "").replace(/[^\p{L}\p{N}]/gu, ""),
      start: m.index,
      end: m.index + m[0].length,
      afterPause: first || /[,.?!;:—–…]/.test(gap),
      possessive: lower.endsWith("'s"),
    });
    first = false;
    prevEnd = m.index + m[0].length;
  }
  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    prev = i > 0 ? words[i - 1].norm : "";
    let end = w.end;
    let isName = !w.possessive && NAME.test(w.norm);
    // "giz ee": the name split in two.
    if (!isName && /^[gj]i[sz]{1,2}$/.test(w.norm) && words[i + 1] && /^(y|ee|ie|ey|e)$/.test(words[i + 1].norm)) {
      isName = !words[i + 1].possessive;
      end = words[i + 1].end;
    }
    if (!isName) continue;
    if (!(w.afterPause || GREETINGS.has(prev))) continue;
    const startAt = GREETINGS.has(prev) && i > 0 ? words[i - 1].start : w.start;
    return { heard: text.slice(startAt, end), question: text.slice(end).replace(/^[\s,.?!;:—–…-]+/, "").trim() };
  }
  return null;
}
