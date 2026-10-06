# Audit + Redesign: Agent Activity design mockup

Two mockup HTML files sit next to this file: `mockup-v1.html` and `mockup-v2.html`. Open both directly in a browser (`open docs/agent-activity-design/mockup-v1.html` etc., or just read the source — they're plain self-contained HTML/CSS/JS, no build step) before doing anything else.

## Context: what this is actually for

Allternit (the product, this repo) has a real, already-functional backend feature called **Factory mail** — an internal agent-to-agent / agent-to-human coordination system (`cmd/allternit-api/src/rails/mod.rs`, the Factory engine crate under `factory/engine`). It's the exact mechanism this repo's own `agent-orchestrator` skill uses to post executor progress. Real capabilities, all already live:

- `ensureThread(topic)` — create/get a thread
- `send(threadId, ...)` — post a message into a thread
- `inbox(...)` — list messages
- `ack(threadId, messageId, note?)` — acknowledge a message
- `requestReview(threadId, wihId, diffRef)` — an agent asks a human to review a specific diff
- `decide(threadId, approve, notesRef?)` — the human's approve/deny decision on a review request
- `reserve(wihId, agentId, paths, ttl?)` — an agent reserves a set of file paths (concurrency control between agents)
- `share(threadId, assetRef, note?)` — attach/share an asset (e.g. a build artifact, a screenshot) into a thread
- `archive(threadId, path, reason?)` — archive a thread
- `guard(action, detail?)` — a policy/guard-rail action

Today, **no surface has a real, discoverable UI for this.** Web's only entry point is a hidden global keyboard shortcut (Ctrl/Cmd+Shift+M) opening `ConversationMonitorOverlay.tsx` — no menu item, no nav entry. The product owner wants this built into a real, first-class feature **across all four of this repo's surfaces** (web/desktop, iOS, gizzi-code CLI — see root `GIZZI.md` for the 4-surface architecture) — starting with getting the web design right first, since that becomes the reference the other surfaces mirror.

`mockup-v1.html` was a first pass (rejected — "I don't like this very much, look at other production examples"). Research into real production tools (findings already folded into `mockup-v2.html`'s own on-page notes) covered: Claude Code's own Agent View (flat session list, status + last-response preview, inline expand, inline quick-reply, Enter for full transcript), GitHub's Copilot "Agents panel" (header-icon slide-over, not a dedicated page; clicking routes into the real PR review rather than reinventing approval UI), and Cursor's Background Agents (mobile/web surfaces are monitor-and-approve only, real review happens via the existing PR flow). `mockup-v2.html` applied those lessons. The product owner has NOT yet approved v2 — they asked for a deeper agent-orchestrator-run audit before committing to it.

## Your task

### 1. Research further — go beyond what's already been covered

Don't re-research Claude Code Agent View / GitHub Agents panel / Cursor Background Agents in depth (already covered, cited in `mockup-v2.html`'s banner). Instead, research and pull real, concrete UI patterns from:

- **Linear's Triage inbox** (`linear.app/docs/triage`, `linear.app/now/how-we-built-triage-intelligence`) — a genuinely different pattern (a dedicated queue for items awaiting a human decision, with AI-suggested triage) worth comparing against a "needs review" queue.
- **Incident-management / on-call tools** (PagerDuty, Opsgenie, or similar) — these have decades of refined UX specifically for "here's something that needs a human decision, here's the context, here's how fast they can act on it" — a very close analog to Factory mail's `requestReview`/`decide` flow.
- **Slack's approval workflows / Workflow Builder approval steps**, and/or **GitHub's own Notifications inbox** (different from the Agents panel — the general `/notifications` page) — both handle "a stream of things that happened, some need action, most don't."
- Pick at least one more you judge relevant (your call) — e.g. Vercel's deployment/comment notifications, Notion AI's inline suggestion UI, or a mobile-specific pattern (push-notification-driven approval flows) given this design also has to work on iOS eventually.

Cite real sources (URLs) for everything, the way the existing mockups' banners do.

### 2. Audit `mockup-v2.html` critically

Don't just validate it — find real problems. Consider at minimum:

- **Does the simplified "Approve/Deny + quick reply" model actually cover Factory mail's real action surface**, or does it flatten away real, distinct actions (`reserve` for path-locking, `share` for assets, `archive`, `guard`) into something that looks complete but isn't? Should some of these be visible/actionable from this UI at all, or are some legitimately backend-only/agent-only actions a human never needs to see?
- **Information density and scannability** — with a realistic volume of threads (this repo alone generated ~10 executor threads in one session; a heavy user could have dozens), does the flat list + inline-accordion-expand pattern hold up, or does it get unwieldy? How do the reference products you researched handle volume/scale?
- **Cross-surface translatability** — this design is the reference for iOS and a CLI too. A 380px desktop slide-over won't translate directly to a phone screen or a terminal. Does the underlying information architecture (not the literal pixels) translate cleanly to a narrow mobile view and to a text-based CLI list? Flag anything that's web-idiom-specific and won't generalize (e.g. hover states, a fixed-width panel, mouse-driven accordion expand).
- **The "View diff" link** — v2 says review items should route to "the real review surface" rather than reinventing one. What IS the real review surface in this specific repo, concretely? (Hint: check how this session's own PRs got reviewed — GitHub PR pages via `gh pr view`/`gh pr diff`. Does routing a Factory mail review-request to a GitHub PR make sense for every `requestReview` call, or only some? What about review requests that aren't about a code diff at all?)
- **Visual/interaction execution** — actual UI critique: hierarchy, whether the status-dot-pulse animation is meaningful or decorative, whether the tab bar (All/Review) is sufficient filtering for real volume, whether an accordion-per-row is the right disclosure pattern vs. something else.
- Anything else you find. Be honest and specific — vague "looks fine" verdicts are not useful. Every finding needs a concrete failure scenario (what a real user would hit).

### 3. Produce `mockup-v3.html` — the redesign

Address your own audit findings. Requirements:
- Self-contained HTML/CSS/JS, no external CDN dependencies (matches v1/v2 — this may get published as a Claude Artifact later, which blocks external requests).
- Reuse the same Allternit design tokens both prior mockups use (surface-canvas/panel/active, text-primary/secondary/muted, border-muted/default, accent-primary `#7c3aed`/`#a78bfa` dark, status-success/warning/error/info, `--font-sans`/`--font-mono`) — check `mockup-v2.html`'s `<style>` block for the exact values, and `surfaces/ai.allternit.com`'s own `DESIGN.md` if you want to verify against the source of truth.
- Both light and dark themes (`prefers-color-scheme` + explicit `data-theme` root overrides, same pattern as v1/v2).
- Interactive (real click/state behavior via inline JS), grounded in realistic example content (real thread names/content in the style of `wih:executor-<slug>`, not lorem ipsum).
- A visible on-page banner (like v1/v2 have) explaining what changed from v2 and why, with cited sources for the new research.

## Constraints

- This is a design/mockup task, not a code-implementation task — do not touch anything under `surfaces/`, `cmd/`, or any file outside `docs/agent-activity-design/`.
- Do not build a real feature yet — this is still the design phase. No backend/frontend production code.
- Do NOT start any item from `docs/learnings/SURFACE_AUDIT_PROGRESS.md` — unrelated, separate work.

## Deliverable

`docs/agent-activity-design/AUDIT_NOTES.md`, YAML frontmatter (`status`, `files_changed`, `deviations`, `remaining`), then prose: your audit findings (with the concrete failure scenario for each), the new sources you pulled from, and what changed in v3 and why. Plus the `mockup-v3.html` file itself. That NOTES file existing = done.
