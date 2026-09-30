// Fixture markup builder for the ChatGPT dots UI. HAND-BUILT, OFFLINE. Chat chrome (composer, send/stop buttons,
// turns, profile button, login gate, alert/limit banner, challenge iframe) mirrors chatgpt-web's live-verified DOM
// (adapters/chatgpt-web/fixtures + selectors/v1.yaml). Everything dots-specific (dots list rows, dot header, Ask first /
// Hand off cards, tasks panel, activity indicator) is INFERRED from the memo, not observed: see README confidence table.
// All text is placeholder; no ids, urls or avatars from a real account.
export interface Scenario {
  view?: "list" | "dot";
  dots?: Array<{ id: string; name: string; handle?: string }>;
  dot?: { id: string; name: string; handle?: string };
  turns?: Array<{ role: "user" | "assistant"; text: string }>;
  streaming?: boolean;
  composerText?: string;
  banner?: string;
  confirmation?: { policy: "ask_first" | "hand_off"; text: string };
  tasks?: Array<{ title: string; state: "in_progress" | "scheduled" | "completed" }>;
  activity?: string;
  showTasks?: boolean;
  loggedOut?: boolean;
  /** chatgpt.com/dots on a plan without dots: an upsell dialog and no app shell (seen live 2026-09-30). */
  planRequired?: boolean;
  challenge?: boolean;
  drift?: "composer";
}
export const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

const turn = (t: { role: string; text: string }) => t.role === "user"
  ? `<div data-user-message-bubble="true">${esc(t.text)}</div>`
  : `<div data-conversation-role="assistant"><div data-markdown-text-style="assistant-message"><p>${esc(t.text)}</p></div></div>`;

const SECTIONS = [["in_progress", "In progress"], ["scheduled", "Scheduled"], ["completed", "Completed"]] as const;

export function renderPage(s: Scenario = {}): string {
  const profile = `<nav><button data-testid="profile-button" aria-label="Open profile menu">Me</button><a href="/dots">Dots</a></nav>`;
  if (s.planRequired) return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>ChatGPT</title></head><body><div role="dialog"><h2>Dots require a Pro plan</h2><p>Upgrade to Pro 100 to be eligible as access rolls out</p><button>Upgrade to Pro</button></div></body></html>`;
  if (s.loggedOut) return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>ChatGPT</title></head><body><header><a href="/auth/login" data-testid="login-button">Log in</a></header><main><h1>Get started</h1><p>Log in to continue.</p></main></body></html>`;
  if (s.challenge) return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>ChatGPT</title></head><body><main><h1>Verify you are human</h1><iframe src="https://challenges.cloudflare.com/turnstile/v0/b/abc123" title="verification"></iframe><p>Complete the verification to continue.</p></main></body></html>`;
  const banner = s.banner ? `<div data-testid="limit-banner" role="alert"><p>${esc(s.banner)}</p></div>` : "";
  let body: string;
  if ((s.view ?? "dot") === "list") {
    const rows = (s.dots ?? []).map((d) => `<a href="/dots/${esc(d.id)}" data-testid="dot-row"><span data-testid="dot-avatar"><img src="https://example.invalid/dot-${esc(d.id)}.png" alt="${esc(d.name)} avatar"></span><span data-testid="dot-name">${esc(d.name)}</span>${d.handle ? `<span data-testid="dot-handle">${esc(d.handle)}</span>` : ""}</a>`).join("");
    body = `<section aria-label="Dots"><h1>Your dots</h1>${rows}</section>`;
  } else {
    const dot = s.dot ?? { id: "nova-dot", name: "Nova", handle: "@nova-dot" };
    const header = `<header data-testid="dot-header"><span data-testid="dot-avatar"><img src="https://example.invalid/dot-${esc(dot.id)}.png" alt="${esc(dot.name)} avatar"></span><h1 data-testid="dot-name">${esc(dot.name)}</h1>${dot.handle ? `<span data-testid="dot-handle">${esc(dot.handle)}</span>` : ""}</header>`;
    const conf = s.confirmation
      ? `<div role="group" data-testid="dot-confirmation" data-policy="${s.confirmation.policy}"><span>${s.confirmation.policy === "hand_off" ? "Hand off" : "Ask first"}</span><p>${esc(s.confirmation.text)}</p>` +
        (s.confirmation.policy === "hand_off" ? `<button type="button">Open computer</button>` : `<button type="button">Approve</button><button type="button">Deny</button>`) + `</div>`
      : "";
    const tasks = s.showTasks || s.tasks
      ? `<section aria-label="Tasks" data-testid="dot-tasks">` + SECTIONS.map(([k, label]) => `<h3>${label}</h3><ul>${(s.tasks ?? []).filter((t) => t.state === k).map((t) => `<li data-testid="dot-task">${esc(t.title)}</li>`).join("")}</ul>`).join("") + `</section>`
      : "";
    const activity = s.activity ? `<div data-testid="dot-activity">${esc(s.activity)}</div>` : "";
    const action = s.streaming ? `<button type="button" data-testid="stop-button" aria-label="Stop">Stop</button>` : s.composerText ? `<button type="submit" data-testid="send-button" aria-label="Send">Send</button>` : `<button type="button" aria-label="Start Voice">Voice</button>`;
    const composer = `<form>` + (s.drift === "composer"
      ? `<div class="composer-x9" contenteditable="true">${esc(s.composerText ?? "")}</div>`
      : `<div id="prompt-textarea" data-testid="prompt-textarea" contenteditable="true" role="textbox" aria-label="Ask ChatGPT"><p>${esc(s.composerText ?? "")}</p></div>`) + action + `</form>`;
    body = `${header}${activity}<div id="thread">${(s.turns ?? []).map(turn).join("")}${conf}</div>${tasks}${composer}`;
  }
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>ChatGPT</title></head><body>${profile}<main>${banner}${body}</main></body></html>`;
}

const DOTS = [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }];
export const SCENARIOS: Record<string, Scenario> = {
  "dot-list": { view: "list", dots: DOTS },
  idle: {},
  streaming: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "Looking at your inbox. So far I see 12 unread" }], streaming: true, activity: "Working" },
  complete: { turns: [{ role: "user", text: "Summarise my inbox" }, { role: "assistant", text: "You have 12 unread messages; 2 need a reply today." }] },
  "ask-first": { turns: [{ role: "user", text: "Email Sam the notes" }, { role: "assistant", text: "Ready to send the notes to Sam." }], confirmation: { policy: "ask_first", text: "Send email to sam@example.com" } },
  "hand-off": { turns: [{ role: "user", text: "Rotate my hosting password" }], confirmation: { policy: "hand_off", text: "Change the password for the hosting account" } },
  tasks: { showTasks: true, tasks: [{ title: "Compile vendor notes", state: "in_progress" }, { title: "Weekly inbox digest", state: "scheduled" }, { title: "Book travel research", state: "completed" }] },
  "rate-limit": { banner: "You've reached your usage limit. Your quota resets at 14:00." },
  paused: { banner: "Nova has been paused for safety monitoring." },
  challenge: { challenge: true },
  "logged-out": { loggedOut: true },
  "plan-required": { planRequired: true },
  drift: { drift: "composer", turns: [{ role: "user", text: "hi" }] },
};
