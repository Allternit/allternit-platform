/**
 * Phone Use act-then-verify and the result envelope
 * (docs/contracts/CONTRACT-phone-use-2026-11-16.md §4, §4.1).
 *
 * A tap route (Mirroring, Bluetooth from a Mac, Android or a second Apple
 * device) sends input blind: nothing on the phone acknowledges a tap. So every
 * acting call is checked with a fresh frame:
 *
 *   1. capture frame A
 *   2. send the action
 *   3. wait for the screen to settle (a quiet window), capture frame B
 *   4. A and B differ (grayscale diff, as the USB Agent already checks); for
 *      phone.type, the typed text must also show in frame B when the field is
 *      visible, else the result is sent but not verified (never typed twice)
 *   5. unchanged: send again, up to 2 more times (3 sends); a retried tap on a
 *      pointer route re-homes the pointer first
 *   6. still unchanged: ok:false, verified:false, detail.error "no_change"
 *
 * Pure orchestration: frames, sending, the clock and OCR are injected, so the
 * loop is unit-tested with fake lanes and no phone.
 */

import { createHash } from 'node:crypto';
import type { NeedsYou } from './phone-link-health.js';
import type { RouteId } from './phone-route.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';
import { changed, type Gray } from './phone-usb-agent-vision.js';

/** The acting tools act-then-verify wraps on a tap route (contract §4.1). */
export const VERIFIED_TOOLS: ReadonlySet<string> = new Set([
  'phone.tap',
  'phone.type',
  'phone.swipe',
  'phone.key',
  'phone.home',
  'phone.back',
  'phone.open_app',
  'phone.find_and_tap',
]);

/** Sends in total: the first one plus two retries. */
export const MAX_ATTEMPTS = 3;

/** One frame of the phone screen. `gray` drives the diff; `hash` identifies the frame in a no_change result. */
export interface VerifyFrame {
  gray: Gray | null;
  hash: string;
}

/** A frame from JPEG/PNG bytes: hash of the bytes, gray from the injected decoder. */
export function frameOf(bytes: Uint8Array, gray: Gray | null): VerifyFrame {
  return { gray, hash: createHash('sha256').update(bytes).digest('hex').slice(0, 16) };
}

/**
 * Every phone.* result on a routed iPhone (contract §4). Existing tools keep
 * their fields at the top level as well (dataBase64, text, screenChanged …),
 * so today's readers (MCP shim, allternit-api task loop, replay) keep working
 * until plan step 14 moves them to `detail`.
 */
export type PhoneUseResult = ToolResult & {
  route: RouteId;
  verified: boolean;
  needs_you?: NeedsYou;
  detail: Record<string, unknown>;
};

/** Tools whose result is a read: `verified` is true when the read succeeded. */
export const READ_TOOLS: ReadonlySet<string> = new Set([
  'phone.describe',
  'phone.screenshot',
  'phone.read_ui',
  'phone.actions.list',
  'phone.home.list',
]);

const ENVELOPE_KEYS = new Set(['ok', 'route', 'verified', 'needs_you', 'detail']);

/**
 * Wrap a lane's raw ToolResult in the envelope. `detail` is the lane's own
 * `detail` when it sent one (new tools), plus the extras given here, plus
 * error/message on a failure. `verified` defaults to: reads that succeeded,
 * or an acting tool whose lane said the screen changed (USB Agent).
 */
export function toEnvelope(
  route: RouteId,
  tool: string,
  raw: ToolResult,
  opts: { verified?: boolean; detail?: Record<string, unknown>; needsYou?: NeedsYou } = {},
): PhoneUseResult {
  const bag = raw as unknown as Record<string, unknown>;
  const flat = Object.fromEntries(Object.entries(bag).filter(([k]) => !ENVELOPE_KEYS.has(k)));
  const laneDetail = bag.detail && typeof bag.detail === 'object' && !Array.isArray(bag.detail) ? (bag.detail as Record<string, unknown>) : {};
  const needs = opts.needsYou ?? (isNeedsYou(bag.needs_you) ? bag.needs_you : undefined);
  if (!raw.ok) {
    return {
      ...flat,
      ok: false,
      error: raw.error,
      message: raw.message,
      route,
      verified: false,
      ...(needs ? { needs_you: needs } : {}),
      detail: { ...laneDetail, ...opts.detail, error: (opts.detail?.error as string | undefined) ?? (laneDetail.error as string | undefined) ?? raw.error, message: raw.message },
    } as PhoneUseResult;
  }
  const verified = opts.verified ?? (READ_TOOLS.has(tool) || raw.screenChanged === true);
  return { ...flat, ok: true, route, verified, detail: { ...laneDetail, ...opts.detail } } as PhoneUseResult;
}

