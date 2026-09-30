// Hand-built Grok Bot renderer markup. Class names / roles / data-role are the ones found in the 0.61.0 bundle;
// approval-card and banner containers are INFERRED from the message catalog (see README, "unverified live").
export interface Scenario {
  turns?: Array<{ role: "user" | "assistant"; text: string }>;
  streaming?: boolean;
  composerText?: string;
  banner?: string;
  approval?: string;
  routine?: string;
  loggedOut?: boolean;
  drift?: "composer" | "turn-content";
}
export const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

const turn = (t: { role: string; text: string }, drift?: string) =>
  `<div class="sand-transcript-row"><div class="sand-message" data-role="${t.role}" role="article">` +
  (drift === "turn-content" && t.role === "assistant" ? `<p class="msg-body-x1">${esc(t.text)}</p>` : `<p class="sand-message-content">${esc(t.text)}</p>`) +
  `</div></div>`;

export function renderPage(s: Scenario = {}): string {
  const banner = s.banner ? `<div role="alert" class="sand-banner"><p>${esc(s.banner)}</p></div>` : "";
  const approval = s.approval
    ? `<div role="group" class="sand-approval-card"><p>Grok Bot needs your approval</p><p>${esc(s.approval)}</p><button type="button">Allow once</button><button type="button">Always allow</button><button type="button">Deny</button></div>`
    : "";
  const routine = s.routine ? `<div class="sand-routine-card" data-component="routine-card"><span>Routine</span> <span>${esc(s.routine)}</span></div>` : "";
  const action = s.streaming ? `<button type="button" aria-label="Stop">Stop</button>` : `<button type="button" aria-label="Send message">Send</button>`;
  const composer = s.loggedOut
    ? `<div class="sand-auth-gate"><p>Sign in to Cursor in settings, then ask anything</p><button type="button">Sign in</button></div>`
    : `<form class="ui-prompt-input"><div class="ui-prompt-input-editor__content">` +
      (s.drift === "composer"
        ? `<div class="composer-x9" contenteditable="true">${esc(s.composerText ?? "")}</div>`
        : `<div class="ProseMirror ui-prompt-input-editor__input" contenteditable="true" role="textbox">${esc(s.composerText ?? "")}</div>`) +
      `</div>${action}</form>`;
  const turns = (s.turns ?? []).map((t) => turn(t, s.drift)).join("");
  return `<!doctype html><html lang="en"><head><meta charset="UTF-8"><title>Grok Bot</title></head><body>` +
    `<div id="root"><div class="sand-app"><nav aria-label="Sidebar"><button type="button" aria-label="New chat">New chat</button><section><h3>Routines</h3></section></nav>` +
    `<main>${banner}<div class="sand-transcript">${turns}${routine}${approval}</div>${composer}</main></div></div></body></html>`;
}

export const SCENARIOS: Record<string, Scenario> = {
  idle: {},
  streaming: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "Looking at your inbox. So far I see 12 unread" }], streaming: true },
  complete: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "You have 12 unread messages; 2 need a reply today." }], routine: "Daily inbox digest" },
  approval: { turns: [{ role: "user", text: "Email Sam the notes" }, { role: "assistant", text: "Ready to send the notes to Sam." }], streaming: true, approval: "Send email to sam@example.com" },
  "rate-limit": { banner: "Usage limit reached. Your usage limit has been reached" },
  "bot-check": { banner: "Unusual activity detected. Verify you are human to continue" },
  "logged-out": { loggedOut: true },
  drift: { drift: "composer", turns: [{ role: "user", text: "hi" }] },
  "drift-turn": { drift: "turn-content", turns: [{ role: "user", text: "hi" }, { role: "assistant", text: "hello" }] },
};
