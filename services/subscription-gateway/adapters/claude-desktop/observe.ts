// Pure page observation: HTML string -> PageState. Same code path for live CDP pages and offline fixtures.
import { allElements, nameOf, parseHtml, queryAll, query, textOf, type El } from "./minidom.js";
import { NAMES, PATTERNS, SELECTORS, nameRe } from "./selectors.js";

export interface Turn { role: "user" | "assistant"; text: string; contentMissing: boolean }
export interface ApprovalCard { id: string; text: string }
export type PageKind = "ok" | "unreachable" | "logged_out" | "rate_limited" | "blocked" | "drift";
export interface PageState {
  kind: PageKind;
  detail: string;
  retryAfterMs?: number;
  composer: boolean;
  streaming: boolean;
  /** The Cowork (agent task) view is the active mode */
  cowork: boolean;
  turns: Turn[];
  approvals: ApprovalCard[];
  toolCues: string[];
  artifactCues: string[];
  /** selector keys that failed to resolve (drift evidence) */
  missing: string[];
}

export const APPROVAL_ID_PREFIX = "ap-";
export function deriveApprovalId(text: string): string {
  let h = 5381; for (const c of text.replace(/\s+/g, " ").trim()) h = ((h * 33) ^ c.charCodeAt(0)) >>> 0;
  return APPROVAL_ID_PREFIX + h.toString(36);
}
function firstMatch(root: El, key: string): El | null {
  for (const css of SELECTORS[key].css) { const el = query(root, css); if (el) return el; }
  return null;
}
function allMatches(root: El, key: string): El[] {
  for (const css of SELECTORS[key].css) { const els = queryAll(root, css); if (els.length) return els; }
  return [];
}
const buttons = (root: El) => allElements(root).filter((e) => e.tag === "button" || e.tag === "a" || e.attrs.role === "button" || e.attrs.role === "tab");

function findApprovals(root: El): ApprovalCard[] {
  const out: ApprovalCard[] = []; const seen = new Set<El>();
  for (const b of buttons(root)) {
    if (!nameRe("approve").test(nameOf(b).trim())) continue;
    // nearest ancestor whose text carries the approval wording
    for (let p = b.parent; p && p.tag !== "#root" && p.tag !== "body"; p = p.parent) {
      const t = textOf(p);
      if (PATTERNS.approval.test(t)) {
        if (!seen.has(p)) { seen.add(p); out.push({ id: p.attrs["data-approval-id"] ?? deriveApprovalId(t.replace(/(allow once|always allow|allow always|allow|approve|deny|don.t allow|decline)/gi, "").trim()), text: t }); }
        break;
      }
    }
  }
  return out;
}

/** "resets in 2 hours" / "resets at 3:00 PM" -> ms from now (bounded 1 min..24 h), or undefined. */
export function retryHint(text: string, now: number): number | undefined {
  const a = PATTERNS.retryIn.exec(text);
  if (a) return Math.min(Math.max(Number(a[1]) * (/^h/i.test(a[2]) ? 3_600_000 : 60_000), 60_000), 86_400_000);
  const b = PATTERNS.retryAt.exec(text);
  if (b) {
    const d = new Date(now); let h = Number(b[1]) % 12; if (/pm/i.test(b[3])) h += 12;
    d.setHours(h, Number(b[2] ?? 0), 0, 0);
    let ms = d.getTime() - now; if (ms <= 0) ms += 86_400_000;
    return Math.min(Math.max(ms, 60_000), 86_400_000);
  }
  return undefined;
}

export function classify(html: string, now: number = Date.now()): PageState {
  const base: PageState = { kind: "ok", detail: "", composer: false, streaming: false, cowork: false, turns: [], approvals: [], toolCues: [], artifactCues: [], missing: [] };
  if (!html || !html.trim()) return { ...base, kind: "unreachable", detail: "empty page" };
  const root = parseHtml(html);
  if (!firstMatch(root, "appRoot")) return { ...base, kind: "unreachable", detail: "page root not present (app still booting or not on claude.ai)" };

  const composer = !!firstMatch(root, "composer");
  const alerts = allMatches(root, "alert").map(textOf).join(" | ");
  const s: PageState = { ...base, composer };
  if (PATTERNS.botCheck.test(alerts)) return { ...s, kind: "blocked", detail: alerts };
  if (PATTERNS.rateLimit.test(alerts)) {
    const usage = /usage limit|limit reached|out of (free )?messages/i.test(alerts);
    return { ...s, kind: "rate_limited", detail: alerts, retryAfterMs: retryHint(alerts, now) ?? (usage ? 3_600_000 : 60_000) };
  }
  if (!composer) {
    if (buttons(root).some((b) => nameRe("signIn").test(nameOf(b).trim()))) return { ...s, kind: "logged_out", detail: "Sign in gate visible" };
    return { ...s, kind: "drift", detail: "composer not found by any selector strategy", missing: ["composer"] };
  }
  s.cowork = buttons(root).some((b) => nameRe("cowork").test(nameOf(b).trim()) && (b.attrs["aria-selected"] === "true" || b.attrs["aria-current"] === "page" || b.attrs["aria-pressed"] === "true")) || !!firstMatch(root, "coworkTask");
  s.streaming = buttons(root).some((b) => nameRe("stop").test(nameOf(b).trim())) || !!firstMatch(root, "streamingMarker");
  for (const t of allMatches(root, "turn")) {
    const role = t.attrs["data-testid"] === "user-message" ? "user" : "assistant";
    const content = role === "user" ? t : SELECTORS.turnContent.css.map((c) => query(t, c)).find(Boolean) ?? null;
    s.turns.push({ role, text: textOf(content ?? t), contentMissing: !content });
  }
  // An assistant turn with text but no markdown node (and not streaming) means the content hook moved. Streaming/tool-only turns are fine.
  if (!s.streaming && s.turns.some((t) => t.role === "assistant" && t.contentMissing && t.text)) return { ...s, kind: "drift", detail: "assistant turn without markdown content node (.standard-markdown)", missing: ["turnContent"] };
  s.approvals = findApprovals(root);
  s.toolCues = allMatches(root, "toolCue").map(textOf).filter(Boolean);
  s.artifactCues = allMatches(root, "artifactCue").map(textOf).filter(Boolean);
  return s;
}
export { NAMES };
