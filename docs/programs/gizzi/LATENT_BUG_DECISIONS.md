# Latent runtime bug decisions — owner review

**What this is.** On 2026-09-18 a full day of typing / lint / burn-down work across
`Allternit/allternit-platform` (PRs #577–#630) and `Allternit/allternit-ai` (PRs #23–#41)
surfaced ~20 latent runtime bugs. The agents **deliberately did not fix them**: the burn-down
and lint lanes were under type-only / behavior-preserved rules, so every finding was kept
byte-semantically identical behind a cast, a guard, or a documented `@ts-nocheck` header
comment, and flagged in the PR body or the `agent-ledger/summaries/2026-09-18-*` files for an
owner decision. This document re-verified every finding against current `main` before giving
it a row — nothing below entered the table without a current-main read proving it is still
broken (or a DONE row naming the fixing PR).

**How to use it.** Mark one DECISION per row: **FIX** (execute the fix plan in the row),
**DEFER** (leave the cast/guard in place; record why), or **WON'T** (accepted as-is; record
why). "Fix all" means every non-DONE row gets FIX. Then execute per the appendix at the
bottom; each row lists files, the change, effort (S < ~1h, M ~half day, L > half day), and
risk.

**Verification base.** Platform first read against `origin/main` @ `5ca613f8c`
(2026-09-18 attestation for burn b0015), then re-verified after burn b0016 landed
(`origin/main` @ `ba75db051`) — no platform row was touched by b0016. allternit-ai read
against `origin/main` @ `53cbeca6` (merge of Allternit/allternit-ai#41). Line numbers refer to
the later of the two platform trees.

**Provenance.** Findings flagged in: platform PRs
[#598](https://github.com/Allternit/allternit-platform/pull/598),
[#599](https://github.com/Allternit/allternit-platform/pull/599),
[#602](https://github.com/Allternit/allternit-platform/pull/602),
[#612](https://github.com/Allternit/allternit-platform/pull/612),
[#613](https://github.com/Allternit/allternit-platform/pull/613),
[#616](https://github.com/Allternit/allternit-platform/pull/616),
[#620](https://github.com/Allternit/allternit-platform/pull/620);
allternit-ai PRs
[#30](https://github.com/Allternit/allternit-ai/pull/30),
[#31](https://github.com/Allternit/allternit-ai/pull/31),
[#32](https://github.com/Allternit/allternit-ai/pull/32),
[#33](https://github.com/Allternit/allternit-ai/pull/33),
[#34](https://github.com/Allternit/allternit-ai/pull/34),
[#35](https://github.com/Allternit/allternit-ai/pull/35),
[#36](https://github.com/Allternit/allternit-ai/pull/36),
[#37](https://github.com/Allternit/allternit-ai/pull/37),
[#38](https://github.com/Allternit/allternit-ai/pull/38),
[#40](https://github.com/Allternit/allternit-ai/pull/40),
[#41](https://github.com/Allternit/allternit-ai/pull/41);
ledger summaries `agent-ledger/summaries/2026-09-18-{0930,1136,1241-agent-43,1300,1349,1420,1838,1908,2015}-*`
(burn batches b0001–b0015) and the dormant-stub decision doc (`DORMANT_STUB_DECISIONS.md`, PR #610).

## Decision table — still broken on current main

### Platform (`cmd/gizzi-code`, this repo)

| # | Bug | What's broken (verified on main) | User-visible symptom | Fix approach | Effort | Risk | Source | Owner decision |
|---|-----|----------------------------------|----------------------|--------------|--------|------|--------|----------------|
| P1 | Dead vfkit VM execution path in cowork runtime | `cmd/gizzi-code/src/runtime/cowork/cowork.runtime.ts:28-39` — `getVfkitManager()` dynamically imports `createVFKitManager` from `@/runtime/vm`, but that module now exports only the Lima surface (`src/runtime/vm/index.ts:1-2`). `case "vm"` at `:62-63` → `executeVM` (`:135`) can never succeed. (The cron executor's VM path, `automation/cron/executors/cowork-executor.ts:242`, is fine — sandbox API + bubblewrap fallback.) | Any cowork run with `mode: "vm"` fails immediately: `createVFKitManager is not a function`, run marked failed. | Two options: **(a) cut** — delete `getVfkitManager`/`executeVM`/`case "vm"` and narrow the mode union (vfkit was removed in the 2026-09 cleanup, Lima replaced it); **(b) port** — rewire `case "vm"` to the Lima executor (`executeInVM` from `@/runtime/vm/lima-executor`). | S (cut) / M (port) | Low (cut — the path is dead today); Medium (port — new live execution surface) | PR #612; ledger `2026-09-18-1349-b0009` | **DECISION: FIX (done, this PR)** — cut: `getVfkitManager`/`executeVM` and the unused `VmSession` import deleted; a vm-mode run now fails with a descriptive error pointing at the cron CoworkExecutor. "vm" stays a valid stored/API mode (the b0142 schema fix adds it to the drizzle mode union), so the mode union was NOT narrowed. |
| P2 | `Plugin.trigger("shell.env")` undeclared in SDK `Hooks` type | `cmd/gizzi-code/packages/plugin/src/index.ts:21-47` — the `Hooks` interface has no `"shell.env"` entry, yet three runtime call sites trigger it: `src/runtime/tools/builtins/bash.ts:172-175`, `src/runtime/integrations/pty/index.ts:334-337` (both cast the hook name `as any`), `src/runtime/session/prompt.ts:2126-2131`. | None today — the trigger dispatches by string name at runtime. The hazard is the SDK type silently diverging from the runtime contract; plugin authors get no typing for a hook the core relies on. | Add `"shell.env"` to `Hooks` with the payload shape observed at the three call sites (input `{cwd, sessionID, callID}`, output `{env: Record<string,string>}`); then drop the two `as any` casts. Type-only + cast removal, no runtime change. | S | Low | PR #612; ledger `2026-09-18-1420-b0010` | **DECISION: FIX (done, this PR)** — five runtime-triggered hooks declared in the SDK `Hooks` type (`shell.env`, `tool.definition`, `experimental.chat.system.transform`, `chat.params`, `experimental.text.complete`) with payload shapes from the call sites; all six `as any` hook-name casts dropped (bash.ts, pty/index.ts, registry.ts, agent.ts, llm.ts ×2, processor.ts). Tracked plugin dist rebuilt. |
| P3 | Dead `USER_TYPE === 'ant'` constant-comparison family | Two patterns coexist: `process.env.USER_TYPE === 'ant'` reads (~90 sites, genuine runtime gating — `USER_TYPE` is build-time `--define`'d per `src/shared/utils/envUtils.ts:148-150`) and committed literals `"external" === 'ant'` / `"production" === 'development'` (~40 sites in REPL.tsx, PromptInput*.tsx, AgentTool.tsx, ultraplan.tsx, DevBar.tsx, useBuddyNotification.tsx, processSlashCommand.tsx, etc.). `AgentTool.tsx:1288` documents the intent: the literal enables dead-code elimination in external builds. The one genuine bug in this family — `classifierDecision.ts` gating `VERIFY_PLAN_EXECUTION_TOOL_NAME` on `USER_TYPE === 'ant'` while `tools.ts` gated the tool on `GIZZI_CODE_VERIFY_PLAN` — was already fixed (PR #613, see DONE table). | None by design in shipped external builds (Bun DCE removes the ant-only blocks). Latent: in dev/unbundled runs the ant-only surfaces silently never activate, and any literal site the build define misses ships permanently dead. | This is an intentional DCE pattern, not a defect — recommended **WON'T** with hygiene: centralize behind one `isAntBuild()` constant (or annotate the family) so future burn batches stop re-flagging it site-by-site; sweep only for sites that *should* be live in dev. | M (mechanical sweep) | Low–Medium (touching gates) | PR #613; ledgers b0012/b0013 (`2026-09-18-1908`, `2026-09-18-2015`) | **DECISION: WON'T** — intentional DCE pattern per the row's own recommendation; the one genuine gate bug in the family is already fixed (PR #613). The isAntBuild() centralization hygiene is deferred to a future sweep (not part of "fix all" — it is a refactor of a deliberate pattern, not a bug fix). |
| P4 | `identity.creature` dead accessor in workspace loader | `cmd/gizzi-code/src/runtime/workspace/workspace-loader.ts:159-161` — reads `(workspace.identity as { creature?: string }).creature`, but `WorkspaceIdentity` has no `creature` field; the value is always `undefined`. TODO(types) comment in tree documents it. | Workspace identity agents always get the fallback description `"<type> workspace identity agent"` instead of a creature-specific one. | Either add `creature?: string` to `WorkspaceIdentity` and populate it where workspaces are created, or delete the accessor and inline the fallback. | S | Low | Ledger `2026-09-18-1908-b0012` | **DECISION: FIX (done, this PR)** — accessor deleted, fallback inlined. The "add + populate" option was rejected: no identity source (IDENTITY.md parsing, manifest, layered workspace) produces a `creature` value anywhere in the tree, so there was nothing to populate from. |
| P5 | `reduceAnsiCodes` called with fantasy 2-arg shape | `cmd/gizzi-code/src/shared/utils/textHighlighting.ts:166` — `reduceAnsiCodes(codes, [])`; the real `@alcalzone/ansi-tokenize` 0.2.5 signature takes 1 arg. Extra arg is ignored → current runtime no-op. | None today (ignored argument). Hazard: any future real 2-arg overload changes behavior silently. | Drop the `, []`. One line; belongs in the next burn batch that touches the file. | S (trivial) | None | PR #599 | **DONE** — verified gone on current main; fixed by burn b0102 (commit `0f7c246aa`, which also removed the file's `@ts-nocheck`). |

### Platform — b0142 escalations (burn batch `ao/ts-burn-next3`, PR #646)

Burn batch b0142 (ledger `2026-09-18-2215-ao-ts-burn-next3-kimi-code-ts-burn-b0142`) escalated
two files whose `@ts-nocheck` headers hid real runtime bugs. Owner decision (2026-09-18, same
"fix all" approval): **FIX both**, executed in the same PR as P1/P2/P4.

| # | Bug | Fix | Owner decision |
|---|-----|-----|----------------|
| B1 | `src/runtime/util/filesystem.ts` — the `Filesystem` namespace's exported `stat` shadowed the `fs/promises` import, so `Filesystem.stat()` was infinitely self-recursive and `isDir`/`isFile`/`copy` built on it. Dormant (no live importer called the broken methods). | Aliased the import (`stat as fsStat`) and repointed all eight internal call sites (top-level helpers + namespace). Also widened the namespace `glob` options type to `{ cwd?, absolute? }` to match the `globUp` call. `@ts-nocheck` header kept (file is queued in burn batch b0180). Regression test: `test/runtime/util-filesystem.test.ts`. | **DECISION: FIX (done, this PR)** |
| B2 | `src/runtime/cowork/cowork.service.ts` — hand-written `Run`/`RunEvent`/`Schedule`/`Approval`/`Checkpoint` interfaces declare `created_at`/`updated_at` (plus `responded_at`/`restored_at` and boolean `enabled`/`resumable`), but the drizzle schema stores `time_created`/`time_updated`/`time_responded`/`time_restored` (via `Timestamps`) and integer boolean columns. The `as Run[]` casts returned rows whose `created_at` was `undefined` at runtime, which `src/runtime/server/routes/cowork.ts` (`created_at: z.number()`, `enabled/resumable: z.boolean()`) and the CLI (`new Date(run.created_at)`) choked on. The schema's mode `$type` also omitted `"vm"` that `RunMode` declares and `RunService.create` accepts. | Added boundary row mappers for all five entities (timestamps + integer-boolean → declared shape) applied at every read site, and added `"vm"` to both schema mode `$type` unions (`RunTable`, `ScheduleTable`) to match `RunMode`. `@ts-nocheck` header kept (file is queued in burn batch b0180). Regression test: `test/runtime/cowork-service-mapping.test.ts`. | **DECISION: FIX (done, this PR)** |

**Latent finding surfaced while testing B2 (not fixed here, out of scope):** the cowork
migration (`migration/20260414094100_cowork_runtime/migration.sql`) contains no
`--> statement-breakpoint` separators, so `Database.applyMigrations` (which splits on that
marker) executes only its first statement on a fresh database — `cowork_run` exists but
`cowork_run_event`/`cowork_schedule`/`cowork_approval`/`cowork_checkpoint` are never created.
Any fresh gizzi data dir that reaches the cowork surfaces before a manual fix hits
"no such table" errors. Follow-up: regenerate the migration with statement breakpoints (or
apply the statements individually).

### Agent workspace (`Allternit/allternit-ai`)

| # | Bug | What's broken (verified on main) | User-visible symptom | Fix approach | Effort | Risk | Source | Owner decision |
|---|-----|----------------------------------|----------------------|--------------|--------|------|--------|----------------|
| A5 | Design/marketing tool handlers receive context-as-args | `src/lib/agents/tools/index.ts:217-223` registers 7 tools by passing `tool.execute` (each a single-args-object function, e.g. `skill-graph.tool.ts:24-26` `execute: (args: SkillGraphArgs) => …`) as a `ToolExecutionHandler`. But `ToolExecutionHandler` is `(context, parameters)` and `executeTool` calls `handler(context, parameters)` (`index.ts:161`). The AgentCommunication tool two lines above (`:213-215`) already shows the correct wrapper pattern. Return-shape mismatch too: handlers must return `{result, error?}`, these return raw records. | Invoking any of the 8 tools (design_extractor, video_use, metadata_gen, marketing_skills, design_inspiration, penpot_sync, social_card, skill_graph_ops) passes `ToolExecutionContext` as the args object → destructured fields (`action`, `nodeId`, …) are all `undefined` → tool errors or no-ops. All 8 are silently broken at runtime. | Wrap each registration like the AgentComm pattern: `registerTool(def, async (context, parameters) => ({ result: await tool.execute(parameters as Args) }))`. Add a smoke test that invokes one tool through `executeTool`. | M (8 tools + test) | Medium — these tools currently never work; fixing *activates* them, so verify each tool's backend before enabling | PRs #34, #36 | |
| A6 | `CreateAgentForm` checks `.ok` on the parsed JSON body | `src/views/agent-view/components/CreateAgentForm.tsx:592-598` — `workspaceResponse = await api.post<…>(…)` then `if (!workspaceResponse.ok)`. But `api.post` (`src/integration/api-client.ts:439-552`) resolves `response.json()` (and throws `AllternitApiError` on non-2xx) — the resolved value is the body, not a `Response`. | If the `workspace/initialize` body has no `ok` field, the warn "Workspace initialization via API failed" fires even on success; if it does have one, the check is redundant. Either way the success/failure signal is wrong. | `api.post` already throws on HTTP error and the surrounding `catch` logs — delete the `.ok` check, or define the endpoint contract as `{ok: boolean}` and check truthiness. | S | Low | PR #37 | |
| A7 | Sessions "Revoke" button broken (Clerk resource mismatch) | `src/views/settings/SettingsView.tsx:359-386` — the sessions table casts each Clerk `SessionResource` to add `latestActivityAt`/`revoke()`, which live on `SessionWithActivitiesResource`, a different object. In-tree comment documents the mismatch. | "Last active" renders `—` forever; clicking **Revoke** throws `sess.revoke is not a function`. | **(a)** switch the auth context to `sessionsWithActivities` so rows are `SessionWithActivitiesResource` (has `latestActivityAt`/`revoke()`); or **(b)** use `SessionResource`'s real surface — `lastActiveAt` + `end()`. (b) is smaller and matches Clerk's session API. Verify which resource the provider actually supplies before choosing. | S | Low–Medium (auth surface) | PR #35 | |
| A8 | `rails.spawn` never existed | `src/allternit-os/programs/WorkflowBuilderProgram.tsx:128-138` — the "spawn terminal for node" button calls `rails.spawn({programId, nodeId})`; `spawn` is on neither `UseAllternitRailsReturn` nor `AllternitRailsClient`. File keeps a `@ts-nocheck` header documenting the finding. | The spawn button silently does nothing (TypeError caught by its own catch and logged). | Product decision: **implement** spawn on the rails client (likely `createTerminalSession` + `createPane`), or **remove** the button. Carried unresolved through slices 2–4. | M/L (implement) / S (remove button) | Depends on decision; removing is low | PRs #31, #33 | |
| A9 | `NativeAgentView` composer/canvas child-API drift | `src/views/NativeAgentView.tsx` (header comment + `@ts-nocheck`) — `SessionComposerRegion` (`:220`) requires full wiring (`serverUrl/value/onSubmit/onStop`; this view is its only consumer in the repo), `MilestoneProgress` (`:234`) needs `AllternitNativeState`, `ToolCallVisualization` (`:235`) needs `ToolCall[]`; no unambiguous data source exists in this view. The message-list crash in the same file was fixed in #32. | The native-agent view's composer and canvas columns render against stale child-component APIs — non-functional or erroring surfaces in that view. | Feature repair, not type work: either wire the three children properly from the session stream (server URL, composer state, native state, tool calls) or replace the columns with the standard chat composer + drop the unused children. | L | Medium–High (only consumer of `SessionComposerRegion`) | PRs #32, #38 | |
| A10 | Todo-item `status` union drift (dynamic-tool branch) | `src/views/cowork/CoworkRightRail.tsx:160-163` — the dynamic-tool branch (TodoWrite/TaskCreate/TaskUpdate) pushes `item.status` raw into `TodoEntry['status']` (`'pending' \| 'in_progress' \| 'completed'`) via cast. The native `task`/`plan` branches in the same function got `mapStatus` normalizers (`:110-113`, `:126-129`); the dynamic-tool branch did not. | A tool emitting `'in-progress'` (hyphenated, common LLM output) produces an out-of-union status; consumers matching `'in_progress'` miss the todo (rail hint, filters, counts). | Reuse the same `mapStatus` normalization in the dynamic-tool branch before pushing. | S | Low | PR #38 | |
| A11 | `board.store.updateItem` spreads wire response verbatim | `src/stores/board.store.ts:138-152` — after the PUT, `data.item` (typed `Partial<BoardItem>` by assertion only) is spread directly into store state: `{ ...i, ...data.item }`. If the API returns the wire shape (e.g. `labels` as a JSON *string*), a string lands in the `labels: string[]` slot. | Board items can corrupt on update — label chips render wrong / `.map` on a string throws downstream. | Normalize the response before spreading: parse `labels` if it arrives as a string, map any snake_case keys, or merge only known `BoardItem` fields. | S | Low | PR #38 | |
| A12 | `agentWorkspaceFS.onChange` doesn't exist (dead cache-invalidation guard) | `src/lib/agents/mode-session-store.ts:373-384` — guards `if (typeof watchableWorkspaceFS.onChange === 'function')`, but `AgentWorkspaceFileSystem` (`src/lib/agents/agent-workspace-files.ts`) exposes no `onChange` method. NOTE comment in tree documents the dead branch. | Context packs are never auto-invalidated when workspace files change on disk — stale context served until an explicit invalidation call happens. | Either implement `onChange` on `AgentWorkspaceFileSystem` (fs watcher wiring) or delete the dead guard + NOTE and document explicit-invalidation-only as the contract. | M (implement) / S (delete guard) | Low | PR #40 | |
| A13 | `AgentWorkspace` missing 4 context fields → metadata writes store `undefined` | `src/lib/agents/agent-workspace-files.ts:70-75` — `AgentWorkspace` declares only `agentId/basePath/files/loadedAt`. In `setSessionMode` (`src/lib/agents/mode-session-store.ts:2205-2247`) the store metadata write reads `workspaceExtras?.{systemPrompt, identityContext, governanceContext, memoryContext}` — all always `undefined`. Subtle second defect: the local `systemPrompt` var built at `:2210` is sent to the API (`:2214-2220`) but *not* used for the store write at `:2233`. | Switching a session to agent mode stores `undefined` for all four context fields in session metadata — identity/governance/memory context missing downstream. | Extend `AgentWorkspace` with the four fields and populate them in `loadWorkspace` (from the CORE/GOVERNANCE/MEMORY file groups already defined in that module); use the local `systemPrompt` var in the store write. | M | Medium — changes what context the model sees | PR #40 | |
| A15 | `Deployment.provider_id` fallback on undeclared field | `src/views/settings/InfrastructureSettings.tsx:1601-1603` — UI prefers `(deployment as { provider_id?: string }).provider_id` over the declared `deployment.provider`; `provider_id` is not on the `Deployment` interface. | If the backend never sends snake_case `provider_id`, the cast is dead and the fallback works; if it does, the type lies. Data-shape confirmation item, not a crash. | Check the deployments API response shape; either add `provider_id` to the `Deployment` type or drop the cast. | S | Low | PR #35 | |
| A16 | Security events may carry top-level `timestamp` | `src/views/settings/SecurityPanel.tsx:31-33,209` — code already honors both `event.timestamp \|\| event.createdAt` via a local `SecurityEventRow` extension. | None visible (both fields honored). The declared `SecurityEvent` type may not match the wire. | Confirm the events API shape; then add `timestamp` to `SecurityEvent` or drop the extension. | S | Low | PR #35 | |
| A17 | `CodeBlock` language asserted to shiki's `BundledLanguage` | `src/components/ai-elements/UnifiedMessageRenderer.tsx:752` — `part.language as BundledLanguage` (a runtime `string`) is passed to `CodeBlock`, which feeds it to shiki's `createHighlighter({langs: [language]})`. | An unrecognized language string (model-emitted, e.g. `typescript2` or a locale name) may make shiki throw → message-list render crash. | Validate against shiki's bundled-language set (or a try/catch around the highlighter) with a fallback to `text`. | S | Low | PR #36 | |
| A18 | `AgentRunner` local `TraceEntry` requires `status`; wire entry's is optional | `src/runner/AgentRunner.tsx:87-93` — the local `TraceEntry` interface requires `status: 'running' \| 'success' \| 'error'`, while `RunnerTraceEntry.status` is optional; the call site (`:592`) bridges with `entry as unknown as TraceEntry`. | Trace entries without a status render through `getStatusIndicator(undefined)` — wrong/blank status chip. | Make the local `status` optional and handle `undefined` in `getStatusIndicator`; remove the double cast. | S | Low | PR #37 | |
| A19 | `CreateAgentForm` temperament boundary wider than target type | `src/views/agent-view/components/CreateAgentForm.tsx:169-171` — `CreationTemperament` has 6 values, `CharacterLayerConfig['identity']['temperament']` has 4; asserted `as unknown as` at the boundary. `CharacterStep`'s Select only emits the 4 valid values today. | None today (unreachable). Hazard if `CreationTemperament` gains an emitter. | Narrow `CreationTemperament` to the 4 valid values (or map at the boundary) and drop the double assertion. | S | Low | PR #37 | |
| A20 | `rust-stream-adapter` reads `part.result` (not on the AI SDK type) | `src/lib/ai/rust-stream-adapter.ts:85` (+ `rust-stream-adapter-extended.ts:670`) — reads `(part as { result?: unknown }).result ?? part.output`; `DynamicToolUIPart` has only `output`. Dead-defensive unless the wire ever populates `result`. | None visible today. | Confirm the backend wire shape with the platform team; then keep the defensive read (documented) or drop it. | S | Low | PR #38 | |

## Already fixed — DONE (do not re-decide; evidence of the fixing PR)

These were flagged the same day and fixed by a later merged PR. Listed so "fix all" doesn't
re-open them.

| Finding | Status | Fixing PR |
|---------|--------|-----------|
| Theme `log('success', …)` silently swallowed (8 call sites; `LOG_LEVELS['success']` undefined) | **FIXED** — `'success'` level added at weight 1 + regression test | platform [#602](https://github.com/Allternit/allternit-platform/pull/602) |
| `question.ts` called static `Bus.publish` on the Bus *class* (TypeError on every question event) | **FIXED** — imports the `Bus` namespace from `@/shared/bus` | platform [#602](https://github.com/Allternit/allternit-platform/pull/602) |
| `TestingPermissionTool.isEnabled()` compared `"production" === 'test'` (always false) | **FIXED** — restored to `process.env.NODE_ENV === 'test'` | platform [#616](https://github.com/Allternit/allternit-platform/pull/616) |
| VerifyPlan gate mismatch: `classifierDecision.ts` (both copies) gated on `USER_TYPE === 'ant'` while `tools.ts` used `GIZZI_CODE_VERIFY_PLAN` | **FIXED** — both copies aligned to the env flag | platform [#613](https://github.com/Allternit/allternit-platform/pull/613) |
| `highlightJs.getLanguage` always `undefined` (ESM default-export interop) | **FIXED** — read off the default export | platform [#620](https://github.com/Allternit/allternit-platform/pull/620) |
| `question-tool.tsx` referenced 5 never-defined values → `ReferenceError` on render | **FIXED** — definitions added per sibling conventions | allternit-ai [#30](https://github.com/Allternit/allternit-ai/pull/30) |
| `WorkflowBuilderProgram` messages tab crash (`msg.role.toUpperCase()`); `dak.store` `leaseInfo` snake/camel mismatch | **FIXED** — render `kind`/`payload`; explicit lease mapping | allternit-ai [#31](https://github.com/Allternit/allternit-ai/pull/31) |
| Video providers never showed configured (`isAvailable === true` on a function); SessionsPage trace inspector `authHeaders` ReferenceError; `NativeAgentView` message-list crash (wrong `parts` contract); design in-place edit feedback shape; `logger.error` arg order; "Current" session chip always false | **FIXED** — all six in the owner-approved fix lane | allternit-ai [#32](https://github.com/Allternit/allternit-ai/pull/32) |
| `OAuthSelectAccountPage` undefined `router.push`; missing class exports; `ChatComposer` handoff passed agent object instead of id; `allowpopups="true"` string prop | **FIXED** — all four | allternit-ai [#33](https://github.com/Allternit/allternit-ai/pull/33) |
| `DesignPropertiesPanel.update()` passed `type: undefined` for missing shapes | **GUARDED** — early-return on missing shape landed (the lie can no longer reach `updateShapes`); root behavior unchanged | allternit-ai [#40](https://github.com/Allternit/allternit-ai/pull/40) |
| `ControlTab` dual queue-item wire shape (`item.task` no longer exists); `ReviewTab`/`ControlTab` dead views | **FIXED** — both views deleted (zero consumers) | allternit-ai [#41](https://github.com/Allternit/allternit-ai/pull/41) |

## Appendix — how to execute a FIX

General ritual per `AGENTS.md` (worktree from fresh `origin/main`, no installs beyond pnpm,
never touch `queue.json` unless a burn batch records it, conventional commits, PR +
`--merge` merge commit, shared-checkout `pull --ff-only`, ledger attestation, worktree/branch
teardown, `scripts/git-discipline-check.sh` PASS as final evidence). Row-specific notes:

**Platform rows (P1–P5) — gates, run from `cmd/gizzi-code` unless noted:**
1. `bash script/ensure-sdk-dist.sh` (rebuild os-contracts/plugin dists).
2. `NODE_OPTIONS=--max-old-space-size=8192 npx tsc --noEmit` → exit 0.
3. `bun run test` smoke → expect ~1311 pass / 0 fail (107 files), including the
   `ts-nocheck-guard` and `dead-code-guard` tests. P1 (cut) and P3/P4/P5 edits remove code —
   if any touched file carried a `// @ts-nocheck` header, the burn-queue guard requires the
   removal to be recorded in `script/typecheck-burndown/queue.json` in the same PR (burn
   bookkeeping only — never edit batch state by hand).
4. `node scripts/release-preflight.mjs` (repo root) → 52/0. P1 touches the runtime the
   desktop bundles → step 8 desktop rebuild applies; P2–P5 are type-level/small and do not.
5. `npx eslint` on all changed files → no new problems.
- Targeted tests: P2 gets a type-level contract test (hook payload shape) or extends the
  plugin SDK's existing tests; P1 cut adds/extends a dead-code-guard manifest entry;
  P4/P5 are covered by tsc + existing smoke.

**allternit-ai rows (A5–A20) — gates, repo root:**
1. `pnpm run typecheck` → the sorted set of the 24 pre-existing error lines must stay
   identical (diff against main's baseline; several rows exist *because* of this rule).
2. `pnpm run build` (vite) → exit 0.
3. `pnpm run lint` → exit 0; `pnpm run lint:ratchet` → if any `no-explicit-any` /
   `exhaustive-deps` / `no-unused-vars` count changes, regenerate and commit the baseline in
   the same PR (the ratchet only ratchets down — fixing a cast that *removes* an `as any`
   must be reflected).
4. `pnpm test` → zero new failures vs an `origin/main` baseline run in the same tree (the
   repo has a known bank of env-dependent live-API failures; compare lists, don't chase).
- Targeted tests: A5 gets an `executeTool` smoke per fixed tool (the pattern already exists
  for AgentCommunication); A6/A7/A10/A11 get store/component unit tests (vitest); A12/A13
  extend `mode-session-store.local-fallback.test.ts` (7/7 today); A17 gets a render test
  with an unrecognized language.
- A5 and A8 are the two rows that change *which features are live* — get explicit owner
  sign-off on the enabled behavior, not just the code, before merging.
