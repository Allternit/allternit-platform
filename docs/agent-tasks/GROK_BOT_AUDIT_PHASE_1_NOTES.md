---
task: grok-bot-audit-phase-1
status: complete
date: 2026-08-13
branch: ao/p3-grok-bot-audit
deliverable_type: audit-notes
phase: 1
next_phase: 2 (implementation plan below)
---

# Grok Bot (x.ai/bot) Audit — Phase 1 Notes

## 1. Grok Bot: Core UX Patterns (Reverse-Engineered from Public Docs)

Source: `docs.x.ai/grok-bot/overview`, `get-started`, `use-cases`, `skills-routines-and-automations`, `computer-and-apps`, `approvals-security-and-privacy`.

### 1.1 Persistent Named Bots

A "Bot" is a **persistent, named agent** with durable state: memory, files, browser sessions, and preferences. Context compounds across turns rather than resetting. Users create bots with a short name, one primary job, and a description of how it should work. Best practice: focused bots for distinct roles, not catch-all agents.

**Key insight:** Bots are identity-bearing entities, not ephemeral chat sessions. They learn user preferences over time.

### 1.2 Shared Cloud Computer

All bots under one account share a persistent cloud VM equipped with a browser, filesystem (`/workspace`), and terminal. Each bot gets its **own screen** for parallel execution, but screens are work surfaces — not security boundaries. Files, browser cookies, CLI credentials, and work state are shared across the bot roster.

**Key insight:** The shared environment enables seamless task handoffs between bots without repeating setup. One bot can continue work saved by another.

### 1.3 Live Screen Preview ("Agent Computer")

Users can view the shared desktop via an embedded interface within the conversation. The preview displays real-time clicks, typing, navigation, and status updates. Closing the preview does not stop cloud-based work.

**Key insight:** The live preview is the primary trust-building mechanism. Users can watch what the bot is doing at any time.

### 1.4 Human Takeover Flow

Bots pause and request user intervention for sensitive steps: passwords/passkeys, 2FA, CAPTCHAs, payment/identity checks. Users complete only the blocked step in the computer interface and instruct the bot to continue. Sensitive values use a "secure secret request" (masked input, excluded from transcripts, hidden from the model).

**Key insight:** The takeover flow is cooperative, not adversarial. The bot explicitly yields control for specific steps and reclaims it after.

### 1.5 Skills (Reusable Instruction Sets)

Skills capture steps, decision rules, expected outputs, and safety boundaries. Two creation paths:
- **Manual:** Ask the bot to save a successful process as a skill.
- **Demonstration:** Record browser interactions (up to 10 minutes) via "Teach a task." The resulting draft requires manual addition of decision rules and failure handling.

Skills are invoked via `/` in the composer. They are available across bots but require relevant connectors/logins per bot.

### 1.6 Routines (Scheduled/Event-Triggered Workflows)

Routines tell a specific bot **when** to run a workflow:
- **Scheduled:** "Every weekday at 8:00 AM" — background execution continues with laptop closed.
- **Event-triggered:** Integrations (Slack, GitHub) trigger routines on narrow matching rules.

Management: enable/pause, test run, edit, inspect history, delete. Max 50 routines per bot; 20 most recent run records retained. Deleting a bot removes its owned routines.

### 1.7 Approval System

Structured permission model with three actions per approval prompt:
- **Allow once** — permits this specific action.
- **Deny** — blocks this action.
- **Always allow** — saves a matching rule for future actions.

Auto-review configuration in Settings → General → Auto-review:
- **Require Approval:** Always stops matching actions (takes precedence).
- **Always Allow:** Permits if automated review finds no other stop reason.

Rules are stored locally on desktop and synced to the cloud computer.

### 1.8 Connectors (Plugins)

Structured integrations for supported services, managed via Settings → Plugins. Connectors are account-wide and more reliable than browser automation. In chat: `@` to attach a connector to a task, `/` to reference a saved skill.

### 1.9 Bot-to-Bot Coordination

