// Pure page observation: HTML string -> PageState. Same code path for the live browser page and offline fixtures.
// The tiny DOM engine is shared with grok-bot (adapters/grok-bot/minidom.ts): one parser, two adapters.
import { allElements, nameOf, parseHtml, queryAll, query, textOf, type El } from "../grok-bot/minidom.js";
import { NAMES, PATTERNS, SELECTORS, TASK_SECTIONS, nameRe } from "./selectors.js";

export interface Turn { role: "user" | "assistant"; text: string; contentMissing: boolean }
export type Policy = "ask_first" | "hand_off";
export interface Confirmation { id: string; policy: Policy; text: string }
export interface DotRef { id: string; name: string; handle?: string; avatar?: string }
export interface DotTask { id: string; title: string; state: "in_progress" | "scheduled" | "completed" }
export type PageKind = "ok" | "unreachable" | "logged_out" | "rate_limited" | "blocked" | "paused" | "drift";
export interface PageState {
  kind: PageKind;
  detail: string;
  retryAfterMs?: number;
  view: "list" | "dot";
  composer: boolean;
  streaming: boolean;
  dots: DotRef[];
  header?: { name: string; handle?: string; avatar?: string };
  turns: Turn[];
  activity?: string;
  confirmations: Confirmation[];
  tasks: DotTask[];
  /** selector keys that failed to resolve (drift evidence) */
  missing: string[];
}

export const CONFIRMATION_ID_PREFIX = "dc-";
export function deriveId(prefix: string, text: string): string {
  let h = 5381; for (const c of text.replace(/\s+/g, " ").trim()) h = ((h * 33) ^ c.charCodeAt(0)) >>> 0;
  return prefix + h.toString(36);
}
function firstMatch(root: El, key: string): El | null {
  for (const css of SELECTORS[key].css) { const el = query(root, css); if (el) return el; }
  return null;
}
function allMatches(root: El, key: string): El[] {
  for (const css of SELECTORS[key].css) { const els = queryAll(root, css); if (els.length) return els; }
  return [];
}
const buttons = (root: El) => allElements(root).filter((e) => e.tag === "button" || e.attrs.role === "button");
const hasButton = (root: El, k: keyof typeof NAMES) => buttons(root).some((b) => nameRe(k).test(nameOf(b).trim()));

function findConfirmations(root: El): Confirmation[] {
  const out: Confirmation[] = [];
  for (const card of allMatches(root, "confirmation")) {
    const full = textOf(card);
    const policy: Policy | undefined = card.attrs["data-policy"] === "hand_off" || PATTERNS.handOff.test(full) ? "hand_off"
      : card.attrs["data-policy"] === "ask_first" || PATTERNS.askFirst.test(full) ? "ask_first" : undefined;
    if (!policy) continue;
    const label = policy === "hand_off" ? PATTERNS.handOff : PATTERNS.askFirst;
    const parts = card.children.flatMap((ch) => (typeof ch === "string" ? [ch] : ch.tag === "button" || ch.attrs.role === "button" ? [] : [textOf(ch)]));
    const body = parts.map((t) => t.trim()).filter((t) => t && !new RegExp(label.source + "$", "i").test(t)).join(" ").replace(/\s+/g, " ").trim()
      || full.replace(label, "").replace(/\s+/g, " ").trim();
    out.push({ id: card.attrs["data-confirmation-id"] ?? deriveId(CONFIRMATION_ID_PREFIX, `${policy}|${body}`), policy, text: body });
  }
  return out;
}

function findTasks(root: El): DotTask[] {
  const panel = firstMatch(root, "tasksPanel");
  if (!panel) return [];
  const out: DotTask[] = [];
  let state: DotTask["state"] | undefined;
  const walk = (el: El) => {
    for (const ch of el.children) {
      if (typeof ch === "string") continue;
      if (/^h[1-6]$/.test(ch.tag)) { state = TASK_SECTIONS[textOf(ch).toLowerCase().trim()] ?? state; continue; }
      if (state && (ch.tag === "li" || ch.attrs["data-testid"] === "dot-task")) { const title = textOf(ch); if (title) out.push({ id: deriveId("dt-", title), title, state }); continue; }
      walk(ch);
    }
  };
  walk(panel);
  return out;
}

