// The bot's camera picture: our real Bot Mode stage (allternit-ai /meeting-tile),
// streamed into the meeting page's camera track.
//
// The tile page opens in its own window of the same Chrome (a second window,
// not a tab: a background tab is "hidden", and the stage pauses its animation
// while hidden). CDP Page.startScreencast sends its frames as JPEG; each one
// goes to window.__gizziTileFrame() in the meeting page, which draws it into
// the canvas gizzi-media.ts streams as the camera. Frames are acked no faster
// than FPS, so the tile's compositor is paced at FPS too (CPU).
//
//   const tile = await startTileFeed(browser, meetPage, { url, name, state: "joining" });
//   await tile.setState("listening");    // joining | listening | thinking | speaking | paused
//   tile.setLevel(0.4);                   // the bot's voice level while it speaks
//   await tile.stop();

import type { Browser, CDPSession, Page } from "playwright-core";

export type TileState = "joining" | "listening" | "thinking" | "speaking" | "paused";
export const TILE_STATES: readonly TileState[] = ["joining", "listening", "thinking", "speaking", "paused"];

export interface TileFeedOptions {
  /** The /meeting-tile page (dev server or ai.allternit.com). */
  url: string;
  name: string;
  state?: TileState;
  /** base64 JSON of the user's own bot (an allternit-ai Agent); absent = Gizzi. */
  agent?: string;
  width?: number;
  height?: number;
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

export function tileUrl(base: string, opts: { name: string; state: TileState; agent?: string }): string {
  const u = new URL(base);
  u.searchParams.set("name", opts.name);
  u.searchParams.set("state", opts.state);
  if (opts.agent) u.searchParams.set("agent", opts.agent);
  return u.toString();
}

export async function startTileFeed(browser: Browser, meetPage: Page, opts: TileFeedOptions): Promise<TileFeed> {
  const width = opts.width ?? 1280;
  const height = opts.height ?? 720;
  const fps = opts.fps ?? 15;
  const frameMs = Math.round(1000 / fps);
  const log = opts.onEvent ?? (() => undefined);
  const context = meetPage.context();

  // A second window, so the tile stays visible while the meeting window is in front.
  const browserSession = await browser.newBrowserCDPSession();
  const url = tileUrl(opts.url, { name: opts.name, state: opts.state ?? "joining", agent: opts.agent });
  const pageEvent = context.waitForEvent("page", { timeout: 15_000 });
  await browserSession.send("Target.createTarget", { url, newWindow: true, background: true, width, height });
  const tilePage = await pageEvent;
  await browserSession.detach().catch(() => undefined);

  const tile: CDPSession = await context.newCDPSession(tilePage);
  await tile.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  await tilePage.waitForFunction(() => (window as unknown as { __tileReady?: boolean }).__tileReady === true, null, {
    timeout: opts.loadTimeoutMs ?? 45_000,
  });
  log("tile_ready", { url });

  // Frames go to the meeting page with a raw Runtime.evaluate (no Playwright
  // serialisation); one in flight at a time, newer frames replace a waiting one.
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

  // Chrome keeps up to two screencast frames unacked, so acks are spaced
  // frameMs apart (not just delayed): that is what holds the source at FPS.
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
  await tile.send("Page.startScreencast", { format: "jpeg", quality: 80, maxWidth: width, maxHeight: height, everyNthFrame: 1 });
  log("tile_screencast_started", { width, height, fps });

  return {
    tilePage,
    async setState(state) {
      const ok = await tilePage
        .evaluate((s) => (window as unknown as { __setTileState?: (x: string) => boolean }).__setTileState?.(s) ?? false, state)
        .catch(() => false);
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
      await tile.send("Page.stopScreencast").catch(() => undefined);
      await tile.detach().catch(() => undefined);
      await meet.detach().catch(() => undefined);
      await tilePage.close().catch(() => undefined);
    },
  };
}
