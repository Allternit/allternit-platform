// Grok Bot selector pack v2 (live-verified against Grok Bot 0.61.0 on 2026-09-29; v1 was bundle-recon only) — ordered fallbacks per named key (chatgpt-web convention).
// Provenance (recon of Grok Bot 0.61.0 renderer bundle, see README):
//   VERIFIED-IN-BUNDLE: .sand-message[data-role=assistant|user][role=article], .sand-message-content,
//     .sand-transcript-row, .ui-prompt-input-editor__input (ProseMirror), lingui English strings
//     "Send message", "Stop", "Sign in", "Allow once", "Always allow", "Deny", "Skip", "Routines", "New chat".
//   INFERRED (from message catalog only; exact markup unseen): approval card container, banners, routine cue.
// v2 changes: turnContent is .sand-message-prose (v1 .sand-message-content does not exist live);
//   composer leads with .ProseMirror[contenteditable=true] (.ui-prompt-input-editor__input absent live).
// Bump SELECTORS_VERSION when the live UI drifts.
export const SELECTORS_VERSION = "v2";

export interface KeySpec { critical: boolean; css: string[] }
export const SELECTORS: Record<string, KeySpec> = {
  appRoot: { critical: true, css: ["#root"] },
  composer: { critical: true, css: [".ProseMirror[contenteditable=true]", "[contenteditable=true][role=textbox]", ".ui-prompt-input-editor__input"] },
  turn: { critical: false, css: [".sand-message[role=article]", "[role=article][data-role]"] },
  turnContent: { critical: false, css: [".sand-message-prose", ".sand-message-content"] },
  routineCue: { critical: false, css: ["[data-component=routine-card]", ".sand-routine-card"] },
  alert: { critical: false, css: ["[role=alert]", "[role=status]", "[role=alertdialog]"] },
};

/** Accessible-name patterns (Lingui English catalog, locale en). Sources are strings so the CDP driver can eval them. */
export const NAMES = {
  send: "^(send message|send)$",
  stop: "^stop$",
  newChat: "^new chat$",
  // New chat opens a Bot picker (live 0.61.0): composer is unusable until a Bot row is chosen.
  closePicker: "^close new chat$",
  // Non-Bot controls inside the picker (live 0.61.0): never reported as Bots.
  pickerControls: "^(close new chat|create new bot|create group chat)$",
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
