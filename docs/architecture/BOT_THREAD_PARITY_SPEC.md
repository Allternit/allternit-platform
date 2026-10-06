---
doc: spec
updated: 2026-09-26
status: draft — for Eoj review, not approved
premise: ~/Downloads/Allternit_Bot_Thread_Subagent_Architecture.docx (architecture lock)
---

# Bot → Thread → Subagent: parity spec

## 0. Premise and reference

**Architecture lock (from the premise doc, unchanged):** Bots are durable
workers. Threads are isolated units of work. Subagents are temporary
delegated workers inside a thread. The Coordinator turns intent into a
dependency graph of Threads and assigns those Threads to the best Bots. State
and artifacts persist outside model context so every level can resume with a
clean context.

**Reference behavior** (what "1:1" means for the experience). Taken from two
recordings Eoj supplied; frames were reviewed:

- **Claude Code Projects** (ClaudeDevs, 2026-09-17): "one conversation with
  Claude. It splits the work into threads itself, runs them as parallel cloud
  sessions, passes context between them, and keeps going when you leave."
- **Claude Tag** (bcherny, 2026-09-22): a Slack message → Claude replies in a
  thread with a live task list, works for hours, posts PRs and decisions back
  into the same thread.

The rule for this spec: **nothing new gets built where Allternit already has
the part.** Every item below names the existing code it extends.

---

## 1. Object model: premise → Allternit today → gap

| Premise object | What exists in Allternit | Gap |
|---|---|---|
| **Project** | Cowork projects (`/cowork/projects`); chat sessions carry `project_id` (`agent_session_routes::create_session`) | A project is not yet the container a coordinator plans in; bots aren't members of a project |
| **Coordinator** | **Al** (`al_persona_routes.rs`): turns a request into a canonical A:// intent, resolves the target through delegation rules, submits it as delegator, reports on canonical run state. Zero capabilities by design | Al submits **one** intent per request. No intent → task-graph decomposition, no fan-out to several bots/threads, no synthesis pass |
| **Bot** | `agents` row (`is_bot`, `version`, `principal_id`) + `botProfile`; fabric principal `a://workspace/{ws}/bot/{id}` minted on create (V163); Computer Cloud desktop per bot (`bot_desktop_*`, `ensureBotComputer`); brain bind (`bot.brain`); connectors/secrets/identity channels (`BOT_AGENT_CONTRACT.md`) | Missing from the premise's Bot: cost limits, preferred machine as a real placement input, project memberships, historical performance |
| **Thread** | A gizzi chat session tagged `botThreadOf` / `botCanonicalFor` in server `session_metadata`; sub-threads + `+ New thread` (`bot-threads.ts`, `BotRailRows`); canonical pin | A thread is **one conversation**, not a durable work object. No objective, success criteria, status, checkpoint, summary, dependencies, or session generations. Canonical pin is browser-only |
| **Subagent** | Spawn-tool tree from the transcript (`bot-subagent-tree.ts`, `BotSubagentTree.tsx`); live child feed (`bot-subagent-feed.ts`, `GET /agents/:id/subagents`, ledger `parent_agent_id`) | No compressed result contract (status/summary/findings/artifacts/confidence/open_questions); no Subagent → Thread promotion |
| **Intent / task graph** | A:// intents → runs → jobs with `causation_chain`, leases, approvals (`FABRIC_TRANSPORT.md`); session DAG (`GET /cowork/sessions/:id/dag`, `use-a-dag.ts`); goal loop (`goal-loop-controller.ts`: Goal/Plan/Task/Attempt/Validation events); WIH partitions (`wih-session-contracts.ts`) | These are three partial graph models. One of them (A:// runs/jobs) must become **the** task graph; the others project from it |
| **Memory scopes** | Global/user: memory kernel (`/memory/*`). Principal-scoped with owner + grants: `/cowork/memory` (V165). Beta memory stores bound to sessions. Client `bot-memory-store.ts` has the right contracts (isolation, provenance, promotion, redaction) but is **in-memory only** | No Project scope, no Thread working-memory scope, no explicit promotion path between scopes; bot memory lost on reload |
| **Artifacts** | Run deliverables (`/cowork/runs/:id/deliverables`), content artifacts, `bot-assets-api.ts`, inline artifact renderer | Not attached to a Thread object; no per-thread artifact count for the thread row |
| **Event ledger** | `bot_events` (V180) with per-bot seq, idempotency, operational-state fold; cowork run events with attribution | No `thread.*` / `subagent.*` / `memory.promoted` / `decision.requested` event types; ledger is per-bot, not per-thread |
| **Routines** | **Automation Tasks** is canonical: `routines` table (`agent_id`, `execution_domain` local/cloud, `routine_runs`, metrics) behind `/automation/routines`; `BotRoutinesPanel` already uses it via `RoutinesListView` | A duplicate browser-only store (`bot-routine.service.ts` + `use-routine-timer.ts`) is still used by Bot Home, the rail, composer, team import, presence. Monitor routines call a tool id (`shell`) the API does not serve |
| **Group chat** | `GroupChatSessionView`, `group-chat-turn-runner.ts` (each member bot replies under its own identity), `bot-group-store.ts` (swarm strategies), channel dialog | Group chat is a room where bots talk; it is not wired to threads (an @mention in a group should be able to start a thread, like Tag in a channel) |
| **Placement (BYOC)** | `routines.execution_domain`; fabric compute capabilities (`compute.local` / `compute.vm` / cloud worker), desktop-managed fabric worker, cloud continuation | No per-bot default placement; background agentic jobs run a generic loop, not **as the bot** (`gizzi-code/.../fabric-transport/agentic.ts` uses a default model and its own tool set) |

