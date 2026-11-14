// Zoom media, video half: the Bot Mode tile as the Zoom bot's camera.
//
// Meet and Teams draw the tile inside the meeting page (media.ts). Zoom has no
// page: the Meeting SDK sidecar takes raw I420 frames over fd 3 (VIDEO_IN,
// adapters/zoom/protocol.ts) and feeds them to its external video source,
// which it registers as FrameDataFormat_I420_LIMITED. So the pod opens the
// same tile page (PodConfig.tileUrl, same name/state/agent query as Meet) in
// its own Chrome and pumps frames to ZoomAdapter.sendVideoFrame:
//
//   tile window --CDP screencast (JPEG, 640x360, ~12 fps)--> converter page
//   converter page: createImageBitmap keeps the newest frame
//   Node pump (fixed rate): Runtime.evaluate __gizziTakeI420() -> the page
//         draws the newest bitmap on an OffscreenCanvas, getImageData,
//         rgbaToI420 (BT.601 limited), base64 -> one reused Node buffer ->
//         ZoomAdapter.sendVideoFrame while the sidecar reports the camera on.
//
// Pull, not push: conversion runs only at the pump rate, only for a new
// frame, and not at all while the camera is off (after the first frame).
// (A Runtime binding pushing ~460 KB per frame stalled Playwright's own CDP
// session on the same page, so frames come back as evaluate results.) The
// screencast and tile states reuse media.ts startTileFeed, so the Zoom tile
// follows exactly the events the Meet tile follows. The audio half is
// livekit-node.ts.

import type { CDPSession } from "playwright-core";
import type { TileState, Unsubscribe } from "./adapter.js";
import type { VideoFrame } from "./adapters/zoom/protocol.js";
import { launchChrome, type ChromeSession } from "./chrome.js";
import { startTileFeed, type TileFeed } from "./media.js";

// ---- pure conversions (unit-tested; rgbaToI420 also runs in the page) ---------------

/**
 * RGBA (canvas ImageData order) to I420, BT.601 limited ("TV") range: Y in
 * 16..235, U/V in 16..240, chroma averaged over each 2x2 block. Width and
 * height must be even. Writes into `out` when given (length w*h*3/2).
 * Coefficients: Rec. 601 studio swing (Y = 16 + 65.481R' + 128.553G' +
 * 24.966B', ...), in 16.16 fixed point.
 * Self-contained on purpose: its source is injected into the converter page.
 */
export function rgbaToI420(rgba: Uint8Array | Uint8ClampedArray, w: number, h: number, out?: Uint8Array): Uint8Array {
  const ySize = w * h;
  const cw = w >> 1;
  const cSize = cw * (h >> 1);
  const dst = out && out.length >= ySize + 2 * cSize ? out : new Uint8Array(ySize + 2 * cSize);
  const uOff = ySize;
  const vOff = ySize + cSize;
  for (let y = 0; y < h; y += 2) {
    for (let x = 0; x < w; x += 2) {
      let rs = 0;
      let gs = 0;
      let bs = 0;
      for (let dy = 0; dy < 2; dy++) {
        const row = (y + dy) * w;
        for (let dx = 0; dx < 2; dx++) {
          const i = (row + x + dx) * 4;
          const r = rgba[i];
          const g = rgba[i + 1];
          const b = rgba[i + 2];
          dst[row + x + dx] = ((16829 * r + 33039 * g + 6416 * b + 32768) >> 16) + 16;
          rs += r;
          gs += g;
          bs += b;
        }
      }
      const r = (rs + 2) >> 2;
      const g = (gs + 2) >> 2;
      const b = (bs + 2) >> 2;
      const c = (y >> 1) * cw + (x >> 1);
      dst[uOff + c] = ((-9714 * r - 19071 * g + 28784 * b + 32768) >> 16) + 128;
      dst[vOff + c] = ((28784 * r - 24103 * g - 4681 * b + 32768) >> 16) + 128;
    }
  }
  return dst;
}