Multiple bots can run in parallel, message each other, share context in threads/group chats, and pass ownership. The user acts as an approver rather than a router between tools.

### 1.10 Onboarding Flow

1. Welcome screen → "Get started"
2. Browser-based sign-in
3. Introduction to bots, shared computer, and routines
4. Tool survey (shapes teammate suggestions; does not auto-connect)
5. Background computer setup
6. "Meet a future teammate" — create or select first bot

### 1.11 Use Case Templates (8 Documented)

| Use Case | Owns | Key Pattern |
|----------|------|-------------|
| Sales Outbound | Account research, outreach drafts | Nightly research routine |
| Talent Scout | Sourcing, candidate research | Approval before outreach |
| Paid Media | Campaign monitoring, budget recs | Approval before changes |
| Expense Manager | Reconciliation, follow-ups | Policy-cited exceptions |
| Product Performance | Investigation with evidence | Recurring health reports |
| Bug Reproduction | Reliable reproduction packs | Staging only, no prod data |
| Account Health | Risk/expansion signals | Ranked watch list |
| Chief of Staff | Source-linked digest | Scheduled digest routine |

---

## 2. Allternit Current State: Agent Surfaces Map

### 2.1 Agent Hub (`src/views/AgentHub.tsx`)

Tab-based hub with four tabs:
- **Agent Studio** — Agent creation wizard (`CreateAgentForm.tsx`, ~2090 lines), edit form, detail view, gallery grid.
- **Sessions** — Agent-mode sessions across surfaces (`AgentSessionsTab.tsx`).
- **Analytics** — Performance analytics (`PerformanceAnalyticsView`).
- **Workspace** — Agent workspace panel (`AgentWorkspacePanel`).

**Note:** OpenMausBot Bots tab was implemented on branch `ao/p1-openmausbot` but is NOT present on this branch (`ao/p3-grok-bot-audit`). The current `AgentHub.constants.ts` has no `bots` tab.

### 2.2 Session Surfaces

Per-surface session stores:
- `useChatSessionStore` — Chat sessions
- `useCodeSessionStore` — Code sessions
- `useCoworkSessionStore` — Cowork sessions

ShellRail (`src/shell/ShellRail.tsx`, ~1595 lines) manages session recents with filters (type, status, date), per-mode rail tabs, and session switching.

### 2.3 Agent Surface Mode Store (`src/stores/agent-surface-mode.store.ts`)

Maps surfaces (`chat`, `cowork`, `code`, `browser`, `design`) to selected agents and modes. Modes include: `research`, `data`, `slides`, `code`, `assets`, `agents`, `flow`, `web`, `computer-use`, `plan`, `execute`, `review`, `report`, `automate`, `sync`, `routines`, `loops`, `team`, `swarms`, `website`, `docs`, `image`, `video`.

### 2.4 Routines System (Already Implemented)

- `src/views/cowork/RoutinesPanel.tsx` — Routine management UI.
- `src/lib/automation-api.ts` — API client for routines (list, create, update, delete, run).
- `src/pages/RoutinesListPage.tsx` — Standalone routines page.
- `src/views/automation/RoutinesListView.tsx` — Automation tasks view.
- Gateway: `CreateAgentSessionRequest` includes `session_mode`, `agent_features`.

### 2.5 Marketplace (`src/views/MarketplaceView.tsx`)

Capability marketplace with cards, search, categories, checkout modal, install/uninstall. Separate from agent marketplace.

Agent marketplace: `cmd/allternit-api/src/agent_routes.rs` has full CRUD for `agent_marketplace_listings`, `agent_marketplace_ratings`, `agent_marketplace_installs` (migration `V35__agent_marketplace.sql`).

### 2.6 Rails / Peer System

- Rust library: `allternit-commrails`
- HTTP surface: `/api/rails/peers`, `/api/rails/steer/*`
- gizzi-code integration: `railsPeer.ts`, `RailsInboxBridge.tsx`
- Peer registration, UDS inbox, message passing between agent sessions.

### 2.7 Computer Use Surface

