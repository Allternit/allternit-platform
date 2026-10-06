# Phase 1 Matrix — Web/Desktop items cross-referenced against iOS and gizzi-code

Source: `docs/learnings/SURFACE_AUDIT_MAP.md` RAW INVENTORY 1, cross-checked against
RAW INVENTORY 2 (iOS) and RAW INVENTORY 3 (gizzi-code) in the same doc.
Classification rubric per that doc: FULL PARITY / PARTIAL / GAP /
INTENTIONALLY SURFACE-SPECIFIC / UNCLEAR — NEEDS CODE-LEVEL CHECK.

Note on granularity: several Web bullets bundle multiple distinct
registered views in one line (e.g. "Cowork Runs/Drafts/Tasks/..."). Where
the source text itself says these are individually registered ("each is
its own registered view", "registered individually", "N registered
routes") or lists distinctly named sub-panels, this matrix gives each one
its own row rather than collapsing them — that's why the row count below
is somewhat higher than the doc's rough "~150" estimate; see the NOTES
file for the exact count and reasoning.

---

## Core Chat / Home

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Chat (Home) | Core Chat/Home | Full — Chat tab, streaming, model picker | Full — `gizzi` default TUI loop, `run` one-shot | FULL PARITY | Core conversational loop is genuinely present and load-bearing on all three surfaces. |
| Chat-legacy route | Core Chat/Home | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | It's a URL-compatibility redirect for old links, not a feature with independent behavior to mirror. |
| Agent Hub | Core Chat/Home | Full — Agents tab (registry, detail, creation from templates) | Full — `agent-hub` command + template library | FULL PARITY | All three let a user browse/launch pre-built agent configs. |
| Projects | Core Chat/Home | Full — Projects tab, explicitly "Claude/ChatGPT Projects parity" | Missing — `init` scaffolds a per-directory `.gizzi` workspace but there's no browsable, named cross-session "Projects" list | GAP | A CLI user has no way to group sessions into a named project the way web/iOS do; `init` is a one-shot scaffold, not a browser. |
| Artifacts Library | Core Chat/Home | Full — dedicated tab, client-side aggregation across chat streams | Partial — HTML artifact publish writes into the same canvas system but gizzi-code has no local browser of past artifacts | PARTIAL | gizzi-code can produce/publish artifacts consumed elsewhere but can't browse its own artifact history. |
| Automation Tasks (Goals/Routines/Loops/Cron) | Core Chat/Home | Missing — no automation/scheduling UI in the iOS inventory | Full and arguably deepest of the three — goal engine, routine engine, loop engine, cron daemon, IntelliScheduleEngine | GAP | Missing from iOS specifically; ironically gizzi-code's engine is the richest implementation of this concept across all three surfaces. |
| Dispatch (QR hand-off to phone, Beta) | Core Chat/Home | Not described — iOS inventory doesn't mention a "receive a dispatched task" flow | Partial — `pair`/`/bridge`/`/mobile` + session QR give gizzi-code its own remote-pairing mechanism, different direction/purpose | UNCLEAR — needs code-level check | Can't confirm from the inventory alone whether iOS actually implements the receiving side of Dispatch or whether it's silently absent. |
| History | Core Chat/Home | Full — dated sidebar history (Today/Yesterday/Prev 7 days/Older) | Full — `session` list/delete | FULL PARITY | |
| Archived | Core Chat/Home | Not confirmed — sidebar buckets by date, no explicit "archived" state mentioned | Not confirmed — `session` command described only as list/delete | UNCLEAR — needs code-level check | Inventories don't say whether either surface has a distinct archived-vs-active session state. |
| Recents | Core Chat/Home | Full — sidebar history serves this | Full — `session` list | FULL PARITY | |
| Search | Core Chat/Home | Partial — Projects tab has search/filter; global session search not confirmed | Missing — no search command in the CLI inventory | UNCLEAR — needs code-level check | Can't tell if iOS's search is scoped to Projects only or covers all chat history too. |
| Customize (Shellrail, nav-rail tab visibility) | Core Chat/Home | N/A — fixed 6-item sidebar, no customizable nav-rail concept | N/A — terminal has no icon-rail/tab concept | INTENTIONALLY SURFACE-SPECIFIC | This only makes sense for a UI paradigm (a persistent icon rail) that only web/desktop use. |

## Cowork (multi-agent team workspace)

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Cowork workspace (CoworkRoot) | Cowork | Thin — Cowork is a composer-level toggle inside Chats, not a dedicated workspace | Full — `cowork` command, 2700+ line runtime subsystem | PARTIAL | Web has the richest GUI, gizzi-code's engine is arguably even more capable via CLI, iOS only exposes a toggle. |
| Cowork Runs view | Cowork | Missing | Partial — run data exists via `cowork`, no dedicated browsing UI | PARTIAL | gizzi-code has the underlying data with no visual browser; iOS has neither. |
| Cowork Drafts view | Cowork | Missing | No explicit "drafts" concept — closest is the approval/HITL queue | GAP | Neither other surface has a named drafts view; gizzi-code's approval queue is the nearest functional analog. |
| Cowork Tasks view | Cowork | Missing | Partial — `cowork-team` Kanban board covers task assignment | PARTIAL | gizzi-code has real task-tracking via cowork-team; iOS has nothing. |
| Cowork Cron view | Cowork | Missing | Full — full `cron` subsystem (daemon, workers, metrics), richer than the web sub-view | GAP | Missing from iOS entirely; gizzi-code's cron is actually the most capable of the three. |
| Cowork Project view | Cowork | Partial — general Projects tab exists but no Cowork-run sub-workspace | Missing — `init` scaffolds a project but has no Cowork-run project view | GAP | Neither other surface exposes Cowork run history scoped to a project. |
| Cowork Documents view | Cowork | Missing — no document surface in iOS inventory at all | N/A — gizzi-code operates on real files directly, no need for an in-app doc viewer | GAP | Missing from iOS with no good reason it couldn't exist there; gizzi-code's absence is more defensible (file-native). |
| Cowork Tables view | Cowork | Missing | N/A — file-native, same reasoning as Documents | GAP | Same as Documents: real gap on iOS, defensible absence on gizzi-code. |
| Cowork Files view | Cowork | Missing — Attachments exist in chat but no generic Cowork file browser | N/A — file-native | GAP | Same pattern as Documents/Tables. |
| Cowork Exports view | Cowork | Missing | Partial — generic `export`/`import` session commands exist, not Cowork-run-specific | PARTIAL | gizzi-code has an adjacent generic mechanism; iOS has none. |
| Cowork Insights panel | Cowork | Missing | Missing — no analytics/activity feed for cowork runs in the CLI inventory | GAP | Absent from both other surfaces, no fundamental blocker to a CLI-side summary. |
| Cowork Activity panel | Cowork | Missing | Missing | GAP | Same as Insights. |
| Cowork Goals panel | Cowork | Missing | Full — standalone goal engine + `/goal` command, arguably richer | PARTIAL | gizzi-code's goal engine is a real, deeper equivalent; iOS has nothing. |
| Cowork Wiki section viewer | Cowork | Missing | Partial — `vault.ts` (unwired top-level command, reachable via TUI) is a conceptually similar knowledge base but not shipped as a first-class command | PARTIAL | Loose, currently-inactive gizzi-code overlap; no iOS equivalent at all. |
| Cowork Audit log viewer | Cowork | Missing | Missing — no audit-trail browser found (actions are logged to the local DB but not surfaced) | GAP | Neither other surface lets a user review a Cowork action audit trail. |
| Connector Settings panel (Cowork) | Cowork | Full — dedicated Connectors feature, 181-entry catalog, arguably richer | Full — `mcp`/`provider` connection management | FULL PARITY | The underlying "manage external connections" concept is real and substantive everywhere, even though scope/UI differs. |
| Intelli-Schedule panel | Cowork | Missing | Full — `IntelliScheduleEngine`, literally the same concept by name | GAP | Missing from iOS only; gizzi-code has the actual named engine likely backing this panel. |
| Harness Config panel (execution sandbox config) | Cowork | Missing | Loose — teleport/environment-runner manage remote execution environments, not sandbox harness config specifically | GAP | Missing from iOS; gizzi-code's overlap is adjacent infrastructure, not a direct match. |

## Code (developer/agentic coding workspace)

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Code workspace (CodeRoot) | Code | Real but thin — Code tab is chat-driven + a real pty terminal, no file editor/diff viewer/canvas | Full — `gizzi`/`run` against a repo IS the underlying agent that this workspace fronts | PARTIAL | gizzi-code is the originating engine and is full; iOS has a genuine but much thinner mobile version. |
| Code Explorer | Code | Missing — no file-tree view; only the pty terminal | N/A — the terminal itself is the file explorer | GAP | iOS has a live shell but no way to browse files without typing commands — a real, fixable gap. |
| Code Git panel | Code | Missing — no dedicated git UI (raw git usable via pty) | N/A — git is used directly in-terminal, more native there than a panel would be | GAP | iOS gap is real (raw pty git is not a UI); gizzi-code's terminal-native git access is a defensible reason it skips a GUI panel. |
| Code Threads (Recents) | Code | Full — Code tab session list | Full — `session` command | FULL PARITY | |
| Code Skills view | Code | Missing — Agent Hub is general-purpose, not code-scoped | Full — `skills` command | GAP | |
| Code Project view | Code | Partial — general Projects tab, no Code-mode-specific sub-workspace | Partial — `init` scaffolds per-project but no browsable view | PARTIAL | Both other surfaces have a thinner, generic version of "per-project workspace" rather than this scoped view. |
| Code Canvas (live preview split view) | Code | Missing — no live-preview pane | Partial — HTML artifact publish renders to the same canvas system, not a live-editing preview | PARTIAL | Loose gizzi-code overlap via artifact publish; iOS has nothing. |
| Code Focus (focused editing view) | Code | N/A | N/A — terminal editing is inherently a single-focus context | INTENTIONALLY SURFACE-SPECIFIC | A screen-real-estate convenience for a windowed IDE; doesn't map to a phone or a terminal. |
| Code Preview Pane | Code | Missing | Missing — output is run/viewed via CLI or browser directly, no in-app preview | GAP | A mobile live-preview of code output (e.g. a local web app) is plausible and would be genuinely useful; not a hard blocker. |
| Orchestrator Center | Code | Missing | Partial — `cowork-team` is the closest analog | PARTIAL | |
| Orchestration View | Code | Missing | Partial — same `cowork-team` overlap | PARTIAL | Likely the same underlying concept as Orchestrator Center, described twice. |
| Goal Control Center | Code | Missing | Full — standalone goal engine, `/goal` command, direct match | GAP | Missing from iOS only; strong, literal gizzi-code equivalent. |
| Kanban(+DAG) Board | Code | Missing | Full — `cowork-team` is explicitly "Team board / Kanban-style task assignment" | GAP | Missing from iOS only; direct, strong gizzi-code match. |
| Debug View | Code | Missing | Partial — `debug` command exists but is CLI/environment diagnostics, not agent-run debugging | PARTIAL | Different purpose (tooling the CLI itself vs. debugging an agent's run) but conceptually adjacent. |
| Logs View | Code | Missing | Partial — cost-tracker/stats and hooks logs exist as data, no unified logs UI | PARTIAL | |
| Run Inspector | Code | Missing | Partial — `verification` command's reasoning-pass review is a loose analog, as are cowork approvals | PARTIAL | |
| Run Replay | Code | Missing | Missing — no session-replay capability found in the CLI inventory | GAP | Likely the same underlying capability as "Replay Manager" under Terminal/Runtime/Infra, described in two places; genuinely absent from both other surfaces. |
| Tools Registry | Code | Missing | Partial — has built-in tools + MCP tools but no browsable registry UI (the terminal is the interface) | PARTIAL | |
| Skills Registry (SkillsRegistryView, "Memory" nav item) | Code | Missing | Full — `skills` command is a direct skills-registry browser | GAP | Missing from iOS only; strong gizzi-code match. |
| Promotion Dashboard | Code | Missing | Missing — no promotion/deploy-tracking found; `github` command is PR-automation-adjacent but not this | GAP | Absent from both other surfaces, no fundamental blocker to a CLI-side status view. |
| Usage Dashboard (Code token/compute usage) | Code | Partial — Settings has a general weekly Usage meter, not code-session-scoped | Full — `stats` command, arguably the richest cost/usage breakdown of the three | FULL PARITY | Some real form of usage/cost visibility exists everywhere, even though granularity differs. |
| Automation Tasks (Code) | Code | Missing | Full — same goal/routine/loop/cron engines as the Core Chat/Home item | GAP | Explicitly "shares component with Cowork's" in the source — same underlying feature as the Core Chat/Home Automation Tasks row, mode-scoped here. |

## ACI / Browser Automation

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| ACI Browser surface (BrowserCapsuleEnhanced) | ACI/Browser | Full — ACI tab: in-app WebKit browser + computer-use agent mode | Missing — no browser-automation tool/capability found in the CLI inventory | GAP | Web and iOS both have real browser-automation implementations; gizzi-code has no equivalent capability despite being a plausible fit (e.g. via a browser-control MCP tool). |
| Mini-apps Store | ACI/Browser | Missing | Partial — `plugin`/marketplace manages npm agent-skill plugins, a different kind of extensibility (not sandboxed UI-runtime embeds) | GAP | Missing from iOS; gizzi-code's plugin marketplace is an adjacent but not equivalent mechanism. |
| Mini-app Review Console | ACI/Browser | Missing | Missing | INTENTIONALLY SURFACE-SPECIFIC | Explicitly operator/admin-facing moderation tooling for a centralized marketplace — reasonably a web-admin-only surface. |
| Mini-app frame/runtime | ACI/Browser | Missing | Loose — plugin execution is a different sandboxing mechanism | GAP | iOS has no sandboxed extension-runtime concept at all. |
| Office Add-ins — Word | ACI/Browser | Missing | Missing | GAP | No fundamental blocker; just not built on either surface. |
| Office Add-ins — Excel | ACI/Browser | Missing | Missing | GAP | Same. |
| Office Add-ins — PowerPoint | ACI/Browser | Missing | Missing | GAP | Same. |
| Office & Extensions view | ACI/Browser | Missing | Missing | GAP | Combined view over the above; same gap. |
| Browser Extensions manager | ACI/Browser | N/A — no desktop browser process to manage extensions in | N/A | INTENTIONALLY SURFACE-SPECIFIC | Chrome-extension bridging requires a desktop browser process; both iOS sandboxing and the CLI context structurally exclude it. |
| Operator Browser | ACI/Browser | Likely the same capability as ACI's computer-use mode, branded differently | Missing — no browser-automation tool | GAP | gizzi-code lacks any browser-automation capability; web's Operator Browser and iOS's ACI computer-use mode are plausibly the same underlying feature under two names. |
| Hermes | ACI/Browser | N/A — can't install/monitor a local companion process | N/A — gizzi-code is itself a local agent runtime, doesn't need a GUI wrapper for another one | INTENTIONALLY SURFACE-SPECIFIC | Requires Electron preload + local process spawning; iOS can't spawn local processes and gizzi-code already occupies this role natively. |
| Oh My Pi | ACI/Browser | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Same reasoning as Hermes. |
| OpenClaw | ACI/Browser | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Same reasoning as Hermes; explicitly not even routed through the backend on desktop itself. |

## Design / Creative

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Design Mode (main: canvas/layers/properties) | Design/Creative | Missing | Missing | INTENTIONALLY SURFACE-SPECIFIC | Precision layer/canvas editing needs a large screen and pointer input; the CLI has no visual-rendering surface at all. |
| Design Mode — Questions tab | Design/Creative | Missing | Missing | GAP | A design-brief Q&A flow isn't inherently visual; could plausibly be a chat-like flow on either surface. |
| Design Mode — Mobile tab (mobile-design preview) | Design/Creative | Missing | Missing | GAP | Previewing mobile-target designs on an actual phone is a plausible, arguably natural fit — not built. |
| Design Mode — Video tab | Design/Creative | Missing | Missing | INTENTIONALLY SURFACE-SPECIFIC | Video/timeline editing is inherently visual, precision-scrubber work. |
| Design Mode — Docs tab | Design/Creative | Missing | Missing | GAP | Documentation generation isn't inherently visual. |
| Design Mode — Handoff tab (design-to-dev specs) | Design/Creative | Missing | Missing | GAP | Handoff specs are structured text/data, plausibly renderable or exportable on either other surface. |
| Design Mode — Graph tab (skill graph) | Design/Creative | Missing | Missing | GAP | Node graphs render adequately on mobile too, and gizzi-code could emit a text/tree representation. |
| Design Mode — Pipeline tab | Design/Creative | Missing | Missing | GAP | A workflow/pipeline view, not canvas-locked. |
| Design Marketplace/Registry | Design/Creative | Missing | Different-scope marketplace exists (skills/plugins, not design templates) | GAP | Missing from iOS; gizzi-code's marketplace covers different content, not a real equivalent. |
| Design Compare | Design/Creative | Missing | Missing | GAP | Side-by-side variant comparison is a common pattern elsewhere and not inherently desktop-only. |
| Form Surfaces (schema-driven forms for agent-human comms) | Design/Creative | Missing — chat is free text only | Missing | GAP | A structured form-rendering capability for approvals/structured input is absent from both, a real usability gap for e.g. approval flows. |
| Canvas Protocol (declarative task-surface catalog) | Design/Creative | Missing | Loose — HTML artifact publish is a distant cousin (structured render pipeline) | PARTIAL | |
| Allternit Canvas ("The Computer", core artifact rendering) | Design/Creative | Full — Artifacts Library detail viewer with embedded web view renders the same artifact types | Full — HTML artifact publish feeds exactly this system | FULL PARITY | Explicitly called out in GIZZI.md as the one deliberate example of a gizzi-code→platform bridge; the rendering capability is genuinely present everywhere. |
| Design Team Workspace | Design/Creative | Missing | Missing | GAP | No collaborative design tool equivalent on either other surface. |
| Content Pipeline | Design/Creative | Missing | Missing | GAP | |
| HyperFrames Timeline Editor | Design/Creative | Missing | Missing | INTENTIONALLY SURFACE-SPECIFIC | Frame-precision timeline editing genuinely needs a large canvas + scrubber; not terminal- or phone-renderable in any meaningful form. |
| Live Artifact Editor | Design/Creative | Partial — Artifacts Library viewer is read/view-only, no live in-place editing | Partial — files backing an artifact can be edited directly via the terminal, functionally live but not through this UI | PARTIAL | |

## Terminal / Runtime / Infra Ops

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Terminal (in-app terminal view) | Terminal/Infra | Full — real pty terminal (SwiftTerm) attached to a gizzi-code server, arguably more literal than web's view | Full — gizzi-code IS a terminal application | FULL PARITY | |
| Monitor (live agent dashboard, pause/resume/restart) | Terminal/Infra | Missing | Partial — `stats` gives usage data but no live pause/resume/restart controls | GAP | No live agent-control dashboard on either other surface, though gizzi-code has partial underlying data. |
| Runtime Operations | Terminal/Infra | Missing | Missing — `doctor`/`debug` are adjacent dev diagnostics, not this | GAP | |
| Budget Dashboard | Terminal/Infra | Thin — weekly Usage meter in Settings | Full — `stats` cost tracking is a strong direct analog | PARTIAL | Some form exists everywhere; web's dedicated dashboard is richest, iOS's is thinnest. |
| Replay Manager | Terminal/Infra | Missing | Missing | GAP | Same underlying gap as Code's "Run Replay," described in two sections. |
| Prewarm Manager (pre-warmed environment pool) | Terminal/Infra | Missing | Missing — teleport/environment-runner manage remote envs but don't pre-warm a pool | GAP | Leans internal-ops, but per the audit's own instruction this should still be checked honestly rather than waved through — genuinely absent elsewhere. |
| Nodes (node/cluster management) | Terminal/Infra | Missing | Missing — `runtime` command is local-only discovery, not cluster management | GAP | |
| Cloud Deploy (deployment wizard) | Terminal/Infra | Missing | Missing — no cloud-deploy command found (`serve`/`allternit` launch things but don't wizard a cloud deploy) | GAP | |
| Capsule Manager (MCP Interactive Capsules) | Terminal/Infra | Missing | Full — `mcp` command manages MCP servers directly, likely the actual backing system for this panel | GAP | Missing from iOS only; gizzi-code's `mcp` command is the probable underlying capability. |
| VPS & Servers panel | Terminal/Infra | Missing | Missing — no SSH/VPS management command found | GAP | |
| Cloud Instances panel | Terminal/Infra | Missing — not in iOS's Settings section list | Missing | GAP | |
| Enterprise BYOC panel | Terminal/Infra | Missing | Missing | GAP | Same item recurs under Onboarding & Account below (it's a Settings sub-section too). |

## Agentic subsystem / advanced internals (DAG suite)

Per the phase task's explicit guidance: iOS's absence is often defensible
(the section's own summary calls this "engineering/debug-facing rather
than mainstream end-user"), used here as INTENTIONALLY SURFACE-SPECIFIC
*for the iOS comparison specifically* — but each item's gizzi-code
comparison is checked independently on its own technical merits, since a
CLI is exactly where an engineering capability plausibly belongs.

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| DAG Integration Page (umbrella) | DAG suite | N/A — debug-facing per its own description | Partial — no single hub, but many sub-capabilities exist piecemeal across separate commands (see rows below) | PARTIAL | |
| Policy Manager | DAG suite | N/A | Full — Permissions system (`permission/`, `/permissions`), a strong direct match | INTENTIONALLY SURFACE-SPECIFIC | iOS reasonably excluded as a debug/technical panel; gizzi-code's permissions system is a full, arguably more end-user-facing equivalent (interactive approval prompts). |
| Policy Gating | DAG suite | N/A | Full — same Permissions/tool-guard system, enforcement side | INTENTIONALLY SURFACE-SPECIFIC | Same match as Policy Manager. |
| Task Executor | DAG suite | N/A | Partial — functionally covered by the cowork/goal/routine engines, no single named "Task Executor" | INTENTIONALLY SURFACE-SPECIFIC | |
| Ontology Viewer | DAG suite | N/A | Missing — no knowledge-ontology browser found; `brain`/vault are memory-adjacent but not this | GAP | A real finding: no CLI-side way to browse the system's ontology despite this being a natural introspection fit for a technical tool. |
| Directive Compiler | DAG suite | N/A | Missing — the goal engine takes high-level goals but doesn't "compile directives" in this sense | GAP | Real gap: no CLI equivalent for compiling high-level directives into executable plans. |
| Evaluation Harness | DAG suite | N/A | Full — `verification` command, a strong direct match ("verifies code changes using a semi-formal reasoning pass") | INTENTIONALLY SURFACE-SPECIFIC | Strong gizzi-code match. |
| GC Agents (garbage-collection/lifecycle) | DAG suite | N/A | Missing — no lifecycle/GC agent management found | GAP | |
| Receipts Viewer (cryptographic/audit receipts) | DAG suite | N/A | Missing — no receipts/audit-trail system found; session export is raw data, not receipts | GAP | Real gap worth flagging: an audit-receipts capability exists only in the web debug UI with nothing generating or viewing receipts CLI-side. |
| Security Dashboard | DAG suite | N/A | Missing as a self-monitoring dashboard — the `security-compliance` skill bundle is an agent capability for doing security work, not the CLI's own posture view | GAP | Distinct concepts: a skill for the agent to use vs. a dashboard about the CLI's own security posture. |
| Purpose Binding (governance: bind actions to declared purpose) | DAG suite | N/A | Missing — Permissions system is adjacent but doesn't capture this governance concept | GAP | |
| DAG WIH (Work Item Handling) | DAG suite | N/A | Partial — cowork/cowork-team work-item concepts are a close, less formal analog | INTENTIONALLY SURFACE-SPECIFIC | |
| Checkpointing | DAG suite | N/A | Partial — teleport's stash/resume is a real checkpoint-like mechanism | INTENTIONALLY SURFACE-SPECIFIC | Reasonable gizzi-code match via teleport. |
| Observability Dashboard | DAG suite | N/A | Partial — cost-tracker/stats provide some telemetry, not full execution-trace observability | PARTIAL | |
| IVKGE Panel | DAG suite | N/A | Unclear — `brain`/vault are memory-adjacent, no explicit knowledge-graph equivalent found | UNCLEAR — needs code-level check | Description is too vague ("knowledge-graph-adjacent panel") to confidently classify from the inventory alone. |
| Multimodal Input (testing UI) | DAG suite | N/A (iOS's chat has production multimodal *input*, a different purpose than a dev test harness) | Missing — no multimodal input testing tool found | GAP | gizzi-code lacks even a dev-facing multimodal test tool. |
| UI Forge (tambo, dynamic UI generation) | DAG suite | N/A | Missing | INTENTIONALLY SURFACE-SPECIFIC | Generating dynamic web UI components is inherently tied to a web rendering target; the concept itself doesn't transfer to a CLI, not just an unbuilt feature. |
| Hooks System | DAG suite | N/A | Full — `runtime/hooks`, `/hooks`, literally the same name and concept | INTENTIONALLY SURFACE-SPECIFIC | iOS reasonably excluded as a technical config surface; strong, identically-named gizzi-code match. |
| Evolution Layer (memory/skill/workflow self-improvement) | DAG suite | N/A | Missing — no self-modifying "evolution" engine found beyond skills/brain | GAP | Real gap: gizzi-code lacks this despite self-improvement being a very CLI-agent-native concept. |
| Context Control Plane (git-based context controller) | DAG suite | N/A | Missing — no git-branch-based context-control system found | GAP | |
| Memory Kernel (3-layer memory: events/entities/edges) | DAG suite | Full — Memory Settings view (browse/search backend long-term-memory docs) | Full — `brain` command (persistent knowledge, remember/recall) | FULL PARITY | Genuine equivalent memory-browsing/management capability on all three, even if the underlying architecture (3-layer graph vs. simpler store) differs. |
| Autonomous Code Factory (self-improving codegen pipeline) | DAG suite | N/A | Partial — the core `gizzi`/`run` agent loop does autonomous code generation, just not architected/branded as a distinct self-improving "factory" | INTENTIONALLY SURFACE-SPECIFIC | |
| Swarm ADE (multi-agent swarm dashboard) | DAG suite | Missing | Partial — `swarm.ts` unwired top-level file, reachable via `/swarm` TUI command: "run several agents in parallel... with a visual task tracker" | PARTIAL | Real gizzi-code capability, likely thinner than web's 34-file dashboard; missing from iOS entirely. |
| Runner (Agent Runner / DAK) | DAG suite | N/A | Partial — the underlying primitives (context/session mgmt, goal/routine planning, session export, agent-hub templates) exist scattered across several commands, no unified console | INTENTIONALLY SURFACE-SPECIFIC | |
| H5I panel — Agent Hooks | DAG suite | N/A | Full — same Hooks system match as above | INTENTIONALLY SURFACE-SPECIFIC | |
| H5I panel — Audit | DAG suite | N/A | Missing — same gap as Receipts Viewer | GAP | |
| H5I panel — Commit | DAG suite | N/A | Partial — commits happen directly and transparently via terminal/agent tool calls, a different but real form of visibility | PARTIAL | |
| H5I panel — Context | DAG suite | N/A | Missing — no dedicated context inspector; `debug` has some file/introspection tooling | GAP | |
| H5I panel — Diff | DAG suite | N/A | Partial — diffs are visible natively in-terminal when the agent proposes changes | PARTIAL | |
| H5I panel — MCP | DAG suite | N/A | Full — `mcp` command, likely more capable than the panel it backs | INTENTIONALLY SURFACE-SPECIFIC | Strong gizzi-code parity, arguably the richer implementation. |
| Changeset Review (diff cards, approve/reject) | DAG suite | Missing — iOS's Code tab runs agentic coding sessions but has no described way to review/approve a diff before it lands | Full — cowork approval/HITL flow + native terminal diffs | GAP | Genuine, actionable gap: a mobile user can kick off an agentic coding session but apparently can't review changes before they're applied. |

## Marketplace / Plugins / Skills

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Marketplace (top-level) | Marketplace/Plugins | Missing — Connectors is narrower in scope | Partial — plugin marketplace exists, different content | PARTIAL | |
| Plugin Registry / Plugin Marketplace (480-file built-in catalog) | Marketplace/Plugins | Missing | Partial — `plugin` command + plugin marketplace, npm-package skill plugins, differently scoped than web's consumer app catalog | PARTIAL | |
| Team Skills panel (org-level shared skills) | Marketplace/Plugins | Missing | Partial — `skills`/agent-hub manage skills but not explicitly at an org-shared level | GAP | |
| MiroFish simulation engine | Marketplace/Plugins | Missing | Missing | GAP | A genuinely distinct product feature with no equivalent elsewhere; no fundamental blocker to at least a text-based CLI version. |

## Products / Discovery / Learning

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Products Discovery | Products/Discovery | Missing | Missing | GAP | |
| A://Labs (experimental features) | Products/Discovery | Missing | Missing (unwired/orphaned commands are the closest informal analog) | GAP | |
| Udemy Catalog | Products/Discovery | Missing | Missing | GAP | Lower-priority third-party integration, but still a real, uncontested gap per the rubric. |
| Discovery Feed | Products/Discovery | Missing | Missing | GAP | |
| Research tab/panel | Products/Discovery | Missing (ACI/chat can browse for research ad hoc, no dedicated panel) | Partial — `search-knowledge` skill bundle is an agent capability shaped like this, not a dedicated panel | PARTIAL | |

## Mail / Knowledge

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Mail Monitor | Mail/Knowledge | Missing | Partial — unwired `vault.ts` mentions Gmail/Calendar sync as a closest analog, not shipped | GAP | Missing from iOS; gizzi-code's overlap exists only as an unwired/inactive feature. |
| Documents (office-file I/O) | Mail/Knowledge | Thin — Attachments support files but no document-editing/workflow surface | N/A — gizzi-code edits real files on disk directly | GAP | Real gap on iOS (can't open/edit an Office doc on mobile); gizzi-code's absence is defensible since it's file-native. |
| Knowledge (stub — not implemented on web itself) | Mail/Knowledge | Thin — Memory Settings is the closest browsing surface | Full — `brain`/vault are actual working knowledge-management capabilities, more built-out than web's own empty stub | PARTIAL | Notable inversion: web's own "Knowledge" directory is an unimplemented stub, so gizzi-code is arguably ahead here, not behind. |
| Jobs (stub — not implemented on web itself) | Mail/Knowledge | Missing | Missing | UNCLEAR — needs code-level check | Unimplemented everywhere including on web itself; nothing to meaningfully cross-reference until the feature is scoped at all. |

## Onboarding & Account

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Onboarding Flow / Guided Tour | Onboarding/Account | Full — 4-page flow, persona picker, starter-task cards, arguably richer in personalization | Full — `init` + `/onboarding` scaffold identity/memory files | FULL PARITY | Concrete flows differ appropriately by surface (human tour vs. project scaffold) but the onboarding concept is real everywhere. |
| Settings (umbrella/shell) | Onboarding/Account | Full — grouped settings hub | Partial — no single settings command; config is distributed across `provider`/`connect`/`mcp`/`output-style`/`permissions` | PARTIAL | |
| Settings > Account (Sign-in, Org & Access, Usage, Plans & Compute, Billing, Privacy) | Onboarding/Account | Partial — has Account + Usage sections; no Org & Access or Plans & Compute/billing UI described | Missing — `connect`/`provider` handle auth only, no org-access or billing management | PARTIAL | Sign-in/Account/Usage are solid parity; Org Access and Billing specifically are absent from both other surfaces. |
| Settings > Platform (General, Appearance, Models, API Keys, Shortcuts, Permissions, Dispatch, Devices, Cloud Instances, Diagnostics) | Onboarding/Account | Thin — Capabilities section is the closest, doesn't clearly cover most of these | Partial — `models`, `provider` (API keys), keybindings (shortcuts, CLI-only), Permissions system, `pair` (devices) each individually cover a slice | PARTIAL | gizzi-code covers most of this category piecemeal via separate commands; iOS covers comparatively little of it explicitly. |
| Settings > Products (Gizzi Code settings, Cowork settings, Extensions) | Onboarding/Account | Missing | N/A — gizzi-code doesn't need settings about itself as a product | GAP | Missing from iOS; gizzi-code's absence for its own self-referential settings is defensible, but iOS's absence isn't. |
| Settings > Infrastructure (Infrastructure, VPS & Servers, Enterprise BYOC, Environment, Security, Agents) | Onboarding/Account | Missing | Thin — provider/environment config exists informally, no VPS/BYOC management | GAP | Real finding: infra management is thin-to-absent on both other surfaces, even though gizzi-code as "the brain" is a plausible place for CLI-driven infra config. |
| Settings > Customize (Skills, Response Style, Connectors, Allternit Plugins) | Onboarding/Account | Partial — has Response Style + Connectors (full), no Skills customization | Full — `skills`, output-style, `mcp`/`provider` (connectors-ish), `plugin` all present | PARTIAL | Response Style is a confirmed direct FULL PARITY sub-item across all three (flagged explicitly in the source map as a good parity check); Skills customization specifically is missing on iOS. |
| Settings > About | Onboarding/Account | Full — has an About section | Implicit — no dedicated "about," version info presumably available but not explicitly listed | FULL PARITY | Trivial but genuinely present everywhere in some form. |
| Device Pairing panel | Onboarding/Account | Not described as an approver — iOS is the device being paired to elsewhere, but no explicit "approve a `gizzi pair` request" UI found | Full — `pair` command is the literal other half of this exact feature | GAP | A mobile user apparently can't approve `gizzi pair` requests from their phone, only from web/desktop; web and gizzi-code are the two real halves of one integrated feature. |
| Organization Access panel | Onboarding/Account | Missing | Missing — tenancy lives only in the cloud-api backend, no CLI-side org/membership management | GAP | Arguably reasonable that an agent CLI doesn't manage org membership (identity/admin plane, not agent capability), but nothing structurally prevents it — not built, not clearly out of scope. |
| Compute Billing panel | Onboarding/Account | Missing | Partial — `stats` gives cost/usage data, not billing/plan management | GAP | |
| Enterprise BYOC panel | Onboarding/Account | Missing | Missing | GAP | Same item as the Terminal/Infra section's BYOC row — it's both a Settings sub-section and a standalone panel in the source inventory. |
| Model Management view | Onboarding/Account | Unclear — Capabilities section might cover this, not detailed enough to confirm | Full — `models` + `provider` commands, a strong direct match | PARTIAL | gizzi-code match is clear and strong; iOS is genuinely unclear from the inventory description alone. |

## Voice / Local Models

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Voice Service (Speech-to-Text) | Voice/Local Models | Full — full ambient Voice Mode + Dictation + on-device TTS, arguably the richest of the three | Full — `voiceModeEnabled`/`/voice`, toggleable TUI voice interaction | FULL PARITY | iOS is actually the strongest implementation here, not the laggard. |
| Local Models (router/catalog for local model providers, loopback) | Voice/Local Models | N/A — no local-model-loopback concept; running local LLMs on a phone is impractical (compute/battery/storage) | Partial — `provider`/`connect`/`models` manage LLM providers generally, plausibly including local ones, though not confirmed as loopback-specific | INTENTIONALLY SURFACE-SPECIFIC | iOS exclusion is a real hardware/OS constraint, not an oversight; gizzi-code has a plausible partial equivalent via its provider system. |

## AllternitOS ("Computer" meta-environment)

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| AllternitOS (kernel/windowing/installable "programs") | AllternitOS | N/A — needs native filesystem + window-management APIs that iOS sandboxing forbids | Missing — no equivalent "installable programs" model, though several individual programs have loose skill-bundle analogs (research-doc ~ vault, data-grid ~ data-sql skill bundle) | GAP | iOS's absence is structurally defensible; gizzi-code's absence is the more actionable finding since several of AllternitOS's "programs" are conceptually close to things gizzi-code's skill bundles already do piecemeal, just not as an installable-program model. |

## Playground / Verification / QA

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Playground (model-parameter workbench) | Playground/QA | Missing | Partial — the normal interactive `gizzi` session plus `models`/`provider` function as a de facto playground, no formal parameter-tuning UI | PARTIAL | |
| Verification View | Playground/QA | Missing | Full — `verification` command, literally named the same concept | INTENTIONALLY SURFACE-SPECIFIC | iOS reasonably excluded as a technical/QA tool; strong, direct gizzi-code match. |
| QA (internal QA utility) | Playground/QA | Missing | Missing — `debug` tooling is the closest analog | INTENTIONALLY SURFACE-SPECIFIC | Explicitly small/internal engineering tooling, not meant to be end-user-facing on any surface. |

## Empty / not-yet-implemented stubs found

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| `views/gizzi`, `components/mesh`, `lib/mesh-network` (empty stub dirs) | Empty stubs | **Real, working feature** — embedded tsnet node joining a private Headscale tailnet, surfaced in Settings' Mesh section | Unclear — gizzi-code is often the thing reached *over* the mesh rather than a client joining one itself | PARTIAL | Notable inversion of the usual pattern: web/desktop's own "mesh" directory is an empty, unimplemented stub while iOS has a real, functioning Mesh networking feature — iOS is ahead here, not behind. |

## Desktop-only / platform-specific (Electron bridge items)

Per the phase task's instruction, checked each of these for a
non-obvious cross-surface equivalent before defaulting to INTENTIONALLY
SURFACE-SPECIFIC — one (`runtime` command / local-runtime discovery) does
have a real gizzi-code equivalent and is reclassified accordingly.

| Item | Section | iOS | gizzi-code | Classification | Note |
|---|---|---|---|---|---|
| Local runtime discovery (`agent-workspace/discovery.ts`, `runtime-client.ts`) | Desktop-only | N/A — sandboxed mobile app, no local-process discovery | Full — `runtime` command explicitly manages local agent-runtime discovery/registration | PARTIAL | The one item in this list with a real, named gizzi-code equivalent — flagged explicitly by the phase task as worth checking before assuming no overlap, and it does overlap. |
| Native filesystem access (`plugins/fileSystem.ts`, `FileSystemService.ts`) | Desktop-only | N/A — app-sandboxed, no native FS browser | Full — gizzi-code operates on real local files directly via its tools, arguably broader access than the desktop plugin sandbox | INTENTIONALLY SURFACE-SPECIFIC | iOS sandboxing is the hard blocker; gizzi-code's native FS access is the CLI-side equivalent capability, just not packaged as this same plugin system. |
| Desktop window chrome (`FloatingWidgets.tsx`, `ControlCenter.tsx`) | Desktop-only | N/A — iOS inventory explicitly confirms no WidgetKit/floating-widget equivalent was built | N/A — terminal has no windowing-chrome concept | INTENTIONALLY SURFACE-SPECIFIC | Desktop-OS window-chrome integration; iOS's absence is explicitly confirmed rather than assumed. |
| Computer-use OS control engine (`computer-use-engine.ts`) | Desktop-only | N/A — ACI's "computer-use" mode controls the in-app browser/viewport only, not the host OS (different scope despite similar naming) | Missing — no OS-control capability found | INTENTIONALLY SURFACE-SPECIFIC | Controlling the actual host OS needs desktop-level privileges iOS sandboxing forbids; note the naming false-friend with ACI's browser-scoped "computer-use." |
| AllternitOS Electron IPC bridge (`electron.d.ts`, `KernelBridge.ts`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | IPC plumbing for the AllternitOS feature already assessed above, not an independent feature. |
| Native screenshot capture (`BrowserScreenshotService.ts`, citation manager) | Desktop-only | N/A — no general screenshot-capture API used for this purpose (SFSafariViewController is used only for OAuth/support links) | Missing | INTENTIONALLY SURFACE-SPECIFIC | Scoped to AllternitOS's citation-manager program, itself desktop-only. |
| Local Python execution (`PythonExecutionService.ts`) | Desktop-only | N/A — sandboxed, can't run arbitrary local Python | Full — gizzi-code can execute Python via its own shell/tool-call capabilities, a genuinely stronger equivalent | PARTIAL | gizzi-code's general coding-agent tool access is a real, arguably better equivalent mechanism; iOS is properly excluded by sandboxing. |
| Onboarding desktop/web branch (`InfrastructureStep.tsx`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | An infrastructure-setup onboarding step only makes sense where there's local infra to configure — implementation detail of the Onboarding Flow row above, not an independent feature. |
| Chrome-extension bridge (`BrowserCapsuleEnhanced`, `useExtensionBridge.ts`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Same underlying capability as the "Browser Extensions manager" row in the ACI section above. |
| Platform-detection / native window-opening (`lib/platform.ts`, `open-code-session-window.ts`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Internal plumbing code, not an independently mirrorable feature. |
| Settings desktop/web auth branch (`SettingsView.tsx`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Implementation detail of the Settings row above (desktop-local vs. hosted Clerk auth branching), not a distinct feature. |
| Mini-app/Office native-rendering fallback (`MiniAppRuntimeSurface.tsx`, `open-office-web.ts`) | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Rendering-preference fallback logic for the Mini-app/Office Add-in rows already classified above as GAPs on their own merits. |
| Hermes/OpenClaw/Oh-My-Pi `window.allternit` bridge | Desktop-only | N/A | N/A | INTENTIONALLY SURFACE-SPECIFIC | Same items already classified individually in the ACI/Browser section above. |

---

**Row count: 171.** See `docs/learnings/SURFACE_AUDIT_PHASE_1_NOTES.md` for the
count breakdown, granularity rationale, and items a human should resolve.
