/**
 * The runtime's phone approval guard, seen from Desktop (contract: "Phone
 * approval holds"). Desktop's phone dispatch enforces; allternit-api decides.
 *
 * - `check`   POST /api/v1/phones/guard/check  → allow | hold | denied
 * - `getHold` GET  /api/v1/phones/holds/:id
 * - `resolve` POST /api/v1/inbox/approval:<id>/approve {decision} — the route
 *   "Needs you" uses, answered as the signed-in person (never a bot).
 *
 * Every call has a short timeout. A failure is a typed answer, never a throw,
 * so the dispatcher can refuse an acting call (fail closed) instead of crashing.
 */

/** The element a tap or type lands on, as the guard's rules read it. */
export interface GuardElement {
  class: string;
  label: string;
  id: string;
}

/** Who the action runs for (TaskCtx on the runtime; `owner` is set there from the session). */
export interface GuardCtx {
  bot_id?: string;
  thread_id?: string;
  session_id?: string;
  source?: string;
}

/** The body of POST /phones/guard/check (CheckIn, camelCase). */
export interface GuardCheckBody {
  serial: string;
  tool: string;
  /** The tool args exactly as received (a pass is a hash over tool + args). */
  args: Record<string, unknown>;
  el?: GuardElement;
  app?: string;
  ctx?: GuardCtx;
  holdId?: string;
  runId?: string;
  phoneName?: string;
  platform?: 'android' | 'ios';
  sensitive?: boolean;
  pass?: string;
  /** Data URL or base64 of the screen, kept on the runtime as the hold's picture. */
  screenshot?: string;
}

export type GuardAnswer =
  | { decision: 'allow'; holdId: string | null }
  | { decision: 'hold'; approvalId: string; inboxId: string; effect: string; expiresAt: string }
  | { decision: 'denied'; approvalId: string }
  /** No runtime, signed out, timeout or an error answer: acting calls are refused. */
  | { decision: 'unavailable'; message: string };

export interface HoldView {
  approvalId: string;
  inboxId: string;
  /** requested | approved | denied | expired | changed */
  state: string;
  consumed: boolean;
  effect: string;
  phone: Record<string, unknown>;
  expiresAt: string;
}

export interface PhoneHoldClient {
  check(body: GuardCheckBody): Promise<GuardAnswer>;
  /** null when the hold is unknown or the runtime can't be reached. */
  getHold(id: string): Promise<HoldView | null>;
  /** true when the runtime took the answer (a hold already answered elsewhere is false). */
  resolve(id: string, decision: 'approve' | 'deny'): Promise<boolean>;
}

export const GUARD_CHECK_TIMEOUT_MS = 8_000;
export const HOLD_READ_TIMEOUT_MS = 5_000;
export const HOLD_RESOLVE_TIMEOUT_MS = 10_000;

export interface HttpPhoneHoldDeps {
  /** allternit-api base URL (no /api/v1). */
  base: () => string;
  /** Runtime credential headers for the signed-in person; null when nobody is signed in. */
  headers: () => Record<string, string> | null;
  fetchImpl?: typeof fetch;
}

const str = (v: unknown): string => (typeof v === 'string' ? v : '');

export function httpPhoneHoldClient(deps: HttpPhoneHoldDeps): PhoneHoldClient {
  const fetchImpl = deps.fetchImpl ?? fetch;
  const send = async (method: 'GET' | 'POST', path: string, body: unknown, timeoutMs: number): Promise<{ status: number; json: Record<string, unknown> } | { error: string }> => {
    const headers = deps.headers();
    if (!headers) return { error: 'Sign in to Allternit Desktop so your phone actions can ask for your approval.' };
    try {
      const res = await fetchImpl(`${deps.base().replace(/\/$/, '')}/api/v1${path}`, {
        method,
        headers: { ...headers, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs),
      });
      const json = (await res.json().catch(() => ({}))) as Record<string, unknown>;
      return { status: res.status, json: json && typeof json === 'object' ? json : {} };
    } catch (error) {
      return { error: `Allternit Desktop's local API did not answer (${error instanceof Error ? error.message : String(error)}).` };
    }
  };
  const enc = encodeURIComponent;
  return {
    async check(body) {
      const r = await send('POST', '/phones/guard/check', body, GUARD_CHECK_TIMEOUT_MS);
      if ('error' in r) return { decision: 'unavailable', message: r.error };
      if (r.status < 200 || r.status >= 300) {
        return { decision: 'unavailable', message: str(r.json.message) || `The approval check answered ${r.status}.` };
      }
      const j = r.json;
      if (j.decision === 'allow') return { decision: 'allow', holdId: str(j.holdId) || null };
      if (j.decision === 'hold' && str(j.approvalId)) {
        return { decision: 'hold', approvalId: str(j.approvalId), inboxId: str(j.inboxId) || `approval:${str(j.approvalId)}`, effect: str(j.effect), expiresAt: str(j.expiresAt) };
      }
      if (j.decision === 'denied' && str(j.approvalId)) return { decision: 'denied', approvalId: str(j.approvalId) };
      return { decision: 'unavailable', message: 'The approval check gave an answer Desktop does not understand.' };
    },
    async getHold(id) {
      const r = await send('GET', `/phones/holds/${enc(id)}`, undefined, HOLD_READ_TIMEOUT_MS);
      if ('error' in r || r.status !== 200) return null;
      const j = r.json;
      return {
        approvalId: str(j.approvalId) || id,
        inboxId: str(j.inboxId) || `approval:${id}`,
        state: str(j.state),
        consumed: j.consumed === true,
        effect: str(j.effect),
        phone: j.phone && typeof j.phone === 'object' ? (j.phone as Record<string, unknown>) : {},
        expiresAt: str(j.expiresAt),
      };
    },
    async resolve(id, decision) {
      const r = await send('POST', `/inbox/${enc(`approval:${id}`)}/approve`, { decision }, HOLD_RESOLVE_TIMEOUT_MS);
      return !('error' in r) && r.status >= 200 && r.status < 300;
    },
  };
}
