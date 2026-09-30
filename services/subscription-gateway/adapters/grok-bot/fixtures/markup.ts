// Grok Bot renderer markup, structure-only. Transcript rows, message, prose, composer (tiptap ProseMirror) and send
// button mirror the LIVE DOM of Grok Bot 0.61.0 (verified 2026-09-29; all text is placeholder, no ids/urls/avatars).
// Stop button, approval-card and banner containers are still INFERRED (not observed live; see README).
export interface Scenario {
  turns?: Array<{ role: "user" | "assistant"; text: string }>;
  streaming?: boolean;
  composerText?: string;
  banner?: string;
  approval?: string;
  routine?: string;
  loggedOut?: boolean;
  drift?: "composer" | "turn-content";
  picker?: boolean;
  /** Bot rows shown in the picker (default one placeholder row). */
  bots?: string[];
}
export const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

const turn = (t: { role: string; text: string }, drift?: string) =>
  `<div class="sand-virtual-transcript__row sand-transcript-row"><div class="sand-message-block"><div class="sand-message" data-role="${t.role}" role="article"><span class="sand-sr">${t.role === "user" ? "You said" : "Bot said"}</span>` +
  (drift === "turn-content" && t.role === "assistant" ? `<p class="msg-body-x1">${esc(t.text)}</p>` : `<div class="sand-message-prose"><p>${esc(t.text)}</p></div>`) +
  `</div></div></div>`;

export function renderPage(s: Scenario = {}): string {
  const banner = s.banner ? `<div role="alert" class="sand-banner"><p>${esc(s.banner)}</p></div>` : "";
  const approval = s.approval
    ? `<div role="group" class="sand-approval-card"><p>Grok Bot needs your approval</p><p>${esc(s.approval)}</p><button type="button">Allow once</button><button type="button">Always allow</button><button type="button">Deny</button></div>`
    : "";
  const routine = s.routine ? `<div class="sand-routine-card" data-component="routine-card"><span>Routine</span> <span>${esc(s.routine)}</span></div>` : "";
  const action = s.streaming ? `<button type="button" aria-label="Stop">Stop</button>` : `<button type="submit" class="ui-icon-button sand-prompt-send" aria-label="${s.composerText ? "Send message" : "Start voice input"}"></button>`;
  const composer = s.loggedOut
    ? `<div class="sand-auth-gate"><p>Sign in to Cursor in settings, then ask anything</p><button type="button">Sign in</button></div>`
    : `<form class="sand-kit-message-input-frame sand-prompt-shell"><div class="sand-prompt-content"><div class="sand-prompt-field-host">` +
      (s.drift === "composer"
        ? `<div class="composer-x9" contenteditable="true">${esc(s.composerText ?? "")}</div>`
        : `<div class="tiptap ProseMirror sand-prompt-field" contenteditable="true" role="textbox" aria-label="Message"><p>${esc(s.composerText ?? "")}</p></div>`) +
      `</div></div><div class="sand-prompt-actions-row"><button type="button" class="sand-prompt-attach" aria-label="Attach file"></button>${action}</div></form>`;
  const picker = s.picker ? `<div class="sand-new-chat-picker"><button type="button" aria-label="Close new chat"></button><button type="button" aria-label="Create new Bot"></button>${(s.bots ?? ["Example Bot"]).map((b) => `<button type="button">${b}</button>`).join("")}</div>` : "";
  const turns = (s.turns ?? []).map((t) => turn(t, s.drift)).join("");
  return `<!doctype html><html lang="en"><head><meta charset="UTF-8"><title>Grok Bot</title></head><body>` +
    `<div id="root"><div class="sand-app"><nav aria-label="Sidebar"><button type="button" aria-label="New chat">New chat</button><section><h3>Routines</h3></section></nav>` +
    `<main>${banner}<div class="sand-transcript">${turns}${routine}${approval}</div>${picker}${composer}</main></div></div></body></html>`;
}

export const SCENARIOS: Record<string, Scenario> = {
  idle: {},
  picker: { picker: true },
  streaming: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "Looking at your inbox. So far I see 12 unread" }], streaming: true },
  complete: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "You have 12 unread messages; 2 need a reply today." }], routine: "Daily inbox digest" },
  approval: { turns: [{ role: "user", text: "Email Sam the notes" }, { role: "assistant", text: "Ready to send the notes to Sam." }], streaming: true, approval: "Send email to sam@example.com" },
  "rate-limit": { banner: "Usage limit reached. Your usage limit has been reached" },
  "bot-check": { banner: "Unusual activity detected. Verify you are human to continue" },
  "logged-out": { loggedOut: true },
  drift: { drift: "composer", turns: [{ role: "user", text: "hi" }] },
  "drift-turn": { drift: "turn-content", turns: [{ role: "user", text: "hi" }, { role: "assistant", text: "hello" }] },
};
