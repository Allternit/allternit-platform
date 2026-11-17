/**
 * The command-sending half shared by the two Phone Use routes whose Bluetooth
 * keyboard and mouse is another device running the Allternit app
 * (docs/contracts/CONTRACT-phone-use-2026-11-16.md §6, §9.2):
 *   - `bt-android` (phone-bt-android.ts): the person's Android phone
 *   - `bt-apple`   (phone-bt-apple.ts): a second iPhone or iPad
 *
 * Both send the same `hid.*` methods over the sending device's app-phone
 * control socket (the node's `POST /api/v1/phones/app/:id/rpc`), get the same
 * error codes back, and act through the same keyboard/mouse core
 * (phone-hid-core.ts). Only how each picks its sender differs, so that stays
 * in each lane.
 */

import { HidError, type HidCore, type HidLinkStatus, type HidTransport, type Point } from './phone-hid-core.js';
import { needsYou } from './phone-link-health.js';
import type { LaneTool, RouteId } from './phone-route.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';

export type AppSenderRpcResult = { ok: true; result: Record<string, unknown> } | { ok: false; status: number; code: string; message: string };

/** The node's app-phone rpc (`AppPhoneNode.rpc`), by phone id (the serial without `app:`). */
export type AppSenderRpc = (phoneId: string, method: string, params: Record<string, unknown>, timeoutMs: number) => Promise<AppSenderRpcResult>;

export const APP_PREFIX = 'app:';
const STATUS_TIMEOUT_MS = 5_000;
const HID_TIMEOUT_MS = 15_000;
/** hid.pointer.reset sweeps ~158 reports on the sending device. */
const RESET_TIMEOUT_MS = 20_000;
/** hid.type text per message (contract §9.2: max 500). */
const TYPE_CHUNK = 200;

/** A failure the sending device (or the node) answered. */
export class AppSenderError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** The sending device's answer to hid.pair.status. */
export interface SenderStatus extends HidLinkStatus {
  available: boolean;
  message?: string;
}

export function readSenderStatus(r: Record<string, unknown>): SenderStatus {
  return {
    paired: r.paired === true,
    connected: r.connected === true,
    available: r.available === true,
    ...(typeof r.name === 'string' && r.name ? { name: r.name } : {}),
    ...(typeof r.message === 'string' && r.message ? { message: r.message } : {}),
  };
}

/** The contract's §6 transport over the sending device's app-phone rpc. */
export function appSenderTransport(rpc: AppSenderRpc, phoneId: () => string): HidTransport & { status(): Promise<SenderStatus> } {
  const call = async (method: string, params: Record<string, unknown>, timeoutMs = HID_TIMEOUT_MS): Promise<Record<string, unknown>> => {
    let res: AppSenderRpcResult;
    try {
      res = await rpc(phoneId(), method, params, timeoutMs);
    } catch (error) {
      if (error instanceof AppSenderError) throw error;
      throw new AppSenderError('device_offline', `Allternit Desktop's local API didn't answer (${error instanceof Error ? error.message : String(error)}).`);
    }
    if (!res.ok) {
      const e = res as Extract<AppSenderRpcResult, { ok: false }>;
      throw new AppSenderError(e.code, e.message);
    }
    return (res as Extract<AppSenderRpcResult, { ok: true }>).result;
  };
  return {
    status: async () => readSenderStatus(await call('hid.pair.status', {}, STATUS_TIMEOUT_MS)),
    pointerMove: async (dx, dy) => void (await call('hid.pointer.move', { dx, dy })),
    pointerClick: async (button, down = true, up = true) => void (await call('hid.pointer.click', { button, down, up })),
    pointerReset: async () => void (await call('hid.pointer.reset', {}, RESET_TIMEOUT_MS)),
    key: async (usage, modifiers, down) => void (await call('hid.key', { usage, modifiers, down })),
    typeText: async (text) => {
      for (let i = 0; i < text.length; i += TYPE_CHUNK) await call('hid.type', { text: text.slice(i, i + TYPE_CHUNK) });
    },
    scroll: async (lines) => void (await call('hid.scroll', { lines })),
  };
}

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

