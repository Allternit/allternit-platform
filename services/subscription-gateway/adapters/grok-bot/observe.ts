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
  /** New-chat Bot picker is showing (a Bot must be chosen before the composer works) */
  picker: boolean;
  turns: Turn[];
  approvals: ApprovalCard[];
  routineCues: string[];
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
const buttons = (root: El) => allElements(root).filter((e) => e.tag === "button" || e.attrs.role === "button");

function findApprovals(root: El): ApprovalCard[] {
  const out: ApprovalCard[] = []; const seen = new Set<El>();
  for (const b of buttons(root)) {
    if (!nameRe("approve").test(nameOf(b).trim())) continue;
    // nearest ancestor whose text carries the approval wording
    for (let p = b.parent; p && p.tag !== "#root" && p.tag !== "body"; p = p.parent) {
      const t = textOf(p);
      if (PATTERNS.approval.test(t)) {
        if (!seen.has(p)) { seen.add(p); out.push({ id: p.attrs["data-approval-id"] ?? deriveApprovalId(t.replace(/(allow once|always allow|allow|approve|deny|skip)/gi, "").trim()), text: t }); }
        break;
      }
    }
  }
  return out;
}

/** Bot names listed in the New-chat picker: buttons under the smallest ancestor holding both the close and create controls. */
export function pickerBots(html: string): string[] {
  const root = parseHtml(html);
  const close = buttons(root).find((b) => nameRe("closePicker").test(nameOf(b).trim()));
  if (!close) return [];
  const isCreate = (b: El) => /^create new bot$/i.test(nameOf(b).trim());
  let scope: El | null = close.parent;
  while (scope && !buttons(scope).some(isCreate)) scope = scope.parent;
  if (!scope) scope = close.parent;
  const skip = new RegExp(NAMES.pickerControls, "i");
  const names = buttons(scope ?? root).map((b) => nameOf(b).trim()).filter((n) => n && !skip.test(n));
  return [...new Set(names)];
}

export function classify(html: string): PageState {
  const base: PageState = { kind: "ok", detail: "", composer: false, streaming: false, picker: false, turns: [], approvals: [], routineCues: [], missing: [] };
  if (!html || !html.trim()) return { ...base, kind: "unreachable", detail: "empty page" };
  const root = parseHtml(html);
  if (!firstMatch(root, "appRoot")) return { ...base, kind: "unreachable", detail: "renderer root not present (app still booting?)" };

  const composer = !!firstMatch(root, "composer");
  const alerts = allMatches(root, "alert").map(textOf).join(" | ");
  const s: PageState = { ...base, composer };
  if (PATTERNS.botCheck.test(alerts)) return { ...s, kind: "blocked", detail: alerts };
  if (PATTERNS.rateLimit.test(alerts)) {
    const usage = /usage limit|limit reached/i.test(alerts);
    return { ...s, kind: "rate_limited", detail: alerts, retryAfterMs: usage ? 3_600_000 : 60_000 };
  }
  if (!composer) {
    if (buttons(root).some((b) => nameRe("signIn").test(nameOf(b).trim()))) return { ...s, kind: "logged_out", detail: "Sign in gate visible" };
    return { ...s, kind: "drift", detail: "composer not found by any selector strategy", missing: ["composer"] };
  }
  s.picker = buttons(root).some((b) => nameRe("closePicker").test(nameOf(b).trim()));
  s.streaming = buttons(root).some((b) => nameRe("stop").test(nameOf(b).trim()));
  for (const t of allMatches(root, "turn")) {
    const role = t.attrs["data-role"] === "user" ? "user" : "assistant";
    const content = SELECTORS.turnContent.css.map((c) => query(t, c)).find(Boolean) ?? null;
    s.turns.push({ role, text: textOf(content ?? t), contentMissing: !content });
  }
  if (s.turns.some((t) => t.role === "assistant" && t.contentMissing)) return { ...s, kind: "drift", detail: "assistant turn without message-content node (.sand-message-prose)", missing: ["turnContent"] };
  s.approvals = findApprovals(root);
  s.routineCues = allMatches(root, "routineCue").map(textOf).filter(Boolean);
  return s;
}
export { NAMES };
