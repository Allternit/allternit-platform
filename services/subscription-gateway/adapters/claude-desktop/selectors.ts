// Claude Desktop selector pack v1. NOT live-verified: the app renders claude.ai remotely (main webContents loadURL(claude.ai)),
// so the DOM is not in the bundle, and Claude Desktop refuses --remote-debugging-port without an Anthropic-signed token (README).
// Confidence per key (H = seen in bundle, M = long-stable claude.ai hook recalled from the public web app, L = designed on
// role/accessible name only). Everything here is unverified against the running app; see README table.
//   appRoot L, composer M (ProseMirror contenteditable, aria-label "Write your prompt to Claude"), turn M (data-testid=user-message,
//   .font-claude-response), turnContent M (.standard-markdown / .progressive-markdown), streaming M (data-is-streaming, "Stop response"),
//   toolCue L, alert L, buttons by accessible name M (English UI only; other locales read as drift by design).
// Bump SELECTORS_VERSION when the live UI drifts.
export const SELECTORS_VERSION = "v1";

export interface KeySpec { critical: boolean; css: string[] }
export const SELECTORS: Record<string, KeySpec> = {
  appRoot: { critical: true, css: ["#root", "body"] },
  composer: { critical: true, css: ["[data-testid=chat-input]", ".ProseMirror[contenteditable=true]", "[contenteditable=true][role=textbox]"] },
  // user turns carry data-testid=user-message; assistant turns are .font-claude-response (role derived in observe.ts)
  turn: { critical: false, css: ["[data-testid=user-message], .font-claude-response", "[data-testid=user-message], [data-is-streaming]"] },
  turnContent: { critical: false, css: [".standard-markdown, .progressive-markdown", ".font-claude-response-body", ".prose"] },
  streamingMarker: { critical: false, css: ["[data-is-streaming=true]"] },
  toolCue: { critical: false, css: ["[data-testid=tool-use-block]", "[data-tool-use]"] },
  artifactCue: { critical: false, css: ["[data-testid=artifact-block-cell]", "[data-artifact]"] },
  coworkTask: { critical: false, css: ["[data-testid=cowork-task]", "[data-cowork-task]"] },
  alert: { critical: false, css: ["[role=alert]", "[role=status]", "[role=alertdialog]", "[data-testid=usage-limit-banner]"] },
};

/** Accessible-name patterns (English UI). Sources are strings so the CDP driver can eval them. */
export const NAMES = {
  send: "^(send message|send)$",
  stop: "^(stop response|stop)$",
  newChat: "^new chat$",
  cowork: "^cowork$",
  newTask: "^new task$",
  signIn: "^(log in|sign in|continue with (google|email))$",
  // Deliberately NOT "always allow": Allternit only ever offers one-shot approval, never a persistent grant.
  approve: "^(allow once|allow|approve)$",
  deny: "^(deny|don.t allow|decline|reject)$",
} as const;
export const nameRe = (k: keyof typeof NAMES) => new RegExp(NAMES[k], "i");

export const PATTERNS = {
  approval: /allow claude to|needs your (approval|permission)|requires your (approval|permission)|wants to (use|access|run)|permission (request|required)/i,
  rateLimit: /usage limit|limit reached|rate limit|too many requests|out of (free )?messages|reached your (message|usage) limit/i,
  botCheck: /verify (that )?you('| a)re (a )?human|unusual activity|suspicious activity|captcha|account (has been )?(suspended|restricted|disabled)/i,
  /** "resets in 2 hours" / "try again in 45 minutes" */
  retryIn: /(?:resets?|try again|available again|back) in (?:about |~)?(\d+)\s*(minute|min|hour|hr)s?/i,
  /** "resets at 3:00 PM" */
  retryAt: /resets? at (\d{1,2})(?::(\d{2}))?\s*(am|pm)/i,
} as const;
