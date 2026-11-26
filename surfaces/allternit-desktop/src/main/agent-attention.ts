/**
 * Pure helpers for the agent-browser "needs you" flow on this Mac: the words
 * of each native notification, validation of what the agent cursor page and
 * the Chrome extension bridge hand to main, and the Safari agent profile card
 * copy. No Electron here, so it is unit-testable.
 *
 * Copy is fixed by the agent-browser contract (2026-11-26):
 *   "Fill your <name> login on <domain>?", "<bot> needs you", "<domain> blocked <bot>".
 * A notice never carries a credential value, and approving happens in the app,
 * never in the notification.
 */

export type AttentionKind = 'credential' | 'human' | 'blocked';

export interface AttentionInput {
  kind: AttentionKind;
  runId?: string;
  computerId?: string;
  botName?: string;
  /** Credential name for `credential` (e.g. "Gmail"). */
  name?: string;
  domain?: string;
  /** Extension handoff id, when the pause came from the Chrome extension. */
  handoffId?: string;
}

export interface AttentionNotice {
  title: string;
  body: string;
  /** In-app route that opens the run. Null when there is nothing to open. */
  route: string | null;
}

const MAX_TEXT = 80;
const ID_RE = /^[A-Za-z0-9._:-]{1,128}$/;

/** One short printable line, or undefined. Control characters and long text are dropped. */
export function cleanText(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined;
  const text = value.replace(/[\u0000-\u001f\u007f]/g, ' ').replace(/\s+/g, ' ').trim();
  return text ? text.slice(0, MAX_TEXT) : undefined;
}

function cleanId(value: unknown): string | undefined {
  return typeof value === 'string' && ID_RE.test(value) ? value : undefined;
}

/** Host only: a full URL becomes its hostname, a bare domain stays. */
export function cleanDomain(value: unknown): string | undefined {
  const text = cleanText(value);
  if (!text) return undefined;
  try {
    return new URL(text.includes('://') ? text : `https://${text}`).hostname || undefined;
  } catch {
    return undefined;
  }
}

/** Validate a message from the cursor page. Unknown shapes return null. */
export function parseAttentionInput(raw: unknown): AttentionInput | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  if (r.kind !== 'credential' && r.kind !== 'human' && r.kind !== 'blocked') return null;
  return {
    kind: r.kind,
    runId: cleanId(r.runId),
    computerId: cleanId(r.computerId),
    botName: cleanText(r.botName),
    name: cleanText(r.name),
    domain: cleanDomain(r.domain),
    handoffId: cleanId(r.handoffId),
  };
}

/** `/computers/<id>?run=<run>` — the same deep link the push uses. */
export function runRoute(computerId?: string, runId?: string): string | null {
  if (!computerId) return null;
  return `/computers/${encodeURIComponent(computerId)}${runId ? `?run=${encodeURIComponent(runId)}` : ''}`;
}

export function attentionNotice(input: AttentionInput): AttentionNotice {
  const bot = input.botName ?? 'Your bot';
  const domain = input.domain;
  let title: string;
  let body: string;
  if (input.kind === 'credential') {
    title = `Fill your ${input.name ?? 'saved'} login${domain ? ` on ${domain}` : ''}?`;
    body = `${bot} wants to sign in. Open Allternit to approve or deny.`;
  } else if (input.kind === 'blocked') {
    title = `${domain ?? 'A site'} blocked ${bot}`;
    body = 'Open Allternit to choose what happens next.';
  } else {
    title = `${bot} needs you`;
    body = 'Open Allternit to take over or let it continue.';
  }
  return { title, body, route: runRoute(input.computerId, input.runId) };
}

/** What the Chrome extension sends when a task pauses for the person. */
export interface ExtensionNeedsYou {
  kind: AttentionKind;
  url?: string;
  domain?: string;
  handoffId: string;
  botName?: string;
}

/** `{type:"needs_you", payload:{kind,url,domain,handoffId}}`; anything else is null. */
export function parseNeedsYou(message: unknown): ExtensionNeedsYou | null {
  if (!message || typeof message !== 'object') return null;
  const m = message as { type?: unknown; payload?: unknown };
  if (m.type !== 'needs_you' || !m.payload || typeof m.payload !== 'object') return null;
  const p = m.payload as Record<string, unknown>;
  const handoffId = cleanId(p.handoffId);
  if (!handoffId) return null;
  const kindText = typeof p.kind === 'string' ? p.kind : '';
  const kind: AttentionKind = kindText === 'credential' ? 'credential' : kindText === 'blocked' || kindText === 'block' ? 'blocked' : 'human';
  const url = cleanText(p.url);
  return { kind, url, domain: cleanDomain(p.domain) ?? cleanDomain(url), handoffId, botName: cleanText(p.botName) };
}

/** `{type:"resume_task", payload:{handoffId, approved}}` or null for a bad handoff id. */
export function resumeTaskMessage(handoffId: unknown, approved: unknown): { type: 'resume_task'; payload: { handoffId: string; approved: boolean } } | null {
  const id = cleanId(handoffId);
  if (!id || typeof approved !== 'boolean') return null;
  return { type: 'resume_task', payload: { handoffId: id, approved } };
}

export const SAFARI_PROFILE_NAME = 'Allternit Agent';

export const SAFARI_PROFILE_STEPS = [
  'Open Safari, then choose Safari → Settings → Profiles.',
  'Click + to add a profile.',
  `Name it "${SAFARI_PROFILE_NAME}" and click Done.`,
] as const;

export function safariProfileCardText(): { message: string; detail: string } {
  return {
    message: 'Set up a Safari profile for your bots',
    detail: [
      'Agents browse in their own Safari profile, so they never use your logins or history.',
      '',
      ...SAFARI_PROFILE_STEPS.map((step, i) => `${i + 1}. ${step}`),
    ].join('\n'),
  };
}
