# Allternit Factory: one product, one engine

2026-10-04, revision 3. For Eoj. This is the design for Allternit's linchpin layer. Nothing is built or renamed yet.

**Allternit Factory** (the Allternit SWE Factory) is the harness of harnesses. It's not a second product. It's what **Gizzi** (the terminal product) and **Allternit Desktop** (the graphical product) both run. You type `gizzi`. Underneath sits one engine, `allternit-factory`, which nobody types. It's built inside the platform repo, ships next to Gizzi and inside Desktop, and is what every surface reads.

---

## 0. Decided (Eoj, 2026-10-04)

1. **One product per surface. Gizzi is the terminal product and Allternit Desktop is the graphical one.** We don't market two TUIs. Gizzi is both an agent harness and the factory floor, and it dogfoods itself as a harness inside the Factory.
2. **Option A: the Factory is commands inside Gizzi, plus one hidden engine binary.** The engine (`allternit-factory`, Rust) installs next to Gizzi through brew, npm and Desktop. Users never type it.
3. **The four parts are top-level Gizzi commands:** `gizzi agents`, `gizzi orchestration`, `gizzi workflows`, `gizzi workspace`. Gizzi's older agent-control commands fold into them.
4. **Live terminals use the engine's pane view:** the Rust (Herdr-fork) wall of live agent terminals, fast. Gizzi hands the screen to it and gets it back on exit.
5. **CommRails and the agent orchestrator (`ao`) merge into the engine.** Their old names are removed, not aliased, and skills, agents, scripts and docs are updated.
6. **Every surface is the same.** Desktop's separate orchestrator (Gizzi's `/v1/orchestrator`) folds in.
7. **It's built inside the platform repo**, not its own repo.
8. **API route prefix: `/api/factory`.**
9. **`allternit-mux`, the cowork queue and `kernel` (as the template compiler) fold in.**
10. **The inside is organized now, by job**, and runs deterministically.
11. **Every pane is a full Bot** (binding `type: terminal`), with everything a bot has: persona, memory, avatar, phone, reach, tools, autonomy.
12. **Vendor tickets fold into node deliveries.**
13. **No new Desktop mode.** It's a new way to present: the Factory views (graphs, markdown dashboards, mermaid).
14. **Approvals in-app, by push and on channels, all now.**

## 1. Gizzi, the engine, and Desktop

```
Gizzi (terminal product)                   Allternit Desktop (graphical product)
  gizzi                → home TUI: chat with one agent + the factory floor
  gizzi agents | orchestration | workflows | workspace …
  pane wall / attach   → handed to the engine's Rust pane view, styled as Gizzi
        │                                   │
        └────────────── both talk to ───────┘
                 allternit-factory   (engine: ledger, Gate, panes, agents, fabric; serves /api/factory)
                        │
     panes: gizzi (as a harness) · claude · codex · kimi · grok · agy
```

- **Gizzi is both.** With no arguments it's the TUI. Inside a Factory pane it's an agent harness like Claude Code or Codex. The same binary does both, so Gizzi dogfoods itself.
- **The engine is one process that outlives any window.** Agents keep running when you close Gizzi. Desktop and the phone read the same engine at the same time. An engine crash doesn't take your terminal with it. That's why it's a separate executable, not code inside Gizzi's process.
- **One TUI product, two renderers.** Gizzi's Ink TUI draws the home screen, the agents list, the board and approvals. The engine's Rust pane view draws the wall of live terminals, because it's built for that. You only ever launch `gizzi`.
- **Windows.** Gizzi ships on Windows, but the pane engine is Unix-only. On Windows, Gizzi drives a Factory running on another machine (Mac, Linux, a cloud computer).

## 2. Do CommRails and ao stay separate?

**No.** After the merge there's one engine, one install, one version and one set of docs. "CommRails" and "ao" stop existing as product names, command names or skill names.

What stays separate is **the inside**, and it's organized by job, not by history:
- **The core** stores events, IDs and state.
- **The Gate** is the only thing allowed to change state.
- **Four parts** match the industry's four words.
- **The surfaces** are the TUI, the local API and remote machines.

---

## 3. The four parts