/** I420 (BT.601 limited) back to RGBA. For tests and the local proof PNG. */
export function i420ToRgba(i420: Uint8Array, w: number, h: number): Uint8Array {
  const out = new Uint8Array(w * h * 4);
  const ySize = w * h;
  const cw = w >> 1;
  const cSize = cw * (h >> 1);
  const clip = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : v);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const c = (y >> 1) * cw + (x >> 1);
      const C = i420[y * w + x] - 16;
      const D = i420[ySize + c] - 128;
      const E = i420[ySize + cSize + c] - 128;
      const o = (y * w + x) * 4;
      out[o] = clip((298 * C + 409 * E + 128) >> 8);
      out[o + 1] = clip((298 * C - 100 * D - 208 * E + 128) >> 8);
      out[o + 2] = clip((298 * C + 516 * D + 128) >> 8);
      out[o + 3] = 255;
    }
  }
  return out;
}

// ---- in-page: the converter -----------------------------------------------------------

/** Installed into the converter page with the source of rgbaToI420. Self-contained (serialised). */
function installConverter(W: number, H: number, convert: typeof rgbaToI420): void {
  const w = window as unknown as Record<string, unknown>;
  const canvas = new OffscreenCanvas(W, H);
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("no 2d context");
  const out = new Uint8Array((W * H * 3) / 2);
  let latest: ImageBitmap | null = null;
  let fresh = false;
  let decoding = false;
  const stats = { decoded: 0, skipped: 0, converted: 0, ms: 0 };
  w.__gizziZoomStats = stats;
  // startTileFeed calls this with each screencast JPEG (base64): decode only, keep the newest.
  w.__gizziTileFrame = async (b64: string) => {
    if (decoding) {
      stats.skipped++;
      return false;
    }
    decoding = true;
    try {
      const bin = atob(b64);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      const bmp = await createImageBitmap(new Blob([bytes], { type: "image/jpeg" }));
      latest?.close();
      latest = bmp;
      fresh = true;
      stats.decoded++;
      return true;
    } finally {
      decoding = false;
    }
  };
  // The Node pump: the newest frame as base64 I420, or "" when nothing new arrived.
  w.__gizziTakeI420 = () => {
    if (!latest || !fresh) return "";
    fresh = false;
    const t0 = performance.now();
    ctx.drawImage(latest, 0, 0, W, H);
    convert(ctx.getImageData(0, 0, W, H).data, W, H, out);
    let s = "";
    for (let i = 0; i < out.length; i += 0x8000) s += String.fromCharCode.apply(null, out.subarray(i, i + 0x8000) as unknown as number[]);
    stats.converted++;
    stats.ms += performance.now() - t0;
    return btoa(s);
  };
}

// ---- Node side --------------------------------------------------------------------------

/** What the tile needs from the Zoom adapter (ZoomAdapter implements it). */
export interface ZoomVideoPort {
  sendVideoFrame(f: VideoFrame): boolean;
  readonly cameraOn: boolean;
  onCamera(cb: (on: boolean) => void): Unsubscribe;
}

export interface ZoomTileOptions {
  tileUrl: string;
  displayName: string;
  agent?: string;
  chromePath?: string;
  headless?: boolean;
  /** Camera frame size, even (default 640x360). */
  width?: number;
  height?: number;
  /** Frames per second to the camera (default 12). */
  fps?: number;
  /** Extra Chrome flags (the local proof passes the macOS ones). */
  chromeArgs?: string[];
  onEvent?: (kind: string, detail?: unknown) => void;
}

/** The Zoom camera tile: own Chrome, tile page, converter, frame pump. */
export class ZoomTile {
  readonly width: number;
  readonly height: number;
  readonly fps: number;
  private chrome: ChromeSession | null = null;
  private feed: TileFeed | null = null;
  private cdp: CDPSession | null = null;
  private latest: Buffer | null = null;
  private port: ZoomVideoPort | null = null;
  private unsubCamera: Unsubscribe | null = null;
  private pump: ReturnType<typeof setInterval> | null = null;
  private pulling = false;
  private tileState: TileState = "joining";
  private stopped = false;
  private startedAt = 0;
  private counters = { framesIn: 0, framesSent: 0, framesRefused: 0, pullErrors: 0 };
  /** The bot's own voice level (0..1) for the speaking state; set by the runner from the audio bridge. */
  levelSource: (() => number) | null = null;

  constructor(readonly opts: ZoomTileOptions) {
    this.width = opts.width ?? 640;
    this.height = opts.height ?? 360;
    this.fps = opts.fps ?? 12;
    if (this.width % 2 || this.height % 2) throw new Error("tile frame size must be even");
  }

  private log(kind: string, detail?: unknown): void {
    this.opts.onEvent?.(kind, detail);
  }