---

## 2. Status grammar (one vocabulary, three views)

Premise grammar: QUEUED · PLANNING · WORKING · BLOCKED · NEEDS YOU · REVIEW ·
DONE · FAILED · PAUSED.

| Premise | Existing operational status (`bot_event_routes` fold, `orpc-contracts.ts`) | Threads panel group (Projects reference) |
|---|---|---|
| NEEDS YOU | `waiting_approval`, `waiting_input` | **Waiting on you** |
| BLOCKED | `blocked` | **Waiting on you** (blocked on a decision) |
| QUEUED / PLANNING / WORKING | `working` (+ new `queued`, `planning`) | **Working** |
| REVIEW | new | **Waiting on you** |
| PAUSED | new | **Idle** |
| DONE | `completed` | **Resolved** |
| FAILED | `failed` | **Resolved** (red) |
| — | `idle`, `offline`, `degraded` | **Idle** |

Status is computed server-side per **thread** (new) and rolled up per bot
(existing fold). The UI never derives status from transcript text.

---

## 3. Experience spec (what the user sees)

Everything here reuses the shared `components/bot-chat/*` set so Desktop,
web, PWA, and iOS stay identical.

### 3.1 Coordinator conversation (the Project chat)

1. User writes one request. Under the user bubble: **"Sent to N threads"**
   receipt (small, tertiary, with a fork icon).