| OpenRig | Factory part | Holds (existing Allternit object names stay inside) |
|---|---|---|
| Rig | **`agents`** | Bots, harnesses (Gizzi, Claude Code, Codex, Kimi, Grok, agy), `team.yaml`, presets, spawn/stop/recover, machines |
| Coordination | **`orchestration`** | Threads (`standing` / `task`), send, capture, transcript, mailbox, mail, attention, steering, the Coordinator |
| Workflows | **`workflows`** | Templates (and the template compiler, from `kernel`), drive, Wake, wait-gates, leases |
| Workspace | **`workspace`** | Campaigns, plans, DAG nodes, WIH pickups, node folders, proof, receipts, judge, vault, the cowork queue (folded) |

**Why `agents` and not `threads` for the first part.** The first part is the crew: which agents exist, which harness each runs on, which machine they're on. A Thread in Allternit is a conversation or an assignment *with* an agent (`standing` or `task`). If the crew part were called `threads`, the word would mean two things. So Threads live inside `orchestration`, which is where conversation and handoffs happen, and the crew part is `agents`, the industry word for it.

---

## 4. Where it lives in the platform repo

It follows the repo's own convention (`REPO_STRUCTURE.md`): binaries in `cmd/`, substrates as top-level crate directories.

```
allternit/                     (platform repo)
├── cmd/allternit-factory/     the engine binary: `allternit-factory` (internal, never typed)
├── factory/                   replaces commrails/ and infrastructure/executor/ao-engine/
│   ├── core/                  ledger · ids · store · projections · index · replay · config
│   ├── gate/                  the only writer: policy, hooks, kill switch, egress, fence
│   ├── agents/
│   ├── orchestration/
│   ├── workflows/
│   ├── workspace/
│   ├── tui/                   the Herdr fork (keeps its Apache-2.0 LICENSE + NOTICE)
│   ├── api/                   /api/factory (the engine's server)
│   └── remote/                machines, fabric
├── cmd/allternit-api/         links factory/* in-process (it links commrails today)
├── cmd/gizzi-code/            the product CLI/TUI: `gizzi agents|orchestration|workflows|workspace` call the engine
└── surfaces/allternit-desktop ships `allternit-factory` + `gizzi` in resources/bin; gizzi on PATH
```

**What depends on it today** (checked 2026-10-04): only `cmd/allternit-api` and the ao engine link the CommRails crate. `allternit-mux` is used by allternit-api's terminal routes (`terminal_routes.rs`), Desktop's `gizzi-manager.ts`, and Gizzi's PTY integration. That's a small, clean set of edges. Building it inside the platform doesn't tangle anything new, and it keeps one coherent platform.

**A fifth CommRails entry point was found:** `commrails/cli` builds its own `commrails` and `rails` binaries and has an `install.sh` that puts them in `~/.cargo/bin`. It's removed in the merge like the others.

## 5. Inside the engine

### Where every existing module goes

