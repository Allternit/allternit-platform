/**
 * The "waiting for approval" notice on the driven Android phone.
 *
 * While a hold is open, the Phone Link helper on the linked phone shows an
 * ongoing notice so the person holding the phone knows why the bot stopped.
 * One additive helper call (iCTRL JSON-RPC), `hold_notice`:
 *
 *   { state: 'waiting', effect, app?, expiresAt }   a hold was made
 *   { state: 'cleared' }                            no hold is open any more
 *
 * An older helper answers "method not found"; every failure here is ignored,
 * because the notice is a courtesy and never gates the hold. Only Phone-Linked
 * Android phones (`link:` serials) get it; iPhones and adb-only phones have no
 * helper to show it.
 *
 * A hold closes when the runtime says it is no longer `requested` (approved,
 * denied, expired, changed, run ended), when its expiry time passes, or when
 * the runtime cannot be read for a while (guard unavailable). The helper also
 * removes the notice by itself at the expiry time, and when the link service stops.
 */
import type { PhoneHoldNotice } from './phone-tools.js';
import type { PhoneHoldClient } from './phone-hold-client.js';

export const HOLD_NOTICE_POLL_MS = 3_000;
/** Consecutive unreadable polls after which the hold is treated as closed (the runtime is unavailable). */
export const HOLD_NOTICE_MAX_UNREADABLE = 3;
const FALLBACK_MS = 10 * 60_000;

export interface HoldNoticeDeps {
  client: PhoneHoldClient;
  /** True for a Phone-Linked Android phone. Anything else gets no notice. */
  isAndroidLink: (serial: string) => boolean;
  /** One raw helper call. Rejects on any failure. */
  helperCall: (serial: string, method: string, params: Record<string, unknown>, timeoutMs?: number) => Promise<unknown>;
  pollMs?: number;
}

export function holdNoticeSender(deps: HoldNoticeDeps): (notice: PhoneHoldNotice) => void {
  const pollMs = deps.pollMs ?? HOLD_NOTICE_POLL_MS;
  /** serial → open hold ids → stop function. One notice per phone, cleared when its last hold closes. */
  const open = new Map<string, Map<string, () => void>>();
  const send = (serial: string, params: Record<string, unknown>) => {
    void deps.helperCall(serial, 'hold_notice', params, 4_000).catch(() => undefined);
  };
  return (notice) => {
    const { serial, approvalId } = notice;
    if (!deps.isAndroidLink(serial)) return;
    const holds = open.get(serial) ?? new Map<string, () => void>();
    // A retry of a hold that is still waiting sends nothing again.
    if (holds.has(approvalId)) return;
    open.set(serial, holds);
    let unreadable = 0;
    let done = false;
    const close = () => {
      if (done) return;
      done = true;
      clearInterval(poll);
      clearTimeout(expiry);
      holds.delete(approvalId);
      if (holds.size === 0) {
        open.delete(serial);
        send(serial, { state: 'cleared' });
      }
    };
    const poll = setInterval(() => {
      void deps.client.getHold(approvalId).then(
        (hold) => {
          if (done) return;
          if (hold && hold.state !== 'requested') return close();
          unreadable = hold ? 0 : unreadable + 1;
          if (unreadable >= HOLD_NOTICE_MAX_UNREADABLE) close();
        },
        () => {
          if (!done && ++unreadable >= HOLD_NOTICE_MAX_UNREADABLE) close();
        },
      );
    }, pollMs);
    const left = Date.parse(notice.expiresAt) - Date.now();
    const expiry = setTimeout(close, Number.isFinite(left) && left > 0 ? left : FALLBACK_MS);
    holds.set(approvalId, close);
    send(serial, { state: 'waiting', effect: notice.effect, ...(notice.app ? { app: notice.app } : {}), expiresAt: notice.expiresAt });
  };
}