/** What one target iPhone's lane state gives the shared act. */
export interface SenderActTarget {
  core: HidCore;
  /** Pixels of the latest still → screen fractions (throws HidError / AppSenderError). */
  fraction(x: unknown, y: unknown): Promise<Point>;
  /** Calibrates on first pointer use; a result = stop with it (ASSISTIVETOUCH_OFF). */
  calibrated(): Promise<ToolResult | null>;
}

const SWIPES: Record<string, Point[]> = {
  up: [[0.5, 0.7], [0.5, 0.3]],
  down: [[0.5, 0.3], [0.5, 0.7]],
  left: [[0.8, 0.5], [0.2, 0.5]],
  right: [[0.2, 0.5], [0.8, 0.5]],
};

/** One tap tool through the keyboard/mouse core. The lane has already found its sender. */
export async function senderAct(route: RouteId, tool: LaneTool, a: Record<string, unknown>, t: SenderActTarget): Promise<ToolResult> {
  try {
    switch (tool) {
      case 'phone.tap': {
        const p = await t.fraction(a.x, a.y);
        const blocked = await t.calibrated();
        if (blocked) return blocked;
        await t.core.tap(p[0], p[1]);
        return { ok: true };
      }
      case 'phone.swipe': {
        let pts: Point[];
        if (a.direction !== undefined) {
          const d = SWIPES[String(a.direction)];
          if (!d) return fail('invalid_args', 'direction must be up, down, left or right.');
          pts = d;
        } else {
          pts = [await t.fraction(a.x1, a.y1), await t.fraction(a.x2, a.y2)];
        }
        const blocked = await t.calibrated();
        if (blocked) return blocked;
        await t.core.drag(pts);
        return { ok: true };
      }
      case 'phone.type':
        if (typeof a.text !== 'string' || !a.text.length || a.text.length > 500) return fail('invalid_args', 'text must be 1–500 characters.');
        await t.core.type(a.text);
        return { ok: true };
      case 'phone.key':
        if (typeof a.combo !== 'string') return fail('invalid_args', 'combo is required, e.g. "enter" or "cmd+space".');
        await t.core.key(a.combo);
        return { ok: true };
      case 'phone.home':
        await t.core.pressHome();
        return { ok: true };
      case 'phone.back': {
        const blocked = await t.calibrated();
        if (blocked) return blocked;
        await t.core.goBack();
        return { ok: true };
      }
      case 'phone.open_app':
        if (typeof a.app !== 'string' || !a.app.trim() || a.app.length > 60) return fail('invalid_args', 'app must be an app name, e.g. "Notes".');
        await t.core.openApp(a.app.trim());
        return { ok: true };
      default:
        return fail('not_supported', `${tool} does not run on the ${route} route.`);
    }
  } catch (error) {
    return senderFailure(error);
  }
}

/** A thrown failure → the tool result; a keyboard with no connected iPhone is BT_NOT_PAIRED. */
export function senderFailure(error: unknown): ToolResult {
  if (error instanceof HidError) {
    const map: Record<HidError['code'], ToolErrorCode> = { bad_params: 'invalid_args', not_calibrated: 'not_calibrated', unsupported: 'unsupported' };
    return fail(map[error.code], error.message);
  }
  if (error instanceof AppSenderError) {
    if (error.code === 'bt_not_paired') {
      const need = needsYou('BT_NOT_PAIRED');
      return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
    }
    const map: Record<string, ToolErrorCode> = {
      bad_params: 'invalid_args',
      invalid_params: 'invalid_args',
      app_not_open: 'device_offline',
      app_timeout: 'device_offline',
      device_offline: 'device_offline',
      no_screen: 'no_screen',
      route_unavailable: 'route_unavailable',
      hid_unavailable: 'route_unavailable',
      unknown_method: 'not_supported',
    };
    return fail(map[error.code] ?? 'phone_failed', error.message);
  }
  return fail('phone_failed', error instanceof Error ? error.message : String(error));
}

/** The AssistiveTouch pointer was looked for and not seen. */
export function pointerMissing(): ToolResult {
  const need = needsYou('ASSISTIVETOUCH_OFF');
  return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
}