`ComputerUseView` capsule + desktop JSON-RPC client + Zustand store. `ACIComputerUseSidecar.tsx` for browser-based computer use.

### 2.8 Composer

Shell-based composer integrated across surfaces. No `/` skill invocation or `@` connector attachment currently.

---

## 3. Gap Analysis: Grok Bot vs Allternit

### 3.1 Feature Parity Matrix

| Grok Bot Pattern | Allternit Status | Gap Severity | Phase 2 Priority |
|------------------|-----------------|--------------|------------------|
| **Persistent named bots** with durable state | Agents exist but sessions are ephemeral; no persistent bot identity in the left rail | **HIGH** | P0 |
| **Shared cloud computer** per account | Gateway sessions + workspace scopes exist; no shared persistent VM concept | **MEDIUM** | P1 |
| **Live screen preview** ("Agent Computer") | `ComputerUseView` + `ACIComputerUseSidecar` exist; not embedded in bot conversation | **HIGH** | P0 |
| **Human takeover flow** for auth steps | No structured takeover UI; approval system exists in automation | **HIGH** | P0 |
| **Skills** (reusable instruction sets) | No equivalent; routines exist for scheduled work | **MEDIUM** | P1 |
| **Routines** (scheduled/event-triggered) | ✅ Implemented: `RoutinesPanel`, `automation-api`, `RoutinesListView` | **LOW** | — |
| **Approval system** (allow once/deny/always) | Automation approval exists; not bot-scoped with rule persistence | **MEDIUM** | P1 |
| **Connectors** (`@` and `/` composer) | Plugins exist; no composer shortcuts for bot-scoped invocation | **MEDIUM** | P1 |
| **Bot-to-bot coordination** | Rails peer system enables messaging; not wired to bot UI | **LOW** | P2 |
| **Onboarding** ("Meet a future teammate") | No bot-specific onboarding; general onboarding exists | **MEDIUM** | P1 |
| **Use case templates** (8 packaged bots) | OpenMausBot templates on `ao/p1-openmausbot` branch; not on this branch | **HIGH** | P0 |
| **Bot screen** (isolated workspace per bot) | Surfaces exist (`chat`, `cowork`, `code`, `browser`, `design`); not bot-scoped | **MEDIUM** | P1 |
| **Recovery** (snapshot/restore/reset) | No bot state snapshot system | **LOW** | P2 |

### 3.2 What Allternit Already Has That Grok Bot Doesn't

| Allternit Capability | Advantage |
|---------------------|-----------|
| Multi-surface architecture (chat/cowork/code/browser/design) | Broader surface coverage than Grok's single computer view |
| Agent marketplace with ratings/installs | Grok Bot has no public marketplace |
| Rails peer system for agent-to-agent messaging | More structured than Grok's ad-hoc bot messaging |
| Cowork mode with plan/execute/review/report/automate/sync | More granular workflow modes |
| Swarm sub-modes (specialist-team) | No equivalent in Grok Bot |
| Computer use on local machine (macOS AX provider) | Grok is cloud-only |

### 3.3 Critical UX Gaps to Close

1. **No "Bot" as a first-class left-rail entity.** Users cannot see, switch between, or manage persistent bots from the shell rail. Sessions are surface-scoped, not bot-scoped.

2. **No embedded live screen in conversation.** Computer use exists as a sidecar/capsule but is not woven into the bot conversation flow as a takeover-able preview.

3. **No skill creation from successful processes.** Users cannot say "save what you just did as a skill" and have it persist for reuse.

4. **No structured approval UI in bot context.** Approval rules are automation-scoped, not bot-scoped with "allow once / deny / always allow" per-action.

5. **No composer shortcuts.** `/` for skills and `@` for connectors are not implemented in the Allternit composer.

---

## 4. Phase 2 Implementation Plan

### Phase 2A: Packaged Bot Sessions in Left Rail (P0)

**Goal:** Add persistent named bots as first-class entities in the ShellRail, with packaged templates and session launch.

**Steps:**