2. Coordinator reply: one line ("On it. Three threads; I'll flag anything that
   needs you.") followed by **thread chips**, one per thread: status dot ·
   title · live one-line activity ("Bisecting · 7 commits left"). Chips update
   in place from the event stream.
3. A follow-up message is routed to the right thread. Receipt: **"Sent to one
   thread"**. Reply: "Sent your message over to **[● Draft release notes]**" —
   an inline thread pill; hover shows a card (status · replies · time) and
   click opens the thread.
4. When a thread needs a decision: a **decision card** in the coordinator
   chat — raised-hand icon, thread title, the question, **View thread**.
   Reuses `ApprovalCard` / `WaitingOnYouPill` styling.
5. Thread completions post back as short coordinator lines ("The p99 thread
   is done: PR #4821 is up and CI is green.") — compressed results, never the
   thread transcript.
6. The **fan-out** view: the intent graph that produced the threads (nodes =
   threads, edges = dependencies, bot avatar per node). Rendered from the
   existing session DAG endpoint once it reads the canonical task graph.

### 3.2 Threads panel (right side / Bot Home)

- Header: greeting + "Nothing is waiting on you" / "1 thread is waiting on
  you". Tab badge with the waiting count.
- Collapsible groups with counts: **Waiting on you · Working · Idle ·
  Resolved** (§2).
- Row: title · live status line · **progress ring k/n** (the thread's todo
  steps) · artifact badge (PR / doc count) · relative time. Rows move between
  groups live.
- Scope switch: all threads in the project, or one bot's threads (the
  existing bot rail sub-threads become this list filtered by bot).

### 3.3 Thread view

- Breadcrumb **Threads › {title}**, actions: pin · resolve · expand · more.
- **Todo checklist card** at the top of the thread's work: done / active /
  pending rows (the k/n in the row). Edited in place, stamped "as of 10:05 PM
  (just now)" like the Tag thread.
- **Provenance line** for routed messages: "Message forwarded from project
  chat ›".
- **The "New" divider** — the ripped-paper line: a wavy rule labelled
  **New**, drawn (a) above activity that arrived since the user last read the
  thread and (b) at every **session generation** boundary (the thread moved
  to a fresh context window after a checkpoint). The generation variant
  carries the time and a one-line checkpoint summary. Extends the existing
  `GapTimestamp` / `SystemLine` components rather than a new one.
- Composer placeholder **"Steer this thread…"**; per-thread model pill
  (existing `BotModeModelPicker`).
- Subagents render nested under the step that spawned them
  (`BotSubagentTree`), collapsed to their compressed result by default, full
  transcript on expand.

### 3.4 @mention anywhere (Tag parity)

- `@Bot` in any chat, group chat, or channel starts a **thread owned by that
  bot**, replying inline under the message: live task list, then results,
  PR/artifact links, and approval asks — all in that thread.
- Footer on bot messages: bot · model · **Configure** (opens the bot's
  config), like "Lens · Opus 5.5 · Configure".
- The same thread appears in the bot's thread list and the Threads panel.
  One object, many windows.

### 3.5 Bot Mode becomes Threads — surface-by-surface redesign map

Bot Mode sessions *are* threads now. Every current surface is mapped; none
is left as a parallel path. UI/UX bar: this is the product surface — each
screen gets a design pass against the reference frames and the Allternit
tokens (white + `--neutral-fill`, no tan surfaces) before build, and is
verified live in the desktop build with screenshots.

| Current surface (file) | What it is today | Becomes |
|---|---|---|
| **Shell rail · Bots section** (`ShellRail.tsx`, `BotRailRows.tsx`: `BotRailRow`, `BotGroupRailRow`, `BotNeedsYouHint`) | Contact-style bot rows, sections, sub-threads under a bot, `+ New thread` | Project switcher on top; bots with a live status dot and a waiting count; expanding a bot shows its threads grouped **Waiting on you / Working / Idle** (Resolved behind "Show all"); thread rows carry the k/n ring. Group rows stay |
| **Bots hub** (`BotLaunchpadView.tsx`, `BotTopDeck.tsx` "To:" bar) | Hub with To: picker, roster/inbox framing | **Project home**: coordinator conversation (§3.1) on the left, **Threads panel** (§3.2) on the right, "Welcome back" header with the waiting count. The To: bar becomes the target selector — the project coordinator, or `@` a specific bot |
| **Bot session** (`BotChatSessionView.tsx`: header + `BotModeModelPicker`, `BotRailsDeck`, `BotWatchStrip`, `BotTranscript`, `BotComposer`, `BotComputerViewport`) | One chat per bot session | **Thread view** (§3.3): breadcrumb Project › Bot › Thread, status + k/n, pin/resolve; todo card; transcript with rip dividers per generation; subagents nested inline; **Steer this thread…** composer; model pill. `BotTranscript` / `BotComposer` stay as the body |
| **Thread inspector** (new right sidecar; absorbs `BotWatchStrip`, `BotRailsDeck`, `BotComputerViewport`, the Agent Context Strip drawers) | Scattered strips above/beside the chat | One sidecar with tabs: **Activity** (thread event timeline), **Subagents** (tree with status, compressed result, transcript on expand — `BotSubagentTree` + subagent feed), **Artifacts**, **Memory** (thread working memory, promotions), **Computer** (live desktop / takeover), **Details** (objective, success criteria, checkpoint, generation history, placement, budget used) |
| **Bot detail** (`BotHomeView.tsx`, 2,092 lines, tabs `chat · tasks · runtime · config`) | Mixed home/chat/task/runtime page | **Redesign.** Header: identity, role, status, placement (this Mac / cloud / own server), spend vs limit. Tabs: **Threads** (panel scoped to the bot; replaces `chat` + `tasks`), **Routines** (Automation Tasks filtered to the bot), **Memory** (bot scope + promotions), **Computer**, **Performance** (thread outcomes over time), **Config** (Edit bot). The 2k-line file splits per tab |
| **Group chat** (`GroupChatSessionView.tsx`, `GroupChatComposer.tsx`, `group-chat-turn-runner.ts`) | Multi-bot room, each bot replies as itself | A group is a channel: `@Bot` under a message starts a **task thread** for that bot, shown as a thread chip with a live task list inline (§3.4). Group discussion turns stay |
| **Bot inbox** (`BotInboxView.tsx`, `bot-inbox.ts`) | Bot-to-bot mail | Inbound messages become routed messages into the target thread ("Message forwarded from …" provenance). Inbox view becomes a filter: threads with unread routed messages |
| **Computer / remote desktop** (`BotDesktopView`, `BotComputerWindow`, `BotComputerViewport`, `useBotActiveVm`, watch strip) | Per-bot computer, watch/takeover | Unchanged capability; surfaced in the thread inspector's Computer tab and as an inline "using computer" row in the thread when a turn drives it |
| **Approvals** (`ApprovalCard`, `ApprovalPill`, `WaitingOnYouPill`) | Inline approval cards, roster pill | Drive the **Waiting on you** group and the coordinator decision cards; same components |
| **Fabric PWA / phone** (`FabricBotMode.tsx`, `FabricSessionPanel`) | Phone bot mode on shared `bot-chat` components | Same Threads panel + Thread view, stacked (panel → thread), same components |
| **iOS** | Bot chat on the shared wire contract | Same object model over the API; layout pass after desktop lands |
| **CLI** (`gizzi bot`) | Canonical session per bot | `gizzi bot threads` / `gizzi thread steer`; canonical session = the bot's standing thread |

### 3.6 Existing surfaces that stay as they are

Chat · Bots · Code · ACI shell switch; bot rail rows with sections, role
badges and sub-threads; `BotChatSessionView` (becomes the Thread view body);
`GroupChatSessionView`; `BotComputerWindow` / remote desktop and the watch
strip (a thread using the bot's computer shows it inline); approvals; iOS /
PWA via the shared `bot-chat` components; `gizzi bot` CLI.

---

## 4. Backend work (by layer; each says exists → extend)

### 4.1 Thread as a durable object
- New server table **`bot_threads`**: `id, kind (standing|task), incognito,
  bot_id, project_id, parent_thread_id, objective, success_criteria, status, todo (json), summary, checkpoint,
  current_session_id, created_by (user|coordinator|bot|mention), origin
  (message ref), started_at, last_activity_at, resolved_at`.
- **Session generations**: `bot_thread_sessions (thread_id, generation,
  session_id, model, context_window, tokens_used, reason
  (budget|model_switch|routine_run|manual), started_at, ended_at,
  checkpoint_summary)`. The handoff rule (D3) is computed per model: at a
  set fraction of the active model's window, or on a switch to a smaller
  window, write the checkpoint and roll to a fresh gizzi session seeded with
  objective + checkpoint + working memory. Each row is one rip in the UI. Existing gizzi sessions become generation 1 of
  an auto-created thread (migration from `botThreadOf` metadata).
- Canonical pin moves server-side (a field on the bot, not a new table).
- Endpoints: list/create/update/resolve threads; steer (post into current
  generation); thread events stream.

### 4.2 Coordinator (Al) fan-out
- Extend Al from one intent to an **intent → task graph** step: nodes with
  dependencies, each node assigned to a bot by specialization (delegation
  rules + bot roles), instantiated as a thread. Independent nodes start in
  parallel; dependent nodes wait.
- Follow-up routing: classify a user message to the thread it belongs to
  ("Sent to one thread") or start new threads.
- Synthesis: completed thread summaries flow back as coordinator messages.
- Al stays zero-capability (plans, delegates, monitors, synthesizes — never
  does the work), per the premise §9 and `AL_IMPLEMENTATION_SPEC`.
- One graph model: A:// runs/jobs + `causation_chain` is the task graph; the
  goal loop and WIH become projections of it (not deleted until the
  projection covers their consumers).

### 4.3 Bot executes its own threads (placement / BYOC)
- A thread turn runs **as the bot**: the bot's prompt, model/brain, tools,
  memory, computer. Background fabric jobs targeting a bot principal load
  that profile instead of the generic agentic loop.
- Placement per bot (default) and per thread (override): this Mac (desktop
  fabric worker), Allternit cloud, or the user's own server/VPS. Feeds the
  existing `compute.*` capability placement and `execution_domain`. Both are
  always available; the user picks.

### 4.4 Subagents
- Compressed result contract `{status, summary, findings[], artifacts[],
  confidence, open_questions[]}` returned to the parent thread; transcript
  kept for inspection only.
- **Promotion**: subagent → thread when work is long-running, user-visible,
  resumable, artifact-heavy, interactive, or has its own dependencies. Max
  visible depth Project → Bot → Thread → Subagent.

### 4.5 Memory scopes
- Global (memory kernel) · Project (new scope on cowork memory) · Bot
  (`/cowork/memory` principal scope — exists) · Thread (working memory on
  the thread object) · Subagent (ephemeral).
- Explicit promotion API + `memory.promoted` event. Client
  `bot-memory-store.ts` becomes a cache over the server scopes.
- Curation pass ("dreaming"): scheduled consolidation per bot using the
  existing `/memory/consolidate`.

### 4.6 Routines (converge, don't add)
- Retire `bot-routine.service.ts` + `use-routine-timer.ts`; Bot Home, rail,
  composer, team import, presence use Automation Tasks (`agent_id` = bot);
  one-time import of browser-stored routines.
- A routine attaches to a **standing thread**; each run is a new generation
  in it (D2), and a run that finds real work spins off a task thread.
- Monitor mode as an Automation routine config; fix `shell` → `shell.exec`.
- Self-scheduling tool: the bot creates its own follow-up as an Automation
  routine (`created_by: bot`), visible in Automation Tasks.
- **Remove** the uncommitted `bot_routines` table / tick written earlier in
  this session (worktree `allternit-wt-botruntime`) — it duplicated
  Automation Tasks.

### 4.7 Events
- New types on the ledger: `thread.created|started|checkpointed|blocked|
  needs_user|completed`, `subagent.spawned|tool_call|completed|promoted`,
  `artifact.created`, `memory.promoted`, `decision.requested|resolved`.
- Ledger gains `thread_id`; operational state computed per thread and rolled
  up per bot. The UI is a projection of this stream.

### 4.8 Governance
- Per-bot spend limit (gateway budgets bound to the bot principal); per-
  thread budget from the coordinator's plan.
- Task log: every thread and run with who asked (ledger initiator exists).
- Tool/connector scoping per surface or channel; admin audit across bots.

### 4.9 Group chat and channels
- An @mention in a group chat starts a thread for the mentioned bot, shown
  inline under the message (§3.4). Group turn runner keeps its role for
  multi-bot discussion.
- External channels (email / phone / Slack-style, via identity channels)
  enter the same way and reply in the thread.

---

## 5. Build order

Each phase ships end to end (backend + wiring + UI) and is checked live in
the desktop build, per the one-current-build rule.

1. **Converge**: routines onto Automation Tasks; canonical pin server-side;
   bot memory onto server scopes; remove the duplicate `bot_routines` work;
   A:// SDK (types + coordinator toolset over the existing HTTP surface).
   Design pass: annotated screenshots of every §3.5 surface today → target
   mocks against the reference frames, signed off before UI build.
2. **Thread object**: `bot_threads` + generations, migration from existing
   sessions, thread status + events; Threads panel (§3.2) and Thread view
   (§3.3) including the New divider and todo card.
3. **Bot runs its threads**: execute as the bot, placement choice, steer.
4. **Coordinator fan-out**: task graph, parallel threads, routing receipts,
   decision cards, completion synthesis, fan-out view (§3.1).
5. **@mention threads** in chats, group chats, channels (§3.4).
6. **Subagent contract + promotion**; memory promotion + curation.
7. **Governance**: spend limits, task log, scoping, audit.

## 6. Decisions (Eoj, 2026-09-26)

### D1 — The Project coordinates (resolved)
The Project is the coordinator's container. Coordination is deterministic:
the model may *propose* a plan, but the task graph, bot assignment
(delegation rules + role + capability eligibility), dependency gating, and
routing are persisted system decisions, not separate chat entries. Every
thread belongs to a project; a bot's direct chat lives in the bot's default
project.

### D2 — Standing threads and task threads (proposed, needs Eoj sign-off)
Two kinds of thread, one object:

- **Standing thread** — long-lived line of work for a bot ("Kiwi · inbox
  triage", a bot's canonical chat, a routine's home). Never "done"; it rolls
  through context generations (D3).
- **Task thread** — bounded work with an objective and success criteria;
  resolves. Created by the coordinator's fan-out, by an @mention, by a user,
  or spun off from a standing thread when a piece of work outgrows it
  (same promotion rules as subagent → thread, §4.4).

A task thread records `parent_thread_id` when a standing thread spun it off,
and the coordinator can steer it like any other thread. In the panel it is a
sibling row labelled "from {standing thread}", not a deeper nesting level —
the visible hierarchy stays Project → Bot → Thread → Subagent.

**Routines** attach to a standing thread. Each run is a new context
generation inside it (rip divider "Routine · Mon 9:00 AM"), carrying forward
the checkpoint, not the transcript. A run that finds real work spins off a
task thread instead of growing the standing one. This avoids both failure
modes: one thread per run (floods the panel, loses continuity) and one
ever-growing conversation (context rot).

### D3 — Handoffs, compaction, the rip, incognito (resolved direction)
- **Context budget per model.** Each thread generation tracks tokens against
  the active model's context window (from the model catalog). At a set
  fraction of the window, or when the user switches to a model with a smaller
  window, the system writes a checkpoint (objective, decisions, open items,
  working memory, artifact refs) and starts a fresh generation seeded from
  it. Compaction inside a generation still happens; the handoff is the
  regular, calculated reset that keeps token counts down.
- **The rip.** Every generation boundary renders as the torn-paper divider
  with the time and what carried over ("Fresh context · 10:02 AM · carried 4
  decisions, 3 open items"). Unread-since-last-visit uses the same divider
  labelled **New**.
- **Incognito asks.** A one-off question inside a thread (or standalone)
  that does not enter the thread's context or memory. It may *read* the
  thread checkpoint; nothing it produces is written back unless the user
  pins it. Built on the existing ephemeral session support (`ephemeral:
  true`, `agent_session_routes::create_session`). Shown as a side answer, not
  in the transcript.

### D4 — A:// is the canonical graph (resolved) — what exists
Findings (2026-09-26):
- **There is no A:// SDK.** What landed: the contract
  (`docs/learnings/A_COORDINATION_CONTRACT_V0_1.md`), schema, conformance
  matrix + tests, developer guide (`docs/development/A_PROTOCOL_DEVELOPER_GUIDE.md`),
  the Rust runtime (`allternit-cowork-runtime`: transport, store, risk
  policy), the gizzi fabric worker, and Al v0.1 (one intent per request,
  deterministic delegation rules, V166).
- `AL_IMPLEMENTATION_SPEC` marks the Al **plan/decompose loop as Planned**.
- The contract already requires convergence: "all trigger sources should
  converge into the same canonical Intent/Run pipeline" (schedules,
  webhooks, Factory peer messages, connector events). Today goal loop, Factory
  WIH/rails DAGs (`BotRailsDeck`), the cowork session DAG, and Automation
  routines each run their own path.

Direction: A:// runs/jobs + `causation_chain` is the graph. Build the
missing **A:// SDK** (TypeScript + Rust types over the existing HTTP
surface) and give the coordinator its toolset through it:
`plan_graph`, `create_thread`, `assign`, `await_threads`, `route_message`,
`request_decision`, `synthesize`, `schedule` (Automation routine),
`message_peer` (Factory peers), `read_dag`. Goal loop, WIH, and rails DAGs become
consumers of (or projections over) the canonical graph, retired only once
their callers are covered.

---

## 7. Current state audit (Eoj's recordings + screenshots, 2026-09-26)

Source: two screen recordings and three screenshots of Allternit Desktop
1.1.3, Bots mode. Root causes are from reading the code; items marked
*suspected* still need a live repro.

### 7.1 Broken

| # | What Eoj saw | Root cause | Fix |
|---|---|---|---|
| B1 | **Create bot fails** on the last step: `Invalid enum value … received 'claude-cli'` at `provider` | The model picker offers any gizzi provider (`claude-cli`, `codex-cli`, `openrouter`, …) but provider is a closed `z.enum` in five places: `lib/agents/agent.types.ts:723` and `:917`, `lib/bots/bot-contract.ts:102`, `lib/bots/bot-workspace-contracts.ts:142`, `lib/bots/orpc-contracts.ts:126/142`. The default model (Claude CLI) is rejected | Provider becomes an open string validated against the live provider catalog (the same one the picker reads); one shared schema instead of five copies. Test: create with every default |
| B2 | **Mascot avatar can't be selected** | *Suspected*: the mascot grid (`steps/AvatarEditor.tsx:387`) shrinks each preview with a CSS `scale-[0.55]` transform, which does not shrink its layout box — the full-size preview overflows its button and can sit on top of neighbours, so clicks land on the wrong target. Wiring (`setMascotTemplate`, `avatar-config.ts` → `type: "mascot"`) looks correct | Size the preview instead of transforming it; confirm in the live app |
| B3 | **"Bot not found."** after editing and saving the bot | `BotHomeView.tsx:301` shows it when the bot is missing from the store *or* `isBot(bot)` is false. *Suspected*: the save path writes the agent back without `isBot` / `botProfile`, or the view loses its `botId` context | Trace the Edit bot save payload; the bot view must resolve by id from the server, not only the local store |
| B4 | **"Computer's computer"** and a black window on Expand computer | `BotComputerWindow.tsx:18-27` renders a placeholder bot named "Computer" until the agent store loads in the new window | Load the bot by id before rendering; show a loading state, never a fake name |
| B5 | **Webhook error**: `Unexpected token '<', "<!doctype"… is not valid JSON` | The webhook receiver-port call hits a route the API doesn't serve, so it gets the app's HTML page back | Point it at the served route, or hide the card when the API has no webhook receiver |
| B6 | **Inbox: "Agent messaging is not connected"** (`NEXT_PUBLIC_ALLTERNIT_FACTORY_API=1`) | Factory messaging is off in the desktop build | Folds into §3.5 (inbox → routed thread messages over the API) |
| B7 | **Model shown three different ways**: config says `openai/gpt-5-mini`, chat header says "OpenRouter · Model" (setup required) and later "Pi · Model" | The bot's model, its brain bind, and the session picker are separate sources | One source: the thread's model, defaulting to the bot's. Unavailable models are dimmed with the reason, not silently shown |
| B8 | **Gizzi has no computer** ("Provision computer"), despite the atomic-create rule | The default Gizzi bot predates atomic create; nothing backfills it | Provision on first use or during migration; never show an empty computer panel as the default state |
| B9 | **Two routine systems on one tab**: "Simple routines" (browser-only store) above the Automation Tasks list | See §4.6 | Converge on Automation Tasks |
| B10 | **"The roster" hub is still a Chat-launch clone**: ModeDock Chat/Cowork/Bots, "Build a site / Make a deck…" chips, featured template cards | The Sept-15 punch list item P0-B was never finished; also a second hub (**Bot Hub**, category tabs, Bots/Sessions switch) exists | Both are replaced by Project home (§3.5) |

### 7.2 Too much: the Bot Home mega view, block by block

`BotHomeView.tsx` (2,092 lines) has 5 header buttons (Inbox, Cloud,
Settings, Chat, Run Task) and 4 tabs. Several blocks appear twice (Inbox,
Bot settings, Workspace & Config are in both Runtime & Desktop and Data &
Config). Every block gets exactly one home:

| Block today | Goes to |
|---|---|
| Header: avatar, name, tagline, "Bot · cloud harness" chips | Bot detail header, with status, where it runs, and spend |
| Header buttons: Chat, Run Task | One **New thread** button (chat and task are both threads now) |
| Header button: Inbox | Threads panel filter "routed to this bot" (§3.5) |
| Header button: Cloud (Cloud Orchestration modal: Cloud Harness / Sandbox Runtime) | The "where it runs" setting in the header (this Mac / cloud / own server) |
| Header button: Settings | Config tab |
| **Chat & Sessions** tab: "Delegate work" card (New Project / Chat / Run Task) | New thread button + Project home |
| Stat cards: Tasks, Artifacts, Connectors, Secrets | Threads tab counts (tasks), Artifacts in the thread inspector and bot Performance tab, Connectors/Secrets in Config |
| Webhook triggers card | Routines tab (a webhook is a trigger, like a schedule) |
| Welcome message + Quick tasks | Empty state of a new thread with this bot (starter prompts as chips) |
| **Tasks & Automation** tab: Tasks list | Threads tab |
| Simple routines composer | Routines tab, backed by Automation Tasks |
| Automation list (Goals / Routines / Loops / Agent Heartbeats) | Routines tab (Goals become coordinator plans once the A:// graph lands) |
| Webhook Triggers + receiver port | Routines tab → Triggers section; receiver port moves to app settings |
| **Runtime & Desktop** tab: Connectors, Secrets | Config tab → Access |
| Harness, Brain, Model, Provider routing | Config tab → Model & runtime (one picker, B7) |
| Identity Channels (email, phone, wallet) | Config tab → Channels |
| Virtual Computer + Desktop | Computer tab (and inspector Computer tab inside a thread) |
| **Data & Config** tab: Artifacts | Performance / artifacts view |
| Workspace & Config (AGENTS.md, SOUL.md, USER.md, TOOLS.md, HEARTBEAT.md; Personality, Voice, Connected Apps, Tools) | Config tab → Files & personality |
| Import team / Edit bot profile | Config tab actions |

Result: header + six tabs (Threads · Routines · Memory · Computer ·
Performance · Config), no duplicates, one entry per action.

### 7.3 Create bot, simplified

Today: 4 steps (template grid → long identity form with 6 avatar modes and
~30 sprite-style chips → job + 8 tool toggles → computer size, provider,
resources, persistence, brain type, native harness choice + session id,
knowledge brain, model/provider/harness fields, Advanced). Too many
decisions before the bot exists, and one bad default blocks the whole thing
(B1).

Proposed: **one screen, three required things, everything else later.**

1. **Name — Role** (e.g. "Quinn — Chief of Staff"). Handle derived.
2. **What it does**, one or two sentences. Drafts the job instructions and
   a default tool set; template chips above the field fill it in one click
   (replaces the template grid step).
3. **Look**: Gizzi in the accent colour by default; "Change look" opens the
   avatar picker (Gizzi · Mascot · Sprite packs · Image) as a sheet.

Collapsed **More options**: tools, where it runs (this Mac / cloud / own
server — replaces computer size + brain type + harness + provider fields),
model (the one live-catalog picker; can't pick an invalid provider).

Create returns immediately and opens the bot's first thread; the computer
provisions in the background with visible status (atomic-create rule kept).
Everything skipped is editable later in the Config tab.

---

## 8. Work plan (tracked)

Status: `[x]` done · `[~]` in progress · `[ ]` not started. Every task ships
backend + wiring + UI together, is verified with tests, and UI tasks are
checked live in a main Desktop build (one-current-build rule).

### P0 — Stabilize what's broken (§7.1)
- [x] P0.1 Create bot accepts any catalog provider (B1) — allternit-ai #105
- [x] P0.2 Mascot selectable (B2) — allternit-ai #105
- [x] P0.3 Edit bot keeps the bot (B3) — platform #794 (config merge) + allternit-ai #105 (isBot)
- [x] P0.4 Computer window loads the real bot (B4) — allternit-ai #105
- [x] P0.5 Webhook triggers reachable (B5) — platform #794
- [x] P0.6 Live verify P0.1–P0.5 in the next main Desktop build (Agent B building from ai 9cdebeeb / platform b2a71fd67)

### P1 — Converge (no duplicate systems)
- [x] P1.1 (platform #796, allternit-ai #109; live-verified on b3914) Routines: retire `bot-routine.service.ts` + `use-routine-timer.ts`; Bot Home, rail, composer, team import, presence read/write Automation Tasks (`agent_id`); one-time import of browser-stored routines; monitor mode as an Automation routine config using `shell.exec` (B9)
- [x] P1.2 (same PRs) Canonical thread pin stored on the bot (server), not localStorage
- [~] P1.3 (platform #813, allternit-ai #142) Bot memory on server scopes (`/cowork/memory`, principal = bot); `bot-memory-store.ts` becomes a cache
- [ ] P1.4 One model source per thread/bot; unavailable models dimmed with the reason (B7)
- [ ] P1.5 Gizzi (and any pre-atomic bot) gets its computer on first use (B8)
- [ ] P1.7 Converge the other browser-side schedulers (`lib/agents/agent-cron-scheduler.ts`, `lib/agents/scheduled-jobs.runner.ts`) onto Automation Tasks + the local scheduler, same as P1.1
- [x] P1.6 A:// SDK: TypeScript client + types over the existing HTTP surface (principals, intents, runs/jobs, approvals, DAG, memory grants); Rust types from `allternit-cowork-runtime`

### P2 — Design pass (sign-off before any P3 UI)
- [x] P2.1 Target mocks (https://claude.ai/artifact/X8fseo967PQQZDuJ1gWN1C — current; earlier copy TrCk8AYYVxhFA6DdFoejqy is in the other org), annotated against the reference frames: Project home (coordinator + Threads panel), Thread view (todo card, rip/New divider, steer composer, nested subagents), Thread inspector, Bot detail (header + 6 tabs), Create bot (one screen), group-chat @thread
- [x] P2.2 Eoj signed off 2026-09-27 ("I love the mock up"); mocks are the acceptance screenshots for P3

### P3 — Thread object + Threads UI
- [x] P3.1 (#797) `bot_threads` + `bot_thread_sessions` tables and API; kinds standing/task; incognito flag
- [x] P3.2 (#797, lazy sync) Migrate existing bot sessions into threads (generation 1)
- [x] P3.3 (platform #805, allternit-ai #132) Per-model context budget → checkpoint → new generation (handoff); compaction within a generation
- [x] P3.4 (#797, #798) Thread status + `thread.*` events on the ledger; per-thread operational state rolled up per bot
- [x] P3.5 (allternit-ai #114) Threads panel (groups, rows, k/n ring, artifact badge)
- [~] P3.6 (allternit-ai #114: bar, rip, plan card; provenance + steer placeholder + model pill next) Thread view (breadcrumb, todo card, rip dividers, provenance, steer composer, model pill)
- [x] P3.7 (allternit-ai #118) Thread inspector (Activity, Subagents, Artifacts, Memory, Computer, Details)
- [~] P3.8 (platform #799, allternit-ai #121: Bots launch + project migration; rail regrouping + Project home coordinator chat + bot project page on server next) Rail regrouping + **Bots launch** (no project open: "What should the team work on?" composer to Al, team strip with live status, projects with waiting/working counts, standing threads, first-run create-bot block) and **Project home** (inside a project) replace "The roster" and Bot Hub (B10). No template gallery, no Chat/Cowork/Bots dock on the Bots launch
- [~] P3.9 (allternit-ai #142) Bot detail rebuild: split `BotHomeView.tsx` into header + Threads · Routines · Memory · Computer · Performance · Config (§7.2)
- [ ] P3.10 Create bot one-screen flow (§7.3)
- [x] P3.11 (allternit-ai #157) Incognito asks
- [x] P3.12 (Agent B, allternit-ai #116) Extract the deck (artifact-mode picker, plugins, starter pills, template gallery) from `ChatComposer` into one shared component; no second copy
- [x] P3.13 (Agent B, allternit-ai #116) Cowork launch is the deck's native home: full deck + artifact template gallery; "Put a team on it" hands off to the Bots launch as a new project. Chat launch keeps its deck; the Chat/Cowork/Bots switcher stays on Chat and Cowork composers only
- [ ] P3.14 Thread composer gets the compact deck (mode ▾ · plugins ▾ · model); artifacts made in a thread open in Cowork
- [~] P3.15 (allternit-ai #121; Hire prefill with P3.10) Templates on the Bots launch by progressive disclosure: level 0 = three "Start from a team" cards with a mini plan graph; level 1 = "Browse templates" panel with **Teams** (orchestration use cases: bots + plan with dependencies, "uses your Scout · adds Pixel") and **Bots** (single-bot marketplace; opens Create bot prefilled, nothing created until confirmed). Visual bar: the mockup, not a card grid of icons

- [x] P3.16 (platform #808, #816; allternit-ai #147; gizzi TUI divider with the TUI owner) Session lineage + context handoff native in gizzi-code for every mode (Chat, Cowork, Code, gizzi CLI, bots): `session.continues_from` / `session.handoff`, `SessionHandoff` (baton on the session's own model, TODOs carried), automatic between turns at 70% of the usable window and before a turn that switches to a smaller window, prompts to a closed window land on the lineage head, `POST /session/:id/handoff` + `GET /session/:id/lineage`, `session.handoff` bus event. Bot threads call gizzi's handoff and record gizzi-initiated windows as generations (server: this PR; next: rip + follow-the-head in the desktop Chat/Cowork/Code views and a divider in the gizzi TUI)
- [x] P3.17 (platform #821, #825; allternit-ai #150, #155; TUI/HUD by the TUI owner) (gizzi `session/pause.ts`, Codex + Claude quota readers, API relay/resume/thread pause; UI next) Land before 5-hour / weekly / rate limits for any provider and auto-resume at reset: signals from gizzi `providers/quota` (add Claude subscription + Codex fetchers), `session/retry.ts` retry-after and gateway budgets; land at a clean point with a `quota` handoff, "Paused until … · <limit>", server tick resumes; falling back to another model is opt-in only

### P4 — Bots run their own threads
- [~] P4.1 (server-started bot turns carry the bot's instructions + saved memory; handoff seeds and resumes keep them) Fabric jobs targeting a bot principal load the bot's prompt, model, tools, memory, computer
- [~] P4.2 (platform: session_placements, passthrough, bot placement API, relayed sync; UI next) Placement per bot / per thread: this Mac, Allternit cloud, own server
- [x] P4.3 (allternit-ai #157; gizzi queued turns) Steer and interrupt a running thread
- [x] P4.4 (routine's own standing thread, a generation per run via gizzi handoff) Routine runs = new generation in their standing thread; spin-off task threads

### P5 — Coordinator fan-out (A://)
- [x] P5.1 (platform #837 + this: validated plan mirrored to the rails DAG, nodes + blocked_by edges, thread status moves nodes) Al plan loop → task graph through the A:// SDK (deterministic assignment, dependency gating)
- [x] P5.2 Fan-out to threads; "Sent to N threads" receipts; live thread chips
- [x] P5.3 Follow-up routing ("Sent to one thread") and thread pills with hover cards
- [x] P5.4 (platform #837, allternit-ai #162) Decision cards; completion synthesis back to the project chat
- [x] P5.5 (allternit-ai #162, Graph tab) Fan-out (intent graph) view from the canonical DAG
- [x] P5.6 (bot plans are rails DAGs; goal loop / WIH / rails views read them via /dags) Goal loop / WIH / rails DAGs read from the canonical graph

### P6 — @mention threads
- [x] P6.1 (allternit-ai #163) `@Bot` in chat / group chat starts a task thread inline with a live task list
- [~] P6.2 (email done: one thread per conversation; Slack once Slack is bound to a bot) External channels (email / phone / Slack-style) enter as threads

### P7 — Subagents and memory
- [x] P7.1 (platform #841, allternit-ai #166) Compressed subagent result contract; transcript on expand
- [x] P7.2 (allternit-ai #166) Subagent → thread promotion
- [x] P7.3 (platform #841) Memory scopes (global / project / bot / thread / subagent) + promotion API + `memory.promoted`
- [x] P7.4 (platform #841) Scheduled memory curation per bot

### P8 — Governance
- [x] P8.1 (platform #842, allternit-ai #168) Per-bot spend limit; per-thread budget from the plan
- [x] P8.2 (allternit-ai #168) Task log (who asked, what ran, result)
- [x] P8.3 (platform #842, allternit-ai #168) Tool/connector scoping per surface or channel; admin audit

### P9 — Other surfaces
- [x] P9.1 (allternit-ai #169) PWA (FabricBotMode) on the Threads components
- [x] P9.2 (Agents tab › bot: Threads card; no new rail tab) iOS on the thread object model
- [x] P9.3 (platform #844) `gizzi bot threads` / `gizzi thread steer`
