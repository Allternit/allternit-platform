// Claude Desktop page markup (claude.ai inside the app), structure-only and HAND-BUILT. NOTHING here was observed live:
// the app refuses a debugging port without a vendor-signed token, so every hook below is either recalled from the public
// claude.ai DOM (composer, user-message, font-claude-response, standard-markdown, data-is-streaming) or designed on accessible
// names (buttons, permission card, banners). See README confidence table. All text is placeholder.
export interface Scenario {
  turns?: Array<{ role: "user" | "assistant"; text: string }>;
  streaming?: boolean;
  composerText?: string;
  banner?: string;
  approval?: string;
  tool?: string;
  artifact?: string;
  cowork?: boolean;
  loggedOut?: boolean;
  drift?: "composer" | "turn-content";
}
export const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

const turn = (t: { role: string; text: string }, streaming: boolean, drift?: string) =>
  t.role === "user"
    ? `<div data-test-render-count="1"><div class="group"><div data-testid="user-message" class="font-user-message"><p>${esc(t.text)}</p></div></div></div>`
    : `<div data-test-render-count="1"><div data-is-streaming="${streaming}" class="group"><div class="font-claude-response">` +
      (drift === "turn-content" ? `<p class="msg-x1">${esc(t.text)}</p>` : `<div class="standard-markdown"><p>${esc(t.text)}</p></div>`) +
      `</div></div></div>`;

export function renderPage(s: Scenario = {}): string {
  const banner = s.banner ? `<div role="alert" class="usage-banner"><p>${esc(s.banner)}</p></div>` : "";
  const approval = s.approval
    ? `<div role="dialog" aria-label="Permission request" class="permission-card"><p>Allow Claude to use ${esc(s.approval)}?</p><button type="button">Deny</button><button type="button">Allow once</button><button type="button">Always allow</button></div>`
    : "";
  const tool = s.tool ? `<div data-testid="tool-use-block"><span>${esc(s.tool)}</span></div>` : "";
  const artifact = s.artifact ? `<div data-testid="artifact-block-cell"><span>${esc(s.artifact)}</span></div>` : "";
  const action = s.streaming ? `<button type="button" aria-label="Stop response"></button>` : `<button type="button" aria-label="Send message"${s.composerText ? "" : " disabled"}></button>`;
  const composer = s.loggedOut
    ? `<div class="auth-gate"><h1>Talk with Claude</h1><button type="button">Continue with Google</button><button type="button">Continue with email</button></div>`
    : `<fieldset class="composer"><div data-testid="chat-input-grid">` +
      (s.drift === "composer"
        ? `<div class="composer-x9" contenteditable="true">${esc(s.composerText ?? "")}</div>`
        : `<div data-testid="chat-input" class="tiptap ProseMirror" contenteditable="true" role="textbox" aria-label="Write your prompt to Claude"><p>${esc(s.composerText ?? "")}</p></div>`) +
      `<button type="button" aria-label="Add files, connectors, and more"></button>${action}</div></fieldset>`;
  const turns = (s.turns ?? []).map((t, i, a) => turn(t, !!s.streaming && i === a.length - 1, s.drift)).join("");
  const nav = `<nav aria-label="Sidebar"><a href="/new" aria-label="New chat">New chat</a><a href="/recents">Recents</a>` +
    `<div role="tablist"><button role="tab" aria-selected="${!s.cowork}">Chat</button><button role="tab" aria-selected="${!!s.cowork}">Cowork</button></div>` +
    (s.cowork ? `<button type="button">New task</button>` : "") + `</nav>`;
  return `<!doctype html><html lang="en"><head><meta charset="UTF-8"><title>Claude</title></head><body>` +
    `<div id="root">${nav}<main>${banner}<div class="transcript">${turns}${tool}${artifact}${approval}</div>${composer}</main></div></body></html>`;
}

export const SCENARIOS: Record<string, Scenario> = {
  idle: {},
  cowork: { cowork: true },
  streaming: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "Looking at your inbox. So far I see 12 unread" }], streaming: true },
  complete: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "You have 12 unread messages; 2 need a reply today." }], tool: "Read files", artifact: "inbox-summary.md" },
  approval: { cowork: true, turns: [{ role: "user", text: "Tidy the notes folder" }, { role: "assistant", text: "Ready to move the files." }], streaming: true, approval: "Finder" },
  "rate-limit": { banner: "You have reached your usage limit. Your limit resets in 2 hours" },
  "bot-check": { banner: "Unusual activity detected. Verify you are human to continue" },
  "logged-out": { loggedOut: true },
  drift: { drift: "composer", turns: [{ role: "user", text: "hi" }] },
  "drift-turn": { drift: "turn-content", turns: [{ role: "user", text: "hi" }, { role: "assistant", text: "hello" }] },
};