1. **Merge OpenMausBot Bots tab** from `ao/p1-openmausbot` into this branch. This provides the Bots tab in Agent Hub with 6 packaged bot templates.

2. **Add `BotSession` concept to session stores:**
   - New `useBotSessionStore` in `src/lib/agents/` with: `botId`, `botName`, `sessions[]`, `activeSessionId`, `memory` (persistent key-value), `skills[]`, `routines[]`.
   - Extend `ShellRail` session recents to group by bot when a session is bot-owned.
   - Add bot icon/avatar to rail session entries.

3. **Add "Bots" section to ShellRail:**
   - New rail section above session recents showing active/pinned bots.
   - Click bot → opens bot conversation view with embedded session.
   - "New Bot" button → opens Agent Hub Bots tab or inline creation.

4. **Wire bot sessions to gateway:**
   - `CreateAgentSessionRequest` already supports `agent_id`, `agent_name`, `session_mode`, `metadata`.
   - Add `bot_id` to session creation; gateway associates session with bot identity.
   - Bot memory persisted via `agent_features.metadata.memory`.

5. **Package 8 Grok-style use case templates:**
   - Sales Outbound, Talent Scout, Paid Media, Expense Manager, Product Performance, Bug Reproduction, Account Health, Chief of Staff.
   - Each template: name, job description, system prompt, tool scope, approval boundaries.

**Files to touch:**
- `src/shell/ShellRail.tsx` — Add Bots section
- `src/lib/agents/bot-session.store.ts` — New store
- `src/views/agent-hub/main/AgentHub.constants.ts` — Add `bots` tab
- `src/views/agent-hub/main/AgentHubContent.tsx` — Route bots tab
- `src/views/agent-hub/main/BotsTab.tsx` — New component
- `surfaces/ai.allternit.com/src/lib/agents/bot-templates.ts` — Template definitions
- Gateway: `cmd/allternit-api/src/agent_routes.rs` — Bot session endpoints

### Phase 2B: Live Screen + Human Takeover (P0)

**Goal:** Embed computer-use preview in bot conversation with structured takeover flow.

**Steps:**

1. **Add `BotScreenPanel` component:**
   - Embedded iframe/canvas showing bot's computer-use screen.
   - Controls: "Take Control" / "Return Control" buttons.
   - Status indicator: "Bot is working" / "Waiting for takeover" / "Paused".

2. **Implement takeover protocol:**
   - Bot emits `takeover_request` event with reason (auth, 2FA, CAPTCHA, payment).
   - UI shows banner + "Take Control" button.
   - On takeover: user's input routes directly to computer-use; bot pauses.
   - On return: user clicks "Return Control"; bot resumes from checkpoint.

3. **Secure secret input:**
   - Masked input field for passwords/secrets.
   - Values excluded from conversation transcript and model context.
   - Stored in bot's encrypted credential store (or session-only).

**Files to touch:**
- `src/components/bot/BotScreenPanel.tsx` — New component
- `src/components/bot/TakeoverBanner.tsx` — New component
- `src/components/bot/SecureSecretInput.tsx` — New component
- `src/lib/agents/bot-events.ts` — Event types
- Integration with `ACIComputerUseSidecar.tsx`

### Phase 2C: Skills System (P1)

**Goal:** Reusable instruction sets creatable from successful bot processes, invocable via `/` in composer.

**Steps:**

1. **Skill data model:**
   - `Skill { id, name, description, steps[], decisionRules[], safetyBoundaries[], createdAt, botId }`
   - Stored in `agent_marketplace_listings` or new `bot_skills` table.

2. **"Save as Skill" flow:**
   - Bot detects stable process completion.
   - User or bot triggers "Save as Skill" with name and scope.
   - Skill extracted from conversation transcript + tool calls.

3. **Composer `/` integration:**
   - Typing `/` in composer opens skill picker.
   - Selected skill injected into next message as system context.

4. **Skill sharing:**
   - Skills available across bots (with relevant connector checks).
   - Optionally publishable to agent marketplace.

