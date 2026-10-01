# chatgpt-dots: OpenAI dots via ChatGPT web (ui_bridge)

Lane `ui_bridge`, guarantee `best_effort`, mode `linked`, account = the user's own ChatGPT subscription (a plan that includes a dot: Pro or Business Premium).
Vendor `openai`, look pack `chatgpt-dots`. Spec: `Research/specs/agent-gateway.md` (dots golden path = ChatGPT app). Memo: `Research/memos/2026-09-29-openai-dots.md`.

**Status: OFFLINE ONLY.** Built and tested without ever opening chatgpt.com. Every dots-specific selector below is a hand-built inference. Nothing here is live-verified until a consented session runs (see "Needs a consented live session").

## Why the ChatGPT web (not the desktop app)
The ChatGPT desktop app on macOS is native (not Electron) so it cannot be driven over CDP. OpenAI publishes no dots API, webhooks or SDK. The ChatGPT website is the only driveable ChatGPT surface, so this adapter reuses the subsfab `chatgpt-web` browser lane (persistent headed Chrome per account profile).

## How it reuses chatgpt-web
- Manifest: origins, `login_url`, `logged_in_probe`, session-cookie hints and the pacing profile are read from `chatgpt-web/manifest.yaml` (`loadManifest()`), so the two adapters cannot disagree about login. A test pins this.
- Selectors: chat chrome keys (composer, assistant/user turn, logged-in probe, banner, challenge) are derived from `chatgpt-web/selectors/v1.yaml` at load time. A ChatGPT UI drift fixed there is fixed here.
- Pacing: the SDK `createPacer` (action gaps, min task gap, hourly and daily caps) fed chatgpt-web's profile. `PacingCapExceeded` maps to `RATE_LIMITED`.
- Browser: `browser-driver.ts` uses the same launch shape as `WorkerPool` (`launchPersistentContext`, `channel: chrome`, headed, per-account profile dir) and `isProfileLockError`. Navigation is restricted to the manifest origins.
- Parser: the DOM engine is grok-bot's `minidom.ts` (one parser, two adapters). Banner wording mirrors chatgpt-web's banner pack.
- Not reused: `DeclarativeChatAdapter` (a task-shaped worker adapter). Dots are conversational agents behind the AAI provider, so the provider observes the page directly.

## Surface
| AAI | Behaviour |
|---|---|
| `agent.list` | The user's dots, read from the dots list view. `agentId` = `chatgpt-dots:<dot id>` |
| `agent.identity` | Dot name, handle and avatar from the dot header (falls back to the list row) |
| `context.open` | Opens that dot's conversation. Binding `externalAgentId` = `chatgpt-dots:<dot id or name or @handle>`. With exactly one dot, bare `chatgpt-dots` works; with several, a missing name is `POLICY_DENIED`. A dot has one continuous conversation, so opening resumes it, but history before the open is not replayed as events (`resume: false`, `adoptContextId` is UNSUPPORTED) |
| `context.message` | Paced, idempotent by `correlationId` (one UI send) |
| events | `message.delta/completed`, `activity.started`, `approval.requested/resolved`, `task.updated`. Guarantees `best_effort`/`inferred`, never `exact` |
| Ask first | Card with policy Ask first: `agent.approval.requested`, authority `vendor`, `payload.policy = "ask_first"`. Only a human can answer (approve/deny are clicked inside that card only) |
| Hand off | Card with policy Hand off: `agent.approval.requested` with `payload.needsYou = true`, `policy = "hand_off"`. Respond returns `POLICY_DENIED` even for a human: passwords and money are always the person's to do in ChatGPT |
| `agent.tasks` | Read-only from the dot's profile panel (In progress, Scheduled, Completed). Also emits `agent.task.updated` when the panel is on screen. Cannot schedule or cancel |
| cancel / close | Cancel clicks Stop. Close never deletes, pauses or resets the dot |

Failures: usage limit banner => `RATE_LIMITED` (cooldown); verification challenge/captcha/unusual activity => `LANE_BLOCKED`, latched, never retried, never solved; selector drift => `ADAPTER_DRIFT`, latched until `clearHalt()`; logged out => `AUTH_REQUIRED` (`AUTH_REVOKED` if it was signed in before); dot shown as paused => `LANE_BLOCKED` (not latched; resume it in ChatGPT yourself).

## Isolation and parallelism (honest)
`maxParallel: 1`, `parallel: false`, `isolation: shared`. One browser page, one dot conversation at a time: opening another replaces the open one when it is idle (`CONTEXT_BUSY` while a reply is still running). A dot keeps one memory across everything it does; memory is opaque (the user cannot inspect it, so neither can we).

