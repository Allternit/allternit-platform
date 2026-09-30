// chatgpt-dots selector pack "dots-v1" (OFFLINE, hand-built; nothing here was checked against a live ChatGPT session).
// Shared chat keys are DERIVED from chatgpt-web's live-verified pack (adapters/chatgpt-web/selectors/v1.yaml, checked
// live 2026-09-27) so a ChatGPT UI drift fixed there is fixed here. Dots-specific keys are INFERRED from the launch
// description in Research/memos/2026-09-29-openai-dots.md and are marked with a confidence the README repeats.
//   confidence "live"     = inherited from chatgpt-web, verified against a logged-in ChatGPT UI on 2026-09-27
//   confidence "inferred" = written from the memo / ChatGPT conventions, unseen. First live session must confirm.
//   confidence "guess"    = wording only (button/label text); expect to change.
import { load } from "js-yaml";
import { selectorsYaml } from "../chatgpt-web/adapter.js";

export const SELECTORS_VERSION = "dots-v1";
export type Confidence = "live" | "inferred" | "guess";
export interface KeySpec { critical: boolean; css: string[]; confidence: Confidence }

type Strategy = { css?: string; testid?: string };
const web = load(selectorsYaml()) as Record<string, { strategies: Strategy[] }>;
/** css fallbacks for a chatgpt-web key (testid strategies become [data-testid=..]). */
function shared(key: string): string[] {
  const out = (web[key]?.strategies ?? []).flatMap((s) => (s.css ? [s.css] : s.testid ? [`[data-testid='${s.testid}']`] : []));
  if (!out.length) throw new Error(`chatgpt-web selector key ${key} has no css/testid strategy`);
  return out;
}

export const SELECTORS: Record<string, KeySpec> = {
  // ---- inherited from chatgpt-web ----
  appRoot: { critical: true, css: ["main", "#__next"], confidence: "live" },
  composer: { critical: true, css: [...shared("composer")], confidence: "live" },
  loggedInProbe: { critical: false, css: shared("logged_in_probe"), confidence: "live" },
  assistantTurn: { critical: false, css: shared("response"), confidence: "live" },
  userTurn: { critical: false, css: shared("user_turn"), confidence: "live" },
  alert: { critical: false, css: ["[role=alert]", "[data-testid='limit-banner']"], confidence: "live" },
  challenge: { critical: false, css: shared("challenge").filter((c) => c.startsWith("iframe")), confidence: "live" },
  // ---- dots-specific (inferred) ----
  // Dots list view: one row per dot, link target /dots/<id>. Name/handle/avatar live inside the row.
  dotRow: { critical: false, css: ["[data-testid='dot-row']", "a[href^='/dots/']"], confidence: "inferred" },
  dotName: { critical: false, css: ["[data-testid='dot-name']", "h2", "h3"], confidence: "inferred" },
  dotHandle: { critical: false, css: ["[data-testid='dot-handle']"], confidence: "inferred" },
  dotAvatar: { critical: false, css: ["[data-testid='dot-avatar'] img", "img[alt*='avatar' i]", "img"], confidence: "inferred" },
  // Dot conversation header (identity of the open dot)
  dotHeader: { critical: false, css: ["[data-testid='dot-header']", "header"], confidence: "inferred" },
  // "Ask first" / "Hand off" cards: a rule outcome shown inline in the thread.
  confirmation: { critical: false, css: ["[data-testid='dot-confirmation']", "[role=group][data-policy]"], confidence: "inferred" },
  // Activity View / working indicator
  activity: { critical: false, css: ["[data-testid='dot-activity']", "[data-testid='activity-view']"], confidence: "inferred" },
  // Tasks panel in the dot's profile: sections "In progress", "Scheduled", "Completed"
  tasksPanel: { critical: false, css: ["[data-testid='dot-tasks']", "section[aria-label='Tasks']"], confidence: "inferred" },
  taskRow: { critical: false, css: ["[data-testid='dot-task']", "li"], confidence: "inferred" },
};

/** Accessible-name patterns (English). Strings so the live driver can eval them. confidence per entry is in NAME_CONFIDENCE. */
export const NAMES = {
  send: "^(send|send prompt)$", // live
  stop: "^(stop|stop streaming)$", // live
  signIn: "^(log in|sign in)$", // live (login-button)
  approve: "^(approve|allow|allow once|yes)$", // guess
  deny: "^(deny|decline|not now|no)$", // guess
  tasks: "^(tasks|activity|view tasks)$", // guess
  dotsList: "^(dots|my dots)$", // guess
} as const;
export const NAME_CONFIDENCE: Record<keyof typeof NAMES, Confidence> = { send: "live", stop: "live", signIn: "live", approve: "guess", deny: "guess", tasks: "guess", dotsList: "guess" };
export const nameRe = (k: keyof typeof NAMES) => new RegExp(NAMES[k], "i");

/** Dots vocabulary (memo section 7): the four rule settings; the two that reach the user are Ask first and Hand off. */
export const PATTERNS = {
  askFirst: /^ask first/i,
  handOff: /^hand off/i,
  rateLimit: /you'?ve reached (your )?(usage )?limit|usage limit|limit reached|too many requests|rate limit/i,
  challenge: /verify (that )?you('| a)re (a )?human|unusual activity|suspicious activity|captcha|account (has been )?(suspended|restricted)/i,
  paused: /(dot|it) (has been |was |is )?paused|paused (by|for) (safety|monitoring)/i,
} as const;

export const TASK_SECTIONS: Record<string, "in_progress" | "scheduled" | "completed"> = {
  "in progress": "in_progress", scheduled: "scheduled", completed: "completed",
};
export const DOT_URL = (id: string) => `https://chatgpt.com/dots/${encodeURIComponent(id)}`; // inferred route
export const DOTS_LIST_URL = "https://chatgpt.com/dots"; // inferred route