**Files to touch:**
- `src/lib/agents/skill.store.ts` — New store
- `src/lib/agents/skill.types.ts` — Types
- `src/components/composer/SkillPicker.tsx` — New component
- `src/components/bot/SaveAsSkillDialog.tsx` — New component
- Composer integration in shell

### Phase 2D: Bot-Scoped Approvals + Connectors (P1)

**Goal:** Per-bot approval rules with "allow once / deny / always allow" and `@` connector invocation.

**Steps:**

1. **Bot approval rules:**
   - Extend automation approval to bot scope.
   - Rule types: `require_approval`, `always_allow`.
   - Match on: tool name, action type, target domain.
   - Stored per-bot in agent config.

2. **Approval UI in bot conversation:**
   - Inline approval card showing: operation, inputs, target, scope.
   - Three buttons: "Allow once", "Deny", "Always allow".
   - Desktop and mobile responsive.

3. **Composer `@` connector invocation:**
   - Typing `@` opens connector picker.
   - Selected connector attached to next message context.

**Files to touch:**
- `src/lib/agents/bot-approval.store.ts` — New store
- `src/components/bot/ApprovalCard.tsx` — New component
- `src/components/composer/ConnectorPicker.tsx` — New component
- Gateway: bot approval rule CRUD endpoints

### Phase 2E: Bot Onboarding (P1)

**Goal:** "Meet your new teammate" onboarding flow for bot creation.

**Steps:**

1. **Onboarding wizard:**
   - Step 1: Welcome + "What tools do you use?" survey.
   - Step 2: Suggest bots based on tool survey.
   - Step 3: Choose template or create custom.
   - Step 4: Configure name, job, description, approval boundaries.
   - Step 5: Launch first task.

2. **Integration with existing `OnboardingFlow`:**
   - Add bot onboarding as a branch when user first opens Bots section.

**Files to touch:**
- `src/components/bot/BotOnboardingWizard.tsx` — New component
- `src/components/onboarding/OnboardingFlow.tsx` — Add bot branch

---

## 5. Implementation Priority Summary

| Phase | Focus | Effort | Dependencies |
|-------|-------|--------|--------------|
| 2A | Packaged bot sessions in left rail | Large | Merge `ao/p1-openmausbot` |
| 2B | Live screen + human takeover | Large | Computer-use integration |
| 2C | Skills system | Medium | 2A (bot identity) |
| 2D | Bot-scoped approvals + connectors | Medium | 2A (bot identity) |
| 2E | Bot onboarding | Small | 2A (bot templates) |

**Recommended execution order:** 2A → 2B (parallel with 2E) → 2C → 2D

---

## 6. Risks and Open Questions

1. **OpenMausBot merge complexity:** The `ao/p1-openmausbot` branch may have diverged significantly. Merge conflicts in Agent Hub components are likely.

2. **Cloud VM vs local execution:** Grok Bot uses a persistent cloud VM. Allternit currently runs agents locally or via gateway sessions. Should Allternit bots run on a persistent cloud workspace, or adapt the local-first model? Recommendation: hybrid — local execution with optional cloud workspace sync.

3. **Bot memory persistence:** Grok's bot memory compounds across sessions. Allternit needs a decision on memory storage: SQLite (local), PostgreSQL (gateway), or a hybrid approach. Recommendation: gateway-managed memory with local cache.

4. **Security boundary model:** Grok explicitly states bots are NOT a security boundary (shared computer). Allternit should decide whether bots get isolated workspaces or share an environment. Recommendation: shared environment with per-bot approval rules (matches Grok's model).

5. **Skill extraction quality:** Extracting reusable skills from conversation transcripts is non-trivial. Grok requires manual refinement of auto-generated skills. Allternit should plan for a similar human-in-the-loop refinement step.

6. **Branch coordination:** Multiple P2 integration branches (`ao/p1-openmausbot`, `ao/p2-openmanus-audit`, `ao/p2-prime-agent-audit`) are in flight. The Grok Bot implementation plan depends on OpenMausBot merging first.
