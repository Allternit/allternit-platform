/**
 * Developer first-run setup — pure logic shared by the setup page and its tests.
 *
 * What is real and where it is stored:
 * - Organization: a real Clerk organization (or the gateway's personal org on
 *   no-Clerk builds). Projects come from /api/v1/platform/projects.
 * - Where workloads run: the console has no single "allowed places" setting
 *   (provider routing is per provider, in Gateway → Routing policy), so this is
 *   saved as a preference on the account (Clerk unsafeMetadata.platformSetup)
 *   and said so in the UI.
 * - API key: a real gateway key (`ak-…`, POST /api/v1/gateway/keys) or a real
 *   scoped key (POST /api/v1/api-keys). The full token lives only in React
 *   state for this page; it is never logged or written to storage.
 * - First call: a real POST /v1/chat/completions with that gateway key, the
 *   same browser path the Playground uses. Its status and reply are shown as
 *   returned; nothing is faked.
 * - Plan: live plans from /api/v1/billing/plans; a paid choice opens the
 *   existing Billing checkout (/billing?plan=…) after setup.
 */

export const API_BASE_URL = "https://api.allternit.com/v1";

export type SetupStep = 0 | 1 | 2 | 3 | 4 | 5 | 6;
export type OrgKind = "personal" | "team";
export type Place = "hw" | "managed" | "cloud";
export type KeyKind = "gateway" | "scoped";
export type Lang = "curl" | "python" | "ts";

export const STEP_LABELS = ["Organization", "Where it runs", "API key", "First call", "Plan"] as const;

/** A:// orb state per step (0 = not started, 6 = connected view). */
export const ORB_STATE_BY_STEP = ["idle", "thinking", "searching", "working", "writing", "thinking", "working"] as const;

/** How many of the 30 progress blocks are lit per step. */
export function litBlocks(step: number): number {
  return [0, 6, 12, 18, 24, 29, 30][step] ?? 30;
}

export function blockStates(step: number, total = 30): Array<"on" | "lead" | ""> {
  const lit = litBlocks(step);
  return Array.from({ length: total }, (_, b) => {
    if (b < lit - 1) return "on";
    if (b === lit - 1) return lit === total ? "on" : "lead";
    return "";
  });
}

export function stepperStates(step: number): Array<{ label: string; state: "done" | "now" | "" }> {
  return STEP_LABELS.map((label, i) => {
    const n = i + 1;
    return { label, state: n < step ? "done" : n === step ? "now" : "" };
  });
}

/** Show the first characters of a key and hide the rest. Never returns the full secret. */
export function maskKey(tokenOrPrefix: string | null | undefined): string {
  if (!tokenOrPrefix) return "••••••••";
  // A server-issued prefix (≤12 chars) is meant to be shown; a full token is cut to 8.
  const visible = tokenOrPrefix.length <= 12 ? tokenOrPrefix : tokenOrPrefix.slice(0, 8);
  return `${visible}…••••`;
}

export interface SnippetOptions {
  lang: Lang;
  model: string;
  /** What to put where the key goes: the real key (copy only) or a masked/placeholder string. */
  key: string;
  baseUrl?: string;
}

/** The first-call snippet, matching the prototype's three languages. */
export function buildSnippet({ lang, model, key, baseUrl = API_BASE_URL }: SnippetOptions): string {
  const m = JSON.stringify(model);
  if (lang === "curl") {
    return [
      `curl ${baseUrl}/chat/completions \\`,
      `  -H "Authorization: Bearer ${key}" \\`,
      `  -H "Content-Type: application/json" \\`,
      `  -d '{"model": ${m},`,
      `       "messages": [{"role": "user", "content": "Hello"}]}'`,
    ].join("\n");
  }
  if (lang === "python") {
    return [
      "from openai import OpenAI",
      "",
      `client = OpenAI(base_url="${baseUrl}",`,
      `                api_key="${key}")`,
      "reply = client.chat.completions.create(",
      `    model=${m},`,
      '    messages=[{"role": "user", "content": "Hello"}])',
      "print(reply.choices[0].message.content)",
    ].join("\n");
  }
  return [
    'import OpenAI from "openai";',
    "",
    `const client = new OpenAI({ baseURL: "${baseUrl}",`,
    `  apiKey: "${key}" });`,
    "const reply = await client.chat.completions.create({",
    `  model: ${m},`,
    '  messages: [{ role: "user", content: "Hello" }] });',
    "console.log(reply.choices[0].message.content);",
  ].join("\n");
}