/** A failure envelope with no lane result behind it. */
export function failEnvelope(
  route: RouteId,
  error: ToolErrorCode,
  message: string,
  opts: { needsYou?: NeedsYou; detail?: Record<string, unknown> } = {},
): PhoneUseResult {
  return toEnvelope(route, '', { ok: false, error, message }, opts);
}

function isNeedsYou(v: unknown): v is NeedsYou {
  return !!v && typeof v === 'object' && typeof (v as NeedsYou).code === 'string' && typeof (v as NeedsYou).message === 'string';
}

export interface VerifyTarget {
  /** One frame now, or null when the route has no eyes right now. */
  frame(): Promise<VerifyFrame | null>;
  /** Send the action once. */
  send(): Promise<ToolResult>;
  /** Pointer routes: drive the pointer to the corner (contract §6), before a retried tap. */
  rehome?(): Promise<void>;
  /** OCR of a frame, for phone.type: the text, and whether a text field is on screen. */
  readText?(frame: VerifyFrame): Promise<{ text: string; fieldVisible: boolean } | null>;
}

export interface VerifyOptions {
  route: RouteId;
  tool: string;
  /** phone.type: the text that must show in frame B. */
  typed?: string;
  /** Quiet window: two frames this far apart that do not differ = settled (default 300 ms). */
  quietMs?: number;
  /** Upper bound on settling (default 3000 ms; the route targets in contract §1.1 are below it). */
  settleMaxMs?: number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
  /** Frame comparison; defaults to framesDiffer. */
  differ?: (a: VerifyFrame, b: VerifyFrame) => boolean;
}

/** Grayscale diff when both frames decode (status bar and small noise ignored), else a hash compare. */
export function framesDiffer(a: VerifyFrame, b: VerifyFrame): boolean {
  if (a.gray && b.gray) return changed(a.gray, b.gray);
  return a.hash !== b.hash;
}

/** Wait until two frames `quietMs` apart do not differ, or `settleMaxMs` passes. Returns the last frame. */
export async function settle(
  target: Pick<VerifyTarget, 'frame'>,
  opts: Pick<VerifyOptions, 'quietMs' | 'settleMaxMs' | 'sleep' | 'now' | 'differ'> = {},
): Promise<VerifyFrame | null> {
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const now = opts.now ?? Date.now;
  const quiet = opts.quietMs ?? 300;
  const deadline = now() + (opts.settleMaxMs ?? 3000);
  const differ = opts.differ ?? framesDiffer;
  await sleep(quiet);
  let last = await target.frame();
  while (last && now() < deadline) {
    await sleep(quiet);
    const next = await target.frame();
    if (!next) return last;
    if (!differ(last, next)) return next;
    last = next;
  }
  return last;
}

/**
 * Act, then check the screen changed (contract §4.1). A send that fails is
 * returned as is (no retry: the action may not be safe to repeat). No frame
 * before the action means nothing can be checked: one send, verified:false.
 */
export async function actThenVerify(target: VerifyTarget, opts: VerifyOptions): Promise<PhoneUseResult> {
  const differ = opts.differ ?? framesDiffer;
  const before = await target.frame();
  if (!before) {
    const raw = await target.send();
    return toEnvelope(opts.route, opts.tool, raw, { verified: false, detail: { attempts: 1, changed: null, verify: 'no_frame' } });
  }
  let after: VerifyFrame | null = null;
  let last: ToolResult = { ok: true };
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt += 1) {
    if (attempt > 1 && opts.tool === 'phone.tap') await target.rehome?.();
    last = await target.send();
    if (!last.ok) return toEnvelope(opts.route, opts.tool, last, { detail: { attempts: attempt } });
    after = await settle(target, opts);
    const moved = !!after && differ(before, after);
    if (!moved) continue;
    // The screen changed, so the action landed somewhere: never send it again
    // (a second phone.type would type the text twice). Only the typed-text check decides `verified`.
    const shows = await typedShows(target, after!, opts.typed);
    return toEnvelope(opts.route, opts.tool, last, {
      verified: shows,
      detail: { attempts: attempt, changed: true, ...(shows ? {} : { typed_seen: false }) },
    });
  }
  return toEnvelope(
    opts.route,
    opts.tool,
    { ok: false, error: 'no_change', message: `The screen did not change after ${MAX_ATTEMPTS} tries. Take a screenshot and look before trying again.` },
    { detail: { attempts: MAX_ATTEMPTS, changed: false, frames: { before: before.hash, after: after?.hash ?? null } } },
  );
}

/** phone.type: the text must be in frame B's OCR when a field is visible; with no OCR or no field, only the diff counts. */
async function typedShows(target: VerifyTarget, frame: VerifyFrame, typed: string | undefined): Promise<boolean> {
  if (!typed || !target.readText) return true;
  const seen = await target.readText(frame);
  if (!seen || !seen.fieldVisible) return true;
  return seen.text.toLowerCase().includes(typed.trim().toLowerCase());
}