function findDots(root: El): DotRef[] {
  const out: DotRef[] = [];
  for (const row of allMatches(root, "dotRow")) {
    const href = row.attrs.href ?? query(row, "a")?.attrs.href ?? "";
    const id = row.attrs["data-dot-id"] ?? decodeURIComponent(href.split("/").filter(Boolean).pop() ?? "");
    if (!id) continue;
    const nameEl = SELECTORS.dotName.css.map((c) => query(row, c)).find(Boolean);
    const handleEl = SELECTORS.dotHandle.css.map((c) => query(row, c)).find(Boolean);
    const img = SELECTORS.dotAvatar.css.map((c) => query(row, c)).find(Boolean);
    out.push({ id, name: (nameEl ? textOf(nameEl) : textOf(row)).trim() || id, handle: handleEl ? textOf(handleEl) : undefined, avatar: img?.attrs.src });
  }
  return out;
}

export function classify(html: string): PageState {
  const base: PageState = { kind: "ok", detail: "", view: "dot", composer: false, streaming: false, dots: [], turns: [], confirmations: [], tasks: [], missing: [] };
  if (!html || !html.trim()) return { ...base, kind: "unreachable", detail: "empty page" };
  const root = parseHtml(html);
  if (!firstMatch(root, "appRoot")) return { ...base, kind: "unreachable", detail: "app root not present (page still loading?)" };

  const composer = !!firstMatch(root, "composer");
  const s: PageState = { ...base, composer };
  const alerts = allMatches(root, "alert").map(textOf).join(" | ");
  const text = textOf(root);
  if (PATTERNS.challenge.test(alerts) || queryAll(root, "iframe[src*='challenges.cloudflare.com']").length || PATTERNS.challenge.test(text) && !composer) return { ...s, kind: "blocked", detail: alerts || "verification challenge" };
  if (PATTERNS.rateLimit.test(alerts)) return { ...s, kind: "rate_limited", detail: alerts, retryAfterMs: /resets? (at|in)/i.test(alerts) ? 3_600_000 : 60_000 };
  if (PATTERNS.paused.test(alerts)) return { ...s, kind: "paused", detail: alerts };

  s.dots = findDots(root);
  if (!composer && s.dots.length) return { ...s, view: "list" };
  if (!composer) {
    if (hasButton(root, "signIn") || queryAll(root, "a[href*='/auth/login']").length || /log in to continue/i.test(text)) return { ...s, kind: "logged_out", detail: "Log in gate visible" };
    return { ...s, kind: "drift", detail: "neither the composer nor the dots list was found by any selector strategy", missing: ["composer", "dotRow"] };
  }
  s.streaming = hasButton(root, "stop");
  const header = firstMatch(root, "dotHeader");
  if (header) {
    const n = SELECTORS.dotName.css.concat(["h1"]).map((c) => query(header, c)).find(Boolean);
    const h = SELECTORS.dotHandle.css.map((c) => query(header, c)).find(Boolean);
    const img = SELECTORS.dotAvatar.css.map((c) => query(header, c)).find(Boolean);
    if (n) s.header = { name: textOf(n), handle: h ? textOf(h) : undefined, avatar: img?.attrs.src };
  }
  // Turns in document order: user bubbles and assistant containers.
  const userEls = new Set(allMatches(root, "userTurn"));
  const assistants = allMatches(root, "assistantTurn");
  const order = allElements(root).filter((e) => userEls.has(e) || assistants.includes(e));
  for (const e of order) s.turns.push({ role: userEls.has(e) ? "user" : "assistant", text: textOf(e), contentMissing: false });
  const act = firstMatch(root, "activity");
  if (act) s.activity = textOf(act);
  s.confirmations = findConfirmations(root);
  s.tasks = findTasks(root);
  return s;
}
export { NAMES };