// ── first-run detection ────────────────────────────────────────────────────

export interface SetupRecord {
  status: "completed" | "skipped";
  at: string;
  places?: Place[];
  plan?: string;
}

export interface FirstRunSignals {
  /** Setup record already on the account (Clerk metadata) or in this browser. */
  record: SetupRecord | null;
  /** Number of organizations the user belongs to; null when unknown. */
  orgCount: number | null;
  /** Gateway key count; null when the list failed. */
  gatewayKeys: number | null;
  /** Scoped key count; null when the list failed. */
  scopedKeys: number | null;
}

/**
 * Setup is offered only when we are sure the account is new: no setup record,
 * no organization, and both key lists loaded empty. Any unknown answers "no",
 * so an existing user is never routed into setup by a failed request.
 */
export function shouldOfferSetup(s: FirstRunSignals): boolean {
  if (s.record) return false;
  if (s.orgCount === null || s.orgCount > 0) return false;
  if (s.gatewayKeys === null || s.scopedKeys === null) return false;
  return s.gatewayKeys === 0 && s.scopedKeys === 0;
}

const LOCAL_PREFIX = "allternit:platform-setup:";

export function readLocalSetupRecord(userId: string | null | undefined): SetupRecord | null {
  if (!userId || typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(LOCAL_PREFIX + userId);
    return raw ? (JSON.parse(raw) as SetupRecord) : null;
  } catch {
    return null;
  }
}

export function writeLocalSetupRecord(userId: string | null | undefined, record: SetupRecord): void {
  if (!userId || typeof window === "undefined") return;
  try {
    window.localStorage.setItem(LOCAL_PREFIX + userId, JSON.stringify(record));
  } catch {
    // Storage unavailable: the Clerk metadata copy still applies.
  }
}

/** Reads `unsafeMetadata.platformSetup` from a Clerk user, if it looks right. */
export function readMetadataRecord(unsafeMetadata: unknown): SetupRecord | null {
  if (!unsafeMetadata || typeof unsafeMetadata !== "object") return null;
  const rec = (unsafeMetadata as Record<string, unknown>).platformSetup;
  if (!rec || typeof rec !== "object") return null;
  const status = (rec as Record<string, unknown>).status;
  if (status !== "completed" && status !== "skipped") return null;
  return rec as SetupRecord;
}

// ── live plans ─────────────────────────────────────────────────────────────

export interface LivePlan {
  id: string;
  label: string;
  price_usd: number;
  monthly_credits_usd: number;
}

export function formatUsd(value: number): string {
  return `$${Number.isInteger(value) ? value.toFixed(0) : value.toFixed(2)}`;
}

/** One-line description of a live plan, built only from the API's numbers. */
export function describePlan(p: LivePlan): string {
  const parts = [`${formatUsd(p.price_usd)}/month`];
  if (p.monthly_credits_usd > 0) parts.push(`${formatUsd(p.monthly_credits_usd)} monthly credits`);
  return parts.join(" · ");
}

/** Plans a new account can pick: paid plans from the API, sorted by price, free tier excluded. */
export function paidPlans(plans: LivePlan[]): LivePlan[] {
  return plans
    .filter((p) => p && typeof p.id === "string" && p.id !== "free" && Number(p.price_usd) > 0)
    .sort((a, b) => a.price_usd - b.price_usd);
}

// ── graph geometry ─────────────────────────────────────────────────────────

export interface GraphEdge {
  x: number;
  y: number;
  len: number;
  angle: number;
}

export function edgeFrom(cx: number, cy: number, x: number, y: number): GraphEdge {
  const dx = x - cx;
  const dy = y - cy;
  return { x: cx, y: cy, len: Math.round(Math.sqrt(dx * dx + dy * dy)), angle: (Math.atan2(dy, dx) * 180) / Math.PI };
}

/** Chebyshev-distance ripple delays for the haptic squares (choice picks only). */
export function rippleDelays(cols: number, rows: number, oc: number, or: number): number[] {
  const out: number[] = [];
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) out.push(Math.max(Math.abs(c - oc), Math.abs(r - or)));
  }
  return out;
}