## Auth
`browser_session`: the user signs in themselves in the dedicated Chrome profile. No password or cookie paste, ever. The adapter never reads cookies, storage or profile files. The login watcher and session import remain chatgpt-web's. `requiresUserOwnedSubscription: true`, with a terms warning in the manifest.
Launch is gated: nothing opens Chrome unless `SUBS_GATEWAY_DOTS_CONSENT=1` and `SUBS_GATEWAY_DOTS_PROFILE_DIR` (absolute Chrome user-data dir) are both set; optional `SUBS_GATEWAY_DOTS_DEFAULT_DOT`. If that profile is already open (e.g. chatgpt-web's WorkerPool holds it) the driver reports `already_running` rather than fighting for it.

## Selector confidence (pack `dots-v1`)
| Key | Confidence | Basis |
|---|---|---|
| composer, send button, stop button | live | chatgpt-web, checked against a logged-in UI 2026-09-27 (role textbox "Ask ChatGPT", `button[type=submit]` "Send", "Stop") |
| assistant turn, user turn | live | chatgpt-web (`data-markdown-text-style=assistant-message`, `data-user-message-bubble=true`) |
| logged-in probe, login button, alert/banner, Cloudflare challenge iframe | live | chatgpt-web |
| dots route `/dots`, `/dots/<id>` | inferred | assumption; the live driver falls back to clicking the row link |
| dot row, name, handle, avatar (`dot-row`, `dot-name`, `dot-handle`, `dot-avatar`, `a[href^=/dots/]`) | inferred | memo section 6 (handle `@name-dot`, character/pet avatar); markup unseen |
| dot header (identity of the open dot) | inferred | unseen |
| Ask first / Hand off card (`dot-confirmation`, `role=group`, label text) | inferred | memo section 7 vocabulary; container markup unseen. Detection is by visible label text first, `data-policy` second |
| Approve / Deny button names (`approve\|allow\|allow once\|yes`, `deny\|decline\|not now\|no`) | guess | wording only |
| Hand off "Open computer" button | guess | never clicked by the adapter |
| Tasks panel + section headings "In progress", "Scheduled", "Completed", task rows | inferred | memo section 4 (listed in the dot's profile) |
| "Tasks"/"Activity" button that reveals the panel | guess | wording only |
| Activity indicator (`dot-activity`) | inferred | memo section 7 (Activity View) |
| dot "paused" banner | guess | memo section 7 says a paused dot shows a warning; wording unseen |
| usage-limit banner text | live wording for the ChatGPT limit banner; whether dots show it is unknown | memo section 9: dot conversations do not count toward ChatGPT limits, Codex/Work tasks do |

Fixtures: `fixtures/markup.ts` (source) generates `fixtures/*.html` (`npx tsx fixtures/generate.mjs`); a test asserts they stay identical. All text is placeholder.

## Needs a consented live session
1. Confirm the dots routes and the row/header markup (dots list, `/dots/<id>`), then the `dots-v1` "inferred" keys.
2. Capture a real Ask first card and a real Hand off card (button names, container, any stable id) and replace the "guess" names; check whether approving an Ask first card leaves a visible trace so `approval.resolved` can stop saying `outcome: unknown`.
3. Confirm how streaming/working looks for a dot (does the Stop button appear, does the composer stay?). A dot may keep working after the reply text stops changing.
4. Tasks: where the profile panel lives and whether it is reachable without leaving the conversation.
5. Confirm a dot chat is a single continuous thread on the web (vs per-chat threads), and the account's real pacing behaviour. Pacing is chatgpt-web's conservative profile until observed.
6. Whether the ChatGPT web profile lock conflicts with the running chatgpt-web lane for the same account (expected: use one profile at a time).
Live-check rules: consented account only, headed Chrome, no cookie/profile reads, stop at any challenge, one human present.

## Look pack
`look-profile.json`: white/black ChatGPT palette with the green orb accent, `dot_character_round` avatar treatment, status vocabulary "Ask first", "Hand off", "Needs you", "In progress", "Scheduled", "Completed". Palette and type are public-styling hints, not a live capture. `assets/chatgpt.png` was extracted read-only with `sips` from `/Applications/ChatGPT.app/Contents/Resources/icon-chatgpt.icns`.

## Tests
`test/chatgpt-dots-adapter.test.ts` (offline): fixtures vs markup, classification of every fixture, chatgpt-web reuse pins, look profile, manifest, `runConformance` (all declared areas pass; isolation/sync skipped-unsupported), list/identity/per-binding dot, streaming, idempotency, drift latch, challenge latch, limit cooldown, SDK pacing cap, Ask first vs Hand off, human-only approvals, tasks, UNSUPPORTED ops, consent gate.