| Part | From CommRails (`commrails/src`) | From ao (`ao-engine/src`) | From elsewhere |
|---|---|---|---|
| **core** | `core`, `ledger`, `index`, `projections`, `replay`, `prompt` (provenance), `compact`, `query`, `dolt` (optional backend) | `config`, `persist`, `events`, `ipc`, `protocol` | — |
| **gate** | `gate`, `hook`, `policy`, `killswitch`, `egress`, `fence`, `constraints` | `cli/ao_gate.rs` (the spawn gate) | — |
| **agents** | `peer`, `orchestrator` (removed: its spawn moves into ao's), `execenv` | `ao/harness`, `ao/native`, `ao/peers`, `agent_resume.rs`, `detect`, `integration` (per-vendor agent detection), `worktree.rs`, `session.rs` | Gizzi `runtime` (local agent CLI discovery) |
| **orchestration** | `mail`, `bus`, `transport`, `attention`, `steer`, `observer` | `ao/mailbox.rs`, `ao/transcript.rs`, pane read/send paths | Gizzi `ac` and `mail` commands. Gizzi `/v1/orchestrator` send/tail |
| **workflows** | `templates`, `drive`, `wake`, `wait_gates`, `leases`, `merge_locks`, `dependencies` | `watch` + `drain` logic | — |
| **workspace** | `work`, `wih`, `campaign`, `receipts`, `judge`, `verification`, `vault`, `lessons`, `context`, `memory`, `echoes` | — | Node folders (new). Proof (new). The cowork queue and run tables (folded) |
| **surfaces/tui** | — | `ui`, `client`, `app`, `pane`, `pty`, `terminal*`, `layout.rs`, `workspace`, `input`, `kitty_graphics`, `ghostty`, `copy_mode.rs`, `selection.rs` | `cmd/allternit-mux` folds into this pane engine (decided) |
| **surfaces/api** | `service.rs`, `mcp.rs`, `bridge`, `workspace` (integration) | `api`, `server`, `ao/serve.rs` (UHP), `ao/visibility` | Gizzi `/v1/orchestrator` routes (re-pointed here, then removed from Gizzi) |
| **surfaces/remote** | `bridge` identities | `ao/fabric`, `remote` | — |

**Not moved in, and why:**
- **`tickets`, `graph`, `sync`**: these are the portable ticket CLI for foreign repos. Earlier ruling: Ticket stays a separate foreign-repo tool. It ships as its own small command, not inside `workspace`.

**`kernel`** (ComputeGraph IR) moves into `workflows` as the template compiler (decided), so there's one authority over templates.

**Herdr stays a clean block.** The forked Herdr code (`surfaces/tui` plus its pane/terminal modules) keeps its Apache-2.0 LICENSE and NOTICE, and keeps the same internal layout. That way upstream Herdr fixes can still be merged. Allternit additions sit beside it, not inside its files.

---

## 6. The determinism contract

This is what "built on software-engineering discipline" means in practice. Every command and module follows these rules, and the docs state them up front.

1. **One writer.** Only the Gate changes state. Every change is an event appended to the ledger, with provenance (who, which prompt, which pickup). No module writes its own side database for work state.
2. **State is derived.** What `ps`, `board` and every screen show is a projection of the ledger plus live process facts. The projection can be thrown away and rebuilt, and `replay` proves it.
3. **One registry, reconciled.** Running sessions are checked against reality (tmux / the pane engine) at start and on every `ps`. A record says "dead" when the process is gone, never "running". Today's 26 recorded vs 45 running can't happen.
4. **Every mutation has a dry run.** `--dry-run` prints exactly what would change, and the real run does exactly that.
5. **Stable IDs and idempotent commands.** Running a create twice with the same key doesn't make two things.
6. **Every command speaks JSON.** `--json` on every read and write. A fixed exit-code table, documented once: 0 ok, 1 refused by Gate, 2 not found, 3 transport broken, 4 timeout, 5 needs a person.
7. **No silent fallbacks.** If a message can't be delivered verified, it's queued and says so. If a spawn is refused, it says why. Nothing "works" by quietly doing something else.
8. **Nothing runs unasked.** `drive` and `watch` run in the foreground. The engine server is explicit (`allternit-factory`, started by Desktop or by the first `gizzi` command that needs it, and it says so), and Wake only schedules checks. It never marks work done.
9. **Proof comes from the judge and receipts.** It never comes from narrative files or todo checkboxes.
10. **Bounded loops.** Every retry or remediation loop has a written limit (`max_rounds`, per-pickup iterations).

---

## 7. The command tree, with every OpenRig command

Yes: every command we mapped from OpenRig is here, under one of the four parts. So are the CommRails and agent orchestrator commands, and Gizzi's older commands.

### `gizzi agents` (OpenRig: Rig)

| Command | OpenRig | Comes from |
|---|---|---|
| `gizzi agents up [<team>] [--preset P] [--on <computer>] [--dry-run]` | `rig up` | new (`team.yaml`), spawn from ao |
| `gizzi agents ps` | `rig ps` | ao status / visibility, CommRails peer list |
| `gizzi agents down [<team>]` | `rig down` | ao kill |
| `gizzi agents whoami` | `rig whoami` | new |
| `gizzi agents recover [--apply]` | `rig seat handover`, `rig restore` | ao recover |
| `gizzi agents snapshot` / `restore` | `rig snapshot` / `restore` | new (whole team) |
| `gizzi agents model <bot> <model>` / `handoff <bot>` | `rig seat set-model` / `handover` | Bot model, thread handoff |
| `gizzi agents harness list\|sync` | (runtimes) | ao harness, `gizzi runtime` |
| `gizzi agents pack` / `install <path\|github-url>` | `rig bundle create` / `install` | new |
| `gizzi agents templates` | (agent specs) | `gizzi agent-hub` |
| `gizzi agents wall [<team>]` | `rig tui` terminals view | opens the engine's live pane wall |
| `gizzi agents attach <bot@team>` | drop into a terminal | the engine's pane view |
| `gizzi agents doctor` | `rig doctor`, `rig health` | ao doctor, `gizzi doctor` checks |

### `gizzi orchestration` (OpenRig: Coordination)

| Command | OpenRig | Comes from |
|---|---|---|
| `gizzi orchestration send <bot@team> "…" [--queue]` | `rig send` | ao send (verified, or queued), recorded in mail |
| `gizzi orchestration capture <bot@team> [lines]` | `rig capture` | ao status / pane read |
| `gizzi orchestration transcript <bot@team>` | `rig transcript` | ao transcript |
| `gizzi orchestration threads list\|show\|new` | `rig chatroom` | Bot threads (`standing` / `task`) |
| `gizzi orchestration mail list\|read\|send\|decide` | — | CommRails mail, `gizzi mail` |
| `gizzi orchestration feed` | `rig stream` | ledger events |
| `gizzi orchestration attention list\|ack` | needs-you | CommRails attention |
| `gizzi orchestration steer checkpoint\|consult` | — | CommRails steer |
| `gizzi orchestration coordinate <project> "…"` | orchestrator seat | the Coordinator |

### `gizzi workflows` (OpenRig: Workflows)

| Command | OpenRig | Comes from |
|---|---|---|
| `gizzi workflows run <template> [--param k=v]` | `rig workflow instantiate` + `run` | plan from template + drive |
| `gizzi workflows drive <dag> [--once] [--dry-run]` | `rig workflow watch` | CommRails drive |
| `gizzi workflows template list\|show\|check\|save` | `rig workflow validate` / `show` | CommRails templates (+ kernel compiler) |
| `gizzi workflows wake list\|due\|run` | `rig watchdog` | Wake, ao watch, `gizzi cron` |
| `gizzi workflows gate add\|resolve\|list` | human gate | wait-gates |
| `gizzi workflows status <run>` | `rig workflow status` | DAG render |

### `gizzi workspace` (OpenRig: Workspace)

| Command | OpenRig | Comes from |
|---|---|---|
| `gizzi workspace campaign new\|list\|status\|pause\|finish` | `rig scope mission` | Campaign |
| `gizzi workspace plan new\|refine` | — | CommRails plan |
| `gizzi workspace node add` | `rig queue create`, `rig scope slice create` | node + folder (new) |
| `gizzi workspace node list [--mine]` | `rig queue list --owned`, `rig scope slice ls` | WIH / DAG |
| `gizzi workspace node claim\|handoff\|close` | `rig queue claim` / `handoff` | WIH pickup / close |
| `gizzi workspace approve <node>` | `rig queue resolve` | wait-gate / judge resolve, mail decide |
| `gizzi workspace proof add\|show` | `rig proof add` / `show` | receipts + node folder |
| `gizzi workspace judge show\|resolve` | `rig proof judge` | judge |
| `gizzi workspace board [<campaign>]` | the project board | node folders, judge verdicts |
| `gizzi workspace tasks` | — | cowork queue (folded), `gizzi cowork` |

### Top level

| Command | What it does |
|---|---|
| `gizzi` | The TUI: one agent, plus the factory floor (agents, board, approvals) |
| `gizzi doctor` | Checks Gizzi, the engine, harnesses, transport and install |
| `gizzi serve` | Unchanged: Gizzi's own harness server. The Factory API is the engine's, at `/api/factory` |

Gizzi commands that move: `gizzi agent` / `agent-hub` / `runtime` → `gizzi agents`; `gizzi mail` / `ac` → `gizzi orchestration`; `gizzi cron` → `gizzi workflows wake`; `gizzi cowork` → `gizzi workspace tasks`. `gizzi agent list` stops printing four hardcoded names.

---

## 8. How a piece of work moves (the explanation for the docs)

1. **You set intent.** `gizzi workspace campaign new "Saved views"` writes the Campaign and its `SPEC.md` intent.
2. **It's split into nodes.** `gizzi workflows run build-check-prove` or `gizzi workspace node add` creates DAG nodes through the Gate. Each node gets a folder (`SPEC.md`, `PROGRESS.md`, `PROOF.md`, `proof/`).
3. **The team boots.** `gizzi agents up product-build` reads `team.yaml`. It opens one space with a pane per bot, each on its harness from the preset, and registers each as a peer.
4. **Work is picked up.** `gizzi workflows drive` finds READY nodes. Each pickup is a WIH through the Gate, and the bot's pane receives the task through a verified send.
5. **It's checked.** The checker bot runs the proof contract. On failure, the template's `on_fail` sends it back, up to `max_rounds`.
6. **It's proven.** Evidence lands in `proof/`. The judge records the verdict. Receipts are written at close.
7. **You approve.** `gizzi workspace approve <node>`, from the TUI, Desktop or the phone. You see the spec next to the proof.

Every step is an event in the ledger. Every screen is a view of those events plus live pane state.

---

## 9. Bots in the Factory

Every agent in the Factory is a **Bot**: an `agents` row with a stable id. The Factory doesn't add a second kind of agent. What differs between bots is **how a bot runs** (its execution binding, `bot_execution_bindings`) and **how people reach it** (channels). Both already exist in allternit-api.

### Three ways a bot runs

| Binding | What it is | Examples | Today |
|---|---|---|---|
| **Hosted** (`type: hosted`) | Runs on Gizzi's session loop, locally or on a cloud computer | Al (the Coordinator), bots created in Bot Mode | Live |
| **Terminal** (`type: terminal`, **new value**) | A CLI harness in an engine pane | Claude Code, Codex, Kimi CLI, Grok CLI, agy, `gizzi` itself | These are ao sessions today, keyed by slug and **not bots at all**. In the Factory, every pane belongs to a bot |
| **Vendor** (`type: vendor`, mode `linked` / `mirror`) | A vendor's own agent or app, reached through the agent gateway | ChatGPT, Claude, Gemini, Copilot, Grok Bot, Kimi, Hermes, dots | Live: lanes `official` / `channel` / `ui_bridge` / `local`, guarantee `exact` / `best_effort` / `read_only`, directed by another bot (`directing_bot_id`) |

**Reach is separate from running.** Telegram, Slack, SMS, phone calls, email and push are how people talk to a bot. A Hosted bot can be on Telegram. A Vendor bot stays inside its directing bot's phone. A channel is an `orchestration` concern (where messages come and go), never a fourth way to run.

### How work reaches each kind (one verb, three deliveries)

`gizzi orchestration send` and a node pickup (`gizzi workspace node claim`, or `drive`) work the same for every bot. The engine picks the delivery from the binding, and it always reports which delivery happened:

| Binding | Delivery | What "delivered" means |
|---|---|---|
| Hosted | A turn posted into the bot's Gizzi session | `verified`: the session accepted it |
| Terminal | A verified paste into the bot's pane, or queued in its mailbox when the pane is busy | `verified` or `queued` |
| Vendor | A **vendor ticket** (`T-n`) through the connector (`get_ticket` / `post_result`), plus a one-line nudge in the vendor chat | Depends on the lane's guarantee: `exact`, `best_effort` or `read_only`. A `best_effort` delivery is never shown as read |

This is rule 7 of the determinism contract applied to bots: the UI shows the real delivery state, not a guess.

### How proof comes back

- **Hosted and Terminal:** WIH close through the Gate, a receipt, and evidence in the node's `proof/` folder.
- **Vendor:** the ticket's `post_result` becomes the node's output. The same judge checks it. The thread keeps the vendor's own look (look packs), and the node keeps the proof.

### What folds in

- **ao sessions become Terminal bots.** `builder@product-build` is a bot whose binding is `terminal` + `claude`. The pane, the thread and the node all point at the same bot id.
- **Vendor tickets become the delivery record of a node.** There's no separate ticket list. A ticket is how a node assigned to a vendor bot gets there, and `T-n` links to the node.
- **Teams mix all three.** `team.yaml` lists bots of any binding. A preset can swap a role between Terminal Claude Code and Vendor ChatGPT, for example.

```yaml
bots:
  - { bot: al,       role: coordinator, binding: hosted }
  - { bot: builder,  role: build,       binding: terminal, harness: claude }
  - { bot: checker,  role: check,       binding: terminal, harness: codex }
  - { bot: research, role: research,    binding: vendor,   vendor: chatgpt, lane: official, directed_by: al }
reach:
  al: [telegram, push]
```

---

### A Terminal bot is a full Bot (decided)

Everything that comes with a bot comes with a Terminal bot. Server-side pieces work the same for every binding. The pieces that have to live inside the harness are delivered by the engine when the pane starts, and again on re-entry (after compaction or a restart). Each one is labeled with what actually happens, the same per-field honesty rule vendor Mirror mode uses.

| What a bot has | Hosted (Gizzi) | Terminal (Claude Code, Codex, …) | Vendor |
|---|---|---|---|
| Identity, name, avatar, photo sprite | Full | Full. The pane header and wall tile show the avatar | Full |
| Threads, inbox ("needs you"), bot phone | Full | Full | Shares its directing bot's phone |
| Reach: channels, phone number, email | Full | Full. Messages arrive as turns in the pane | Through the directing bot |
| Live voice calls | Full | Routed: the call is answered by the bot's Hosted front (or its directing bot), and the request lands in the pane as a turn. A CLI can't hold a live call | Through the directing bot |
| Persona + role instructions, twin persona (V234) | System prompt | **Guidance**: written into the harness's own instruction file as a managed block (`CLAUDE.md` / `AGENTS.md` / `GIZZI.md`) and re-sent on re-entry. Labeled *guidance*, because the harness's built-in prompt still applies | Through the connector, where the lane allows |
| Memory: bot memory + active twin memory | Injected | A **context pack** written at pickup (WIH `context_pack_path`) plus `LEARNED.md`. What the bot learns comes back as `proposed`, and only the owner's accept makes it active (the twin rule) | Connector read |
| Skills | Loaded | Copied into the harness's skills folder at spawn (`.claude/skills`, …) | Not available |
| Tools, connectors, computer, browser | Native | The Allternit MCP connector is registered in the pane's harness config (the same connector vendor bots use, `mcp.allternit.com/mcp/bots/<id>`, or local). Computer control goes through leases | Connector |
| Vault and credentials | Leased | Never written into the pane. Only reached through a connector tool with a lease, and every use is audited | Connector, with a lease |
| Model | Setting | Passed as the harness's own flag (`--model`, Codex profile, …) | The vendor's |
| Permissions and autonomy (V233) | Gate | Gate hook in the harness (`PreToolUse` for Claude, Codex and Qwen) plus the bot's autonomy policy (`draft` / `ask` / `tell` / `limits`) | Lane + policy |

### Vendor tickets (decided: fold into nodes)

A node assigned to a vendor bot is delivered as a vendor ticket. The node is the work item, and the ticket (`T-n`, `vendor_tickets`) is its delivery record: what was sent, on which lane, and the `post_result` that came back. The UI shows the ticket inside the node and the thread. There's no separate ticket list. The table stays as the delivery log, so nothing already sent is lost.

### Approvals: in-app, push and channels (decided: all now)

Every approval is a Gate event with the owner as actor and the surface as provenance. The same request appears everywhere at once, and the first valid answer wins. The others then show who approved and where.

- **In-app:** the Approve button on the Node page, the board card, the bot phone Work app, and Gizzi.
- **Push:** a notification with Approve and Open buttons. Approve checks the device session belongs to the owner.
- **Channels (Telegram, Slack, SMS, email):** the request is posted only to channels bound to the owner's own account (`provider_account_bindings`, `channel_account_bots`), with a one-time code tied to that node and that owner. Telegram and Slack get buttons. SMS and email reply with `approve n_02 <code>`.
  - The code expires.
  - A reply from any sender other than the verified owner identity is refused and logged.
  - A forwarded message can't approve.
  - The bot's autonomy policy can require in-app approval for high-risk nodes (money, deploys, client messages), which matches the review gates in Allternit's rules.

---

## 10. How it looks and works on each surface

No new mode. The Factory's four parts land in places Allternit already has, and every surface reads the same engine.

### The Factory views: one way to draw graphs and markdown dashboards (decided: no new mode)

The new thing is a way to present, not a place. One set of views lives in `allternit-ai` (`src/components/factory/`) and is used by Bot Mode, Code mode, the shared pane and the phone. Gizzi draws text versions from the same JSON.

| View | What it draws | Built with | Data |
|---|---|---|---|
| **Team graph** | Bots as cards (avatar, binding badge, state, node, proof k/n, reach). Edges: delegates to, checks, directs (vendor) | `@xyflow/react` (already a dependency) with a fixed layered layout, so the same team always draws the same way | `team.yaml`, bindings, live state |
| **Flow graph** | A Template or DAG: steps, `blocked_by`, the fail route back, the human gate. Node status live on top | `@xyflow/react`, layered left to right by `blocked_by` depth | ledger DAG |
| **Board** | The markdown dashboard: summary line (NOW / NEXT / PROVEN k/n / NEEDS YOU), waves by `blocked_by` depth, one card per node | Plain components from the node folders' `SPEC.md` frontmatter + judge verdicts | node folders + ledger |
| **Node page** | INTENT → PLAN → DELIVERED, `PROGRESS.md` to the side labeled narrative, proof paired to contract lines, Approve | `allternit-markdown` renderer | node folder |
| **Diagrams in markdown** | Any ```` ```mermaid ```` block in a `SPEC.md`, `PROGRESS.md` or Campaign doc renders as a diagram | `mermaid` (already a dependency) inside `allternit-markdown` | the file |

Rules for every view: the same input always draws the same picture (no random layout), empty shows "—", and counts come from the judge, never from narrative.

Two graph libraries are installed today (`reactflow` 11 and `@xyflow/react` 12). The Factory views use `@xyflow/react` only, and the `reactflow` callers move over in the same change.

### Allternit Desktop

| Part | Where it shows | Built on (allternit-ai) |
|---|---|---|
| **Front door** | Bot Mode's launch screen: "What should the team work on?" That screen is already the Factory front door | `BotLaunchpadView` (composer, team with live state, templates, projects, Threads panel) |
| **agents** | The team strip on the launch screen and project page. Each card shows avatar, `bot@team`, a binding badge (Gizzi / Terminal · Claude Code / Vendor · ChatGPT · official · exact), state, current node, proof k/n, and reach icons. A graph view draws who delegates to whom and who checks whom | `BotLaunchpadView` team, `ProjectHomeView` Team card, `BotSubagentTree` |
| **orchestration** | Threads: `standing` and `task` labeled differently. Task threads carry a node strip. Vendor threads keep their look pack, and channel threads keep their channel look. Every message shows its delivery state | `BotChatSessionView`, `ChannelThread`, vendor look packs, `BotInboxView` ("needs you") |
| **workflows** | A template picker in the composer ("Run as: Build, check, prove"), and a Flow graph tab on the project page with live node status, the fail route and the human gate | `ProjectHomeView` threads graph (extended to nodes) |
| **workspace** | Project = Campaign. A Board tab on the project page, and the Node page in the shared pane: INTENT → PLAN → DELIVERED, narrative to the side, proof paired, and Approve | `PaneWorkspace` (the shared docked pane), `ProjectFileWorkspace` |
| **Live terminals** | Code mode: the terminal wall shows every Terminal bot's pane, with a `bot@team` + node header. Hosted bots open their Gizzi session. Vendor bots open their thread, since they have no terminal | `TerminalWorkspace`, `CodeCanvasView` (xterm tiles fed by engine panes over `/api/factory`) |
| **Per-bot device** | The bot phone. Its home screen gets a Work app (that bot's nodes and approvals). Vendor bots stay inside their directing bot's phone | `PhoneOS`, `VendorBotsApp` |

On Desktop, the wall is drawn with xterm tiles fed by the engine's panes. In the terminal it's the engine's Rust pane view. They're the same panes, so a keystroke in either shows up in both.

### ai.allternit.com phone layout and m.allternit.com

- The bot phone is the main surface: one bot, its threads, its Work app, Approve.
- Code: one terminal with a bot strip above it to switch panes (Terminal bots only).
- Board and Node page are one-column lists. Approve works from day one.
- "Needs you" arrives as a push notification with Approve and Open buttons, and on the owner's own channels with a one-time code (see Approvals above).

### Gizzi (terminal)

- `gizzi` home lists every bot of every binding with the same badges.
- Attach depends on the binding. A Terminal bot opens the live wall on its pane. A Hosted bot opens its session right in Gizzi's TUI, so Gizzi dogfoods itself. A Vendor bot shows its thread and ticket status (no terminal).

---

## 11. Every surface reads the same thing (summary)

| Surface | Today | After |
|---|---|---|
| Terminal | `ao` TUI (Herdr fork) + `allternit-rails` CLI + Gizzi | `gizzi`: home TUI in Ink, live terminal wall in the engine's Rust pane view |
| Desktop Code mode | OrchestratorCenter → Gizzi `/v1/orchestrator` (own tmux). TerminalWorkspace → allternit-api `/terminal` → `allternit-mux` | Both read the engine (`/api/factory`). A terminal tile is an engine pane, the same one the TUI shows |
| Desktop MissionControl / RailsTaskList | allternit-api `/api/rails` + `/api/commrails` | allternit-api links the `factory/` crates instead of CommRails. Routes move to `/api/factory` |
| Bot Mode threads | allternit-api threads + Coordinator | Unchanged objects. Task threads mirror nodes through the same Gate |
| ai.allternit.com phone + m.allternit.com | One terminal tab | Bot strip over the one terminal, plus Board and Approve. Same engine data, through the fabric relay |
| Gizzi | Has `mail`, `ac`, `runtime`, `cowork`, `/v1/orchestrator` scattered | The terminal product: the four parts as commands, the factory floor in its TUI, and a harness inside panes. `/v1/orchestrator` is replaced by the engine |

---

## 12. The rename: what changes where

Counted 2026-10-04: about **360 files** in the platform repo, **29** in allternit-ai, **75** in the Allternit Brain, and **7** skill files (`agent-orchestrator` + its scripts, `research-pipeline`, `repo-ritual-land`, `client-report`) mention CommRails or ao by name.

| What | Change |
|---|---|
| Binaries | `allternit-rails`, `allternit-commrails`, `allternit-commrails-service`, `commrails`/`rails` (commrails/cli), `ao`, `ao-*` scripts → `gizzi agents|orchestration|workflows|workspace` for people and agents. The engine binary is `allternit-factory` |
| Env vars | `ALLTERNIT_COMMRAILS_*`, `ALLTERNIT_RAILS_*`, `AO_*`, `HERDR_*` → `ALLTERNIT_FACTORY_*` |
| On-disk state | `.allternit/` workspace layout keeps its paths, since that's data, not a name. `~/.agent-orchestrator/` moves to `~/.allternit/factory/`, with a one-time migration that `gizzi doctor` checks |
| HTTP routes | `/rails`, `/api/rails`, `/commrails`, `/api/commrails` → `/api/factory`. Old routes are removed in the same release that updates every caller |
| Event fields | `actor.type: "gate"` stays. Event names are data in the ledger and are not renamed, so old ledgers still replay |
| Skills and agents | `agent-orchestrator` skill → rewritten around `gizzi agents` / `gizzi orchestration`. The other three skills update their commands. CLAUDE.md / AGENTS.md / GIZZI.md lines that mention the old names get updated |
| Docs | New dev docs section in `surfaces/docs/` for the layer (this explanation + the command reference + the determinism contract), user docs for the Desktop views, and Brain `Products/` + `Infra/` pages. `check_links.py` must show 0 problems |
| Desktop | Ships `allternit-factory` and `gizzi` in `resources/bin` and puts `gizzi` on PATH at first run. The old `~/.local/bin/allternit-rails`, `ao-*` symlinks and the `ao-consult` guard are removed by the installer |

---

## 13. Build order

Each step lands as reviewed PRs. The names are now decided, so every step can start.

1. **Freeze the boundary.** Write this spec into `surfaces/docs/` as the dev doc. Add the determinism contract as tests: replay, `--dry-run` parity, exit codes.
2. **Create `factory/` and `cmd/allternit-factory/`.** Move the CommRails and ao code in with `git mv`, so history follows. Build one engine binary. Module moves only, no behavior change. Old binaries are no longer built.
3. **One spawn path, one send path, one registry.** Delete CommRails `orchestrator spawn` in favor of the pane engine's spawn. Reconcile the registry against live panes.
4. **Rename sweep.** Binaries, env vars, routes, skills, agents, scripts, docs, Brain, all in one pass, with a grep check in the repo's local pre-merge script that fails on any old name. Not a new GitHub Actions job: private-repo minutes are capped.
5. **Gizzi commands + Desktop fold-in.** Add `gizzi agents|orchestration|workflows|workspace` as thin clients of the engine, and fold Gizzi's old commands into them. OrchestratorCenter and TerminalWorkspace read the engine. `allternit-mux` folds into the pane engine. Gizzi's `/v1/orchestrator` is removed. Ship `allternit-factory` next to Gizzi (brew, npm) and inside Desktop.
6. **Bots and the four concept files.** The `terminal` binding (every pane is a full bot: persona as guidance, context pack + `LEARNED.md`, skills, the Allternit connector, Gate hook, autonomy), vendor tickets as node deliveries, delivery state on every send, `team.yaml` with mixed bindings and presets, Template fields, node folders, thread labels.
7. **Approvals everywhere.** In-app, push, and owner-bound channels with one-time codes, all as Gate events.
8. **The Factory views + screens.** `src/components/factory/` (Team graph, Flow graph, Board, Node page, mermaid in markdown) on `@xyflow/react`, with the `reactflow` callers moved over. Gizzi's TUI gets the factory floor plus the hand-off to the engine's live pane wall, Desktop Team / Terminals / Board / Node, and phone, all in one change with docs.
9. **Cowork fold.** The cowork queue and run tables become `workspace` nodes. This is the third list of work today, so it goes last, once nodes and their folders are solid.

---

## 14. Still open (small)

1. **Short forms** for the four parts (for example `gizzi orch`). Default: none, and shell completion does the typing.
2. **How the hand-off looks:** the key that opens the live wall from Gizzi's TUI, and the key that returns. Default: the wall opens on `w`, and `ctrl+b q` returns, as in the pane engine today.