  /** Launch Chrome, open the tile and the converter, start pulling frames. */
  async start(): Promise<void> {
    const chrome = await launchChrome({ chromePath: this.opts.chromePath, headless: this.opts.headless, chromeArgs: this.opts.chromeArgs, onEvent: (k, d) => this.log(k, d) });
    this.chrome = chrome;
    if (this.stopped) return void (await chrome.close());
    const page = chrome.page;
    await page.evaluate(`(${installConverter.toString()})(${this.width}, ${this.height}, ${rgbaToI420.toString()})`);
    this.cdp = await chrome.context.newCDPSession(page);
    this.startedAt = Date.now();
    this.feed = await startTileFeed(chrome.browser, page, {
      url: this.opts.tileUrl,
      name: this.opts.displayName,
      state: this.tileState,
      agent: this.opts.agent,
      width: 1280,
      height: 720,
      frameWidth: this.width,
      frameHeight: this.height,
      fps: this.fps,
      onEvent: (k, d) => this.log(k, d),
    });
    if (this.stopped) return void (await this.feed.stop());
    this.pump = setInterval(() => void this.tick(), Math.round(1000 / this.fps));
  }

  /** One pump step: pull a new frame when it will be shown (or none exists yet), then send the newest. */
  private async tick(): Promise<void> {
    if (this.pulling || this.stopped || !this.cdp) return;
    const camera = this.port?.cameraOn ?? false;
    if (this.latest && !camera) return; // camera off: no conversion, no frames
    this.pulling = true;
    try {
      const r = (await this.cdp.send("Runtime.evaluate", { expression: "window.__gizziTakeI420 ? window.__gizziTakeI420() : ''", returnByValue: true })) as {
        result?: { value?: unknown };
      };
      const b64 = typeof r.result?.value === "string" ? r.result.value : "";
      if (b64) {
        const size = (this.width * this.height * 3) / 2;
        const buf = this.latest ?? Buffer.alloc(size);
        if (buf.write(b64, 0, "base64") === size) {
          this.latest = buf;
          this.counters.framesIn++;
          if (this.counters.framesIn === 1) this.log("zoom_tile_first_frame", { ms: Date.now() - this.startedAt });
        }
      }
    } catch {
      this.counters.pullErrors++;
    } finally {
      this.pulling = false;
    }
    if (this.latest && this.port?.cameraOn) {
      if (this.port.sendVideoFrame({ width: this.width, height: this.height, i420: this.latest })) this.counters.framesSent++;
      else this.counters.framesRefused++;
    }
  }

  /** The newest I420 frame (reused buffer), or null before the first one. */
  get latestFrame(): Buffer | null {
    return this.latest;
  }

  /** Send frames to the adapter's camera from now on. */
  attach(port: ZoomVideoPort): void {
    if (this.port || this.stopped) return;
    this.port = port;
    this.unsubCamera = port.onCamera((on) => this.log("zoom_camera", { on }));
    this.log("zoom_media_bridge", { phase: "start", part: "video", width: this.width, height: this.height, fps: this.fps });
  }

  async setTile(state: TileState): Promise<void> {
    if (state === this.tileState) return;
    this.tileState = state;
    await this.feed?.setState(state);
  }

  get currentTile(): TileState {
    return this.tileState;
  }

  setLevel(level: number): void {
    this.feed?.setLevel(level);
  }

  async outLevel(): Promise<number> {
    return this.levelSource?.() ?? 0;
  }

  stats(): { framesIn: number; framesSent: number; framesRefused: number; pullErrors: number; screencast: ReturnType<TileFeed["stats"]> | null } {
    return { ...this.counters, screencast: this.feed?.stats() ?? null };
  }

  async stop(): Promise<void> {
    if (this.stopped) return;
    this.stopped = true;
    if (this.pump) clearInterval(this.pump);
    this.pump = null;
    this.unsubCamera?.();
    const conv = this.cdp
      ? await this.cdp
          .send("Runtime.evaluate", { expression: "window.__gizziZoomStats || null", returnByValue: true })
          .then((r) => (r as { result?: { value?: unknown } }).result?.value ?? null)
          .catch(() => null)
      : null;
    this.log("zoom_media_bridge", { phase: "stop", part: "video", ...this.stats(), converter: conv });
    await this.feed?.stop().catch(() => undefined);
    await this.cdp?.detach().catch(() => undefined);
    await this.chrome?.close().catch(() => undefined);
  }
}
