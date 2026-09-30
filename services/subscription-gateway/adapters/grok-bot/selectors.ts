// Grok Bot selector pack v1 — ordered fallbacks per named key (chatgpt-web convention).
// Provenance (recon of Grok Bot 0.61.0 renderer bundle, see README):
//   VERIFIED-IN-BUNDLE: .sand-message[data-role=assistant|user][role=article], .sand-message-content,
//     .sand-transcript-row, .ui-prompt-input-editor__input (ProseMirror), lingui English strings
//     "Send message", "Stop", "Sign in", "Allow once", "Always allow", "Deny", "Skip", "Routines", "New chat".
//   INFERRED (from message catalog only; exact markup unseen): approval card container, banners, routine cue.
// Bump SELECTORS_VERSION and add v2 when the live UI drifts; never edit v1 in place.
export const SELECTORS_VERSION = "v1";

export interface KeySpec { critical: boolean; css: string[] }
export const SELECTORS: Record<string, KeySpec> = {
  appRoot: { critical: true, css: ["#root"] },
  composer: { critical: true, css: [".ui-prompt-input-editor__input", ".ProseMirror[contenteditable=true]", "[contenteditable=true][role=textbox]"] },
  turn: { critical: false, css: [".sand-message[role=article]", "[role=article][data-role]"] },
  turnContent: { critical: false, css: [".sand-message-content"] },
  routineCue: { critical: false, css: ["[data-component=routine-card]", ".sand-routine-card"] },
  alert: { critical: false, css: ["[role=alert]", "[role=status]", "[role=alertdialog]"] },
};

/** Accessible-name patterns (Lingui English catalog, locale en). Sources are strings so the CDP driver can eval them. */
export const NAMES = {
  send: "^(send message|send)$",
  stop: "^stop$",
  newChat: "^new chat$",
  signIn: "^sign in$",
  approve: "^(allow once|allow|approve)$",
  deny: "^(deny|skip)$",
} as const;
export const nameRe = (k: keyof typeof NAMES) => new RegExp(NAMES[k], "i");

export const PATTERNS = {
  approval: /needs (your )?approval|requires your approval|needs your attention|needs you\b/i,
  rateLimit: /rate limited|rate.limiting|usage limit|limit reached|too many requests/i,
  botCheck: /verify (that )?you('| a)re (a )?human|unusual activity|suspicious activity|captcha|account (has been )?(suspended|restricted)/i,
} as const;
