# Subscription Capability Fabric — Hardening Amendments

**Status:** Architecture lock, amended — supersedes SPEC.md where they conflict
**Date:** 2026-09-25
**Inputs:** `SPEC.md` (original), `REVIEW_CLAUDE.md` (architecture review, accepted in full), repo grounding pass against HEAD `b1bf20057`

The §44 invariant stands unchanged:

> Bots request capabilities. The fabric chooses entitlements. Adapters execute them. Artifacts come back normalized.

Everything below is an amendment to SPEC.md. Section numbers refer to SPEC.md.

---

## D. Decisions locked (were open questions)

| # | Question | Decision |
|---|---|---|
| D1 | Gateway language | **TypeScript / Node ≥ 20**, pnpm workspace package. Playwright is the core competency and the repo's browser harness (`@allternit/browser-tools`, `platform/packages/browser-tools`) is TS. Rust only if routes are later folded into `allternit-api`. No Python. |
| D2 | Account scope | MVP is **owner-accounts-only**. Client-account connection (the `chatgpt-image` skill's consent path) is out of scope; if ever added, it gets a separate profile store and policy namespace per client. |
| D3 | Local-only guarantee | **Structural:** the gateway refuses to start without access to the local macOS Keychain (token + at-rest key live there). It must never run on Allternit cloud infrastructure — user sessions are never hosted server-side. |
| D4 | Window posture | One dedicated **"Allternit Sessions" Chrome window**, one tab per connected provider, real Chrome channel (`channel: "chrome"`), headful, minimized/off-screen allowed. Headless is opt-in per adapter only after probe history proves stability. No fingerprint spoofing, no stealth plugins — those are both the detection signal and a SPEC §26 violation. |
| D5 | Provider history hygiene | Bot conversations go in a dedicated provider project/folder named **"Allternit"** where the provider supports projects. For ChatGPT, per-account user choice: stateless bot tasks in temporary-chat mode, or accept memory learning from bot prompts. Default: temporary chat for stateless tasks. |
| D6 | `chatgpt-image` lane | **Migrate onto the fabric.** media-router's ChatGPT free lane calls `image.generate` through the gateway; the standalone `~/.chatgpt-image-profile` lane is retired once the fabric lane is verified. One automation identity per account — two profiles on one account fight over rotating session tokens. |
| D7 | Content refusals | **Always surface the refusal.** Never auto-shop a refused prompt to another provider (`content_refused`: not fallback-eligible). |
| D8 | Thread migration on exhaustion | **Ask for interactive threads; wait for reset on background threads.** Cross-provider thread migration is opt-in (`allow_thread_migration`) and requires an explicit context-transfer strategy (`summary \| full_replay \| none`). |
| D9 | Publish gate | `website.publish` and any `external_publish` capability **always** requires per-task human approval (`approval_id` bound to the specific task + preview). No policy override. Matches the repo's deploy-preview and invoice-approval gates. |
| D10 | Default pacing budgets | Per provider per day until real signals are observed: **ChatGPT 150, Claude 80, Kimi 60**. `min_task_gap_s: 5`, `min_action_gap_ms: [800, 2500]`, no parallel submits on one account ever (parallelism only for read-only detached watches). User-overridable. |

---

## A. Critical amendments (from REVIEW_CLAUDE.md, all accepted)

1. **Streaming adapter contract (replaces §29).** `execute()` returns `AsyncIterable<AdapterEvent>` and receives an `ExecutionContext` carrying `AbortSignal`, a worker-owned page lease, an artifact sink, a pacer, and `markSubmitted()`. Cancellation = `signal.abort()` → adapter clicks the provider Stop button and reports partial output. Normative TypeScript: REVIEW_CLAUDE.md §A1.
2. **At-most-once submission (§19/§20/§31 gap).** Every attempt carries `submission_state: not_sent | sent_unconfirmed | acknowledged` and a `prompt_fingerprint`. `markSubmitted()` is durably written BEFORE Send is clicked and again after provider acknowledgment. On crash recovery, `sent_unconfirmed` attempts go through `reconcile()` (open mapped thread, match last user turn by fingerprint, adopt or mark `submission_ambiguous`). **Blind resubmit is forbidden.** Tasks accept a caller `idempotency_key`.
3. **Detached execution for long-running capabilities (§17/§19).** Worker capacity splits into: 1 interactive slot (typing/submitting/extracting) + N (default 2) read-only detached watches. Adapters declare `detachable` per capability; after acknowledgment they emit `detached { resume_token, poll_after_s }` and free the interactive slot. New task status `provider_running`. Deep research and Kimi slides/websites run 5–30 min provider-side; without this, one task freezes a provider for 20+ minutes.
4. **Pool-based quota with downgrade + cooldown semantics (replaces §11/§33).** `QuotaPool` keyed `(provider, account_id, pool_id)`; capabilities reference `pool_id`. Record `observed_model` per attempt; mismatch vs requested class → `quota.degraded` signal, pool → `degraded`. State machine and router behavior under uncertainty: REVIEW_CLAUDE.md §A4 (unknown pools stay eligible above metered lanes; local soft budgets; exponential cooldown 30m→24h; no active probing of exhausted pools; circuit breaker trips adapter to `ui_drift` after 3 consecutive selector failures).
5. **Session health state machine + non-spending canary (§29/§34 gap).** `SessionHealth: ready | degraded | auth_required | challenge_presented | account_restricted | ui_drift | provider_down | profile_locked`. `adapter.probe()` checks every `critical` locator WITHOUT submitting anything — runs on worker start, after any selector failure, and on a 15–30 min idle timer. Challenge/restriction → halt the whole worker, pause its queue, notify via desktop shell, never auto-retry.
6. **Profile ownership (§25/§34 conflict).** The gateway is the **sole owner** of each provider profile. "Open manually" (§25) means the gateway brings its own window forward, or releases the lock and hands the window to the user, then reattaches. Detect `SingletonLock` → `profile_locked`. See D6.
7. **Thread mapping is 1:N with epochs and divergence detection (replaces §15).** `ThreadMapping` gains `epoch`, `last_synced_turn_index`, `last_turn_fingerprint`, `on_divergence: adopt | fork | fail`, `context_transfer_from`. Adapters check divergence before continuing a thread. Schema: REVIEW_CLAUDE.md §S6.
8. **Localhost threat model (§25 gap).** Default transport is a **Unix domain socket** (`~/.allternit/subscriptions/gateway.sock`, mode 0600). TCP `127.0.0.1:7788` is optional and ALWAYS requires a bearer token (per-caller scoped tokens; bots never get `accounts:manage` or `approve:*`). Strict `Host` validation (DNS-rebinding defense), reject `Origin` unless allowlisted (browser-CSRF defense), never CORS `*`. `inputs[].path` must realpath under an allowlisted root — it is an arbitrary-file-read + data-egress primitive otherwise. Downloaded artifacts are content-addressed, quarantined, and rendered only sandboxed. Provider output is `trust: untrusted_provider_output` — prompt-injection path into tool-wielding bots. Full list: REVIEW_CLAUDE.md §A6.
9. **Side-effect classification (§9/§32 gap).** Every capability declares `side_effects: none | provider_state | external_publish`. See D9.
10. **Placement + naming (replaces §28, resolves §18).** Implementation lives in this monorepo per `REPO_STRUCTURE.md` ownership rules — not a standalone root. Layout in IMPLEMENTATION_PLAN.md. "Gateway" is overloaded in this repo (8013, `services/gateway/*`, LLM gateway): always say **Subscription Gateway**. Chat streaming reuses `@allternit/replies-contract` `ReplyEvent` so Thread UIs reuse `@allternit/replies-reducer` instead of a third event dialect.

---

## S. Schema amendments (summary)

Normative TypeScript for all of the below is in REVIEW_CLAUDE.md §S1–S7 and gets transcribed into the contracts package in plan phase P0:

- **Capabilities (§9):** flat dot-named `CapabilityId`; remove artifact-operations (`*.export`, `image.download` → artifact API) and API verbs (`task.*`, `*.list` → gateway endpoints). `reasoning` is a modifier: `options.model_class: fast | standard | reasoning | deep`. Split `research.run` → `research.quick` + `research.deep`. Add `code.generate` or drop `/v1/code`; add `html_app` artifact type (§14 already uses it).
- **Manifest (§10):** flat typed `AdapterManifest` with `origins` allowlist, `auth` probes, per-capability `pool_id`/`detachable`/`export_formats`/`status`, `pacing`, `selectors_version`. Reliability is **measured** (rolling success rate per adapter_version+capability from the event log), never hand-scored.
- **Entitlement (§11):** a **computed join** of Account × Manifest × QuotaPool × SessionHealth — not a stored record (stored records drift). Unknown values stay unknown.
- **Task (§20):** gains `idempotency_key`, `requester`, `routing` block (`mode`, `allow_fallback`, `allow_metered` default false, `allow_thread_migration` default false), `constraints` block (`sensitivity`, `max_metered_usd`), `approval_id`, `priority`, `attempts[]`. Status set revised: add `provider_running`, `needs_user`, `partial`; remove `artifact_ready` (it's an event, and several can fire per task).
- **Artifact (§13):** gains `sha256`, `size_bytes`, `bot_id`, `retrieval_state`, `version`/`parent_artifact_id` edit chains, `provider_url_expires_at`; `preview_url` splits into provider URL (needs session) vs local sandboxed preview.
- **Errors (§31):** `TaskError` gains `scope` (what the failure poisons: task/pool/account/adapter/gateway), `fallback_eligible`, `user_action`, `evidence_ref`. New classes: `challenge_presented`, `account_restricted`, `submission_ambiguous`, `model_downgraded`, `content_refused`, `stalled`, `output_truncated`, `profile_locked`, `worker_crashed`, `artifact_expired`, `approval_required`, `policy_denied`. Fold `selector_not_found` into `provider_ui_changed` with `detail.locator_key`.

---

## X. Adapter extensibility (the "add Gemini cheaply" requirement)

Adding a provider must NOT mean re-implementing launching, auth detection, typing, completion detection, downloads, retries, and quota parsing. Design (full text: REVIEW_CLAUDE.md "Adapter extensibility design"):

1. **Adapter SDK** (`@allternit/subscription-adapter-sdk`) provides shared primitives: `fillComposer` (contenteditable + textarea, paste/insertText for long prompts, never per-key typing), `submit`, `awaitCompletion` (multi-signal: stop button gone + send re-enabled + text stable 1.5–3s + no streaming node), `extractLastAssistantTurn` (DOM→markdown preserving code/citations), `captureDownload`, `captureImages`, `detectAuthState`, `detectBanners` (regex packs → `QuotaSignal`), `threadIdFromUrl`.
2. **Selector registry** — `adapters/<p>/selectors/<version>.yaml`, named locator keys with ordered fallback strategies (semantic first: role/accessible-name, `data-testid`; CSS/XPath last). The resolver records which strategy matched; fall-through to a later strategy is the early drift warning. `probe()` checks every `critical: true` key.
3. **Declarative chat adapters** — chat-only providers (Gemini, Grok, Mistral, DeepSeek) are pure config: `manifest.yaml` + `selectors.yaml` + thread-URL regex, run by a generic `DeclarativeChatAdapter`. New chat-only provider = config + fixtures PR, zero TypeScript. Only artifact-producing capabilities need code hooks. **Gemini is the designated proof-of-extensibility adapter** (plan phase P6).
4. **Capability probes** — `probe()` also confirms each declared capability's entry points still exist (image toggle, Slides mode button) without spending; vanished entry point → capability auto-marked `disabled` in the live registry.
5. **Fixtures + conformance suite** — sanitized DOM snapshots per state (logged-out, idle, streaming, complete, limit-banner, challenge); every adapter must pass the shared conformance suite to merge. Nightly optional live canary (probe-only).
6. **Manifest-driven routing** — the router never branches on provider names; enforced by a lint rule banning provider-name literals in `router/` and `queue/` (§42 separation, now in CI).
7. **Versioned hot-swap** — `adapter_id@version` loading; v2 ships next to v1 with canary traffic; cut over on measured success rates. §5 disposability delivered.

---

## C. MVP cuts (tighter than §27/§40)

Cut or defer: per-capability REST endpoints in §8 (keep `/v1/tasks`, `/v1/tasks/{id}/events`, `/v1/artifacts/{id}`, `/v1/accounts`, `/v1/capabilities`); `project.*`, `website.publish/modify`, `*.edit`, `spreadsheet.analyze`, `file.*` as capabilities; desktop-app bridges for v1 (a committed class in P7 per D14 — cut from v1 only because they depend on the cloud-desktop containment); avoided-cost economics UI (record `est_metered_usd` per route candidate now — it's cheap — but defer §24/§27-Phase-6 surfaces); router scoring weights (ordered preference + policy until measured reliability exists); multi-account per provider, raised concurrency, headless mode (schema-ready, not built); metered-API fallback execution (router may *report* a metered candidate; running it needs `allow_metered` + per-task approval; wiring to `llm_gateway` deferred); §43 non-AI SaaS generalization.

Kimi phase is reduced to **one artifact family** (`presentation.create` → pptx) held until pptx capture is stable for two weeks, per REVIEW_CLAUDE.md MVP cuts. Remaining Kimi artifact families (document/spreadsheet/website) follow one at a time behind the same stability gate.


---

## P. Progress streaming and completion push (2026-09-25 amendment — closes the "silent gap")

The native provider UIs show work happening during long provider-side tasks (Kimi slide generation steps, ChatGPT deep-research source counts, "thinking" traces). A fabric that shows nothing for 20 minutes reads as broken, can't be debugged, and — worse — if completion is only discovered when the caller happens to ask, the system is neither autonomous nor deterministic. Both are fixed as **v1 requirements**, not later enhancements.

### D11. Progress streaming during `provider_running` is mandatory

- Detached watch pages are not blind pollers. While a task is in `provider_running`, the watch adapter scrapes the provider's **native progress surface** and emits normalized `AdapterEvent`s:
  - `{ t: "progress", label, fraction? }` — mapped from provider-native signals (step lists, "researching N sources", slide thumbnails appearing, partial text growth). Each adapter declares its progress extractors per capability in `selectors/<v>.yaml` under a `progress:` key group; the SDK ships generic extractors (step-list items, counter badges, streaming-text growth) that declarative adapters get for free.
  - `{ t: "artifact.partial", ref, preview? }` — partial artifacts the provider exposes mid-run (e.g. first slides of a deck, streamed image tiles) so the UI can show something real before completion.
  - `{ t: "progress.heartbeat", elapsed_s, last_change_at }` — emitted every 15 s even when nothing changed. **The UI must never render silence**: a heartbeat with a growing `elapsed_s` is the difference between "working" and "hung," and it's the watchdog's input too.
- All of it lands in the durable event log, so debugging a stuck task is `GET /v1/tasks/{id}/events` — you see the last thing the provider actually did, not a spinner.
- The stall watchdog consumes `last_change_at`: no DOM/event change beyond `stall_timeout_s` (per capability: 90 s chat, 20 m deep research) → `stalled`, classified per the A9 taxonomy. "Provider quiet but heartbeating with recent change" ≠ "stuck."

### D12. Completion is detected and pushed by the gateway — never pulled by the caller

The model/bot/user must never have to ask "is it done yet?" for the system to finish.

- **Deterministic detection:** the worker (interactive or detached watch) owns completion detection via the multi-signal `awaitCompletion` (stop button gone + send re-enabled + content stable + no streaming node). On completion it immediately captures artifacts into the store and transitions the task — autonomously, with no caller round-trip.
- **Push while connected:** SSE subscribers of `/v1/tasks/{id}/events` (or a per-thread subscription) receive `artifact.ready` / `done` the moment they fire.
- **Durable per-caller inbox:** every event is also written to a caller-addressed outbox table (`caller_outbox`). A bot that was disconnected receives everything on reconnect without re-asking; delivery is acknowledged per event (at-least-once, idempotent on `event_id`).
- **Proactive notification:** on terminal states (`completed`, `partial`, `needs_user`, `failed`) the gateway notifies the requester through its channel of record — CommRails message to the bot peer (`POST /api/rails/peers/:name/send`), desktop-shell notification for user-initiated tasks, MCP resource update for MCP clients. `needs_user` is treated as a first-class push (auth expired, challenge, approval required) — surfaced immediately, never discovered by timeout.
- **Contract for bots:** a bot that submits a task and continues other work gets the result delivered into its thread as a system/result message; it never polls. Polling endpoints exist for debugging only.

### D13. Model-selector naming: subscriptions appear as named models

When gizzi-code or an Allternit UI shows a model picker, each connected subscription appears as first-class entries under a **"Subscriptions"** group, distinct from metered API models:

```text
subs/<provider>:<model_class>
```

| Selector entry | Routes to |
|---|---|
| `subs/chatgpt:auto` | Capability Router picks model class per task (default) |
| `subs/chatgpt:standard` | chat.message, `options.model_class: standard` |
| `subs/chatgpt:reasoning` | chat.message, `model_class: reasoning` |
| `subs/chatgpt:deep` | research.deep (deep research surface) |
| `subs/claude:standard` / `subs/claude:reasoning` | chat.message on the Claude subscription |
| `subs/kimi:standard` / `subs/kimi:deep` | chat.message / research.deep on the Kimi subscription |

Display names carry provenance, e.g. "ChatGPT (subscription) · Reasoning". Rules:

- `subs/*` entries appear in the picker only when the account is connected and `session_health` is `ready|degraded`; degraded shows a badge, `needs_user` shows the required action.
- Selecting a `subs/*` model is sugar for a fabric task (`chat.message` with `model_class`); the conversation is a normal fabric thread with full progress/artifact streaming. The picker never exposes providers directly for artifact capabilities — those stay capability calls (`image.generate`, `presentation.create`), surfaced as **tools**, not models.
- The concrete provider model behind each `model_class` is adapter-manifest config (`plans`/`model_class` mapping per plan tier), never hard-coded in the picker; `observed_model` telemetry (A4) still verifies what actually answered.
- In gizzi-code these entries are registered through the same provider-registry path as existing CLI-subscription entries (`provider_routes.rs` already lists claude/codex/kimi CLI OAuth) — the fabric extends that UX rather than colliding with it.

### D14. Two adapter classes: web first, desktop as the geo-block-resilient lane

Revised 2026-09-26 (was "web only, desktop deferred indefinitely"). Desktop-app adapters are a **committed adapter class**, not a someday item — because a desktop app has no web URL to geo-block. It talks to the provider's service endpoints directly, so browser-surface restrictions, region-blocked web domains, and web-only anti-automation measures don't apply. If a provider's web app becomes unreachable or degraded from the user's region/network, the desktop lane keeps the entitlement usable.

- **Web remains the default per capability** (SPEC §7 rationale stands: inspectable DOM, deterministic selectors, easy downloads/thread URLs). **Desktop is the second execution path for the same capabilities**, implemented as ordinary adapters behind the same `SubscriptionAdapter` contract — the manifest schema already carries `interface: "ui_bridge_desktop"` (S2), so contracts don't change.
- **Router semantics:** a provider can have two live adapters (e.g. `chatgpt-web`, `chatgpt-desktop`) covering the same entitlement. Web ranks first under normal conditions; the desktop adapter is the automatic fallback when the web lane fails with `network_error` / geo-block / repeated `provider_down` / challenge loops — and the user can pin either in policy. Same account, same pools, same pacing budgets: the two lanes share one `QuotaPool` set per account, never double-spend.
- **Desktop bridge implementation (researched — see `DESKTOP_BRIDGE_RESEARCH.md`, 2026-09-26):** raw vision automation is NOT the plan. The desktop apps are automatable at the DOM level:
  - **Guest image OS: Windows.** On Windows, all three target apps are DOM-automatable: Claude Desktop (Electron) and ChatGPT Desktop (Electron on Windows; the Mac app is native Swift — no CDP there) via Playwright `connectOverCDP` on `--remote-debugging-port`, and Kimi desktop (Tauri) via WebView2's documented `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port`. CDP attach to Claude Desktop is confirmed in the wild (Jan 2026, MutationObserver-based response extraction, multi-instance). CDP attach to ChatGPT-Windows and Kimi-Windows is mechanism-sound but unproven in public — **verified on first deployment** as a P7 gate. macOS guests are the degraded path (ChatGPT = Swift/AppKit, Kimi = WKWebView → accessibility-tree only) and are not the recommended image.
  - **Fallback ladder per app:** ① CDP/DOM (reuses the web adapter's selector packs where the app renders the same web UI) → ② Windows UI Automation (FlaUI / pywinauto — both maintained; WinAppDriver is dead) → ③ self-hostable vision agents (Agent S2 or UI-TARS-desktop, both Apache-2.0, SDK-shaped). OmniParser is excluded (AGPL-tainted weights). The RPA generation (TagUI/SikuliX/UI.Vision/OpenRPA) is excluded — unmaintained or wrong shape.
  - **Update watchdog:** a provider app update could flip Electron fuses and kill `--remote-debugging-port`; the probe canary (A5/Critical #5) detects this and the image's per-provider update policy pins app versions, updating only after conformance passes.
  - The desktop bridge satisfies the same contract as web: streaming `AdapterEvent`s, progress scraping (D11 — DOM when attached via CDP, UIA/OCR only in fallback), multi-signal completion detection, at-most-once `markSubmitted`, non-spending probe.
  - **Legal flag (noted, not relitigated — owner decision stands):** Anthropic/OpenAI consumer terms restrict automated access to consumer surfaces; flagged in the research for legal review before this lane is ever offered commercially to third parties. Owner-account use continues per the accepted-risk decision.
- **Honest tradeoff, accepted:** desktop automation is brittler (focus/window-state dependence, framework drift on app updates, harder text extraction in fallback modes). Containment on the pinned-image Sessions machine (D15) removes the biggest brittleness factors — fixed OS, pinned app versions, no user focus stealing — which is why desktop adapters run **only** there, never on the user's daily-driver desktop.
- No provider desktop app is required for v1 web capabilities; the desktop class lands in plan phase P7 with the cloud-desktop containment it depends on.

### D15. The containment machine: one managed image, three provisioning tiers

D3 is refined: the boundary is **single-tenant user ownership + Allternit-provisioned containment**, not physical locality. The fabric runtime (gateway, browser profiles, provider desktop apps) NEVER runs uncontained in the user's interactive desktop session. It always runs on an **"Allternit Sessions" machine** — a dedicated, disposable computer provisioned from one managed image (Windows guest — required for the desktop lane per D14 research; a macOS image remains possible for web-lane-only deployments — with gateway + Chrome + provider desktop apps pre-installed, per-provider pinned update policy, secret store, node-agent telemetry). What varies by plan is WHO provides the hardware:

| Tier | Who provides the machine | How it's provisioned | What the user sees |
|---|---|---|---|
| **T1 Hosted** (Allternit cloud subscription) | Allternit cloud — carved from the VPS/resources already allotted in the user's subscription tier (CPU/RAM/disk/bandwidth per plan) | One click. The Sessions machine is simply a workload on their existing cloud allotment, spun up from the managed image | Settings → **Sessions Computer** panel in the desktop app: status, CPU/RAM/disk/bandwidth usage against plan allotment, uptime, connected provider accounts, lane health (web/desktop per provider) |
| **T2 BYOC** (user's own cloud/hardware) | A VPS the user owns (any provider) or an always-on machine they own | The existing node agent (`cmd/allternit-node` VPS edge agent / cloud-wizard flow) enrolls the machine; Allternit deploys the same managed image onto it | Same Settings panel, same telemetry — fed by the node agent instead of Allternit cloud metering; panel labels it "Your machine" and shows no Allternit-side resource billing |
| **T3 Local-contained** (no cloud sub, own hardware) | The user's own machine — but NEVER the daily-driver desktop session | Allternit provisions a local contained environment: a lightweight VM via the repo's existing drivers (`drivers/apple-vf`, `drivers/firecracker`) — falling back to the `bot_desktop_sandboxes` pattern (V91) where a VM isn't viable. Boots on demand when tasks queue, sleeps when idle | Same Settings panel showing the local sandbox/VM, its resource caps, and a clear "runs only while working" posture |

Unifying rules across all three tiers:

- **Same image, same gateway binary, same security posture** (UDS + scoped tokens, keychain-equivalent secret store on that machine, D3/D15 ownership rules). Tier changes are a re-provision, not a re-architecture; accounts and state migrate via the encrypted state bundle.
- **Single-tenant always.** No shared/multi-tenant hosting of subscription sessions, ever. Session material never transits infrastructure the user doesn't own unencrypted (T1 encrypts at rest and in the tunnel; the allotment is theirs alone).
- **The Settings → Sessions Computer panel exists in every tier** and is the single place to see the machine, its resources, its connected accounts, and its needs-user queue — this is also where T1 shows plan-allotment consumption, satisfying the "user can see their cloud computer usage" requirement.
- **Auth UX is identical in every tier:** the machine's display is streamed (computer-embed / remote surface) for the one-time interactive provider logins, then it runs unattended.
- Desktop-app adapters (D14) run only on T1/T2/T3 machines with a pinned image — which in practice means all of them, since the fabric never runs uncontained anyway.

## M. Surface-mode unification (how the fabric appears in each mode)

One fabric, four consumption surfaces. The rule from SPEC §16 holds everywhere: **bots and surfaces request capabilities; only the router knows providers.**

| Surface | How the fabric shows up | Completion path |
|---|---|---|
| **Chat mode** (ai.allternit.com / desktop chat) | `subs/*` entries in the model picker (D13); capability tools (`image.generate`, `research.deep`, `presentation.create`) offered in the composer tool menu when any connected subscription supports them | Same-thread streaming via SSE; `provider_running` shows the progress feed (D11); artifacts land on the thread's artifact shelf; `needs_user` renders inline with the required action |
| **Cowork mode** | Cowork tasks that need a capability submit fabric tasks with `requester.kind: "bot"`; approvals (`external_publish`, metered) surface as cowork approval cards | Push to the cowork thread via caller outbox + CommRails; the coordinator bot is woken with the result, never polls |
| **Code mode** (gizzi-code) | MCP server tools `subscription_task_run` / `subscription_task_status` / `artifact_get`; `subs/*` models selectable as the chat model | MCP resource update + CommRails peer message lands the artifact reference in the session; long tasks detach and report back without blocking the coding loop |
| **Bot mode** | Bots call the fabric client with capability + constraints, never provider names; per-bot scoped tokens (`tasks:submit`, `artifacts:read` only) | Durable caller outbox per bot + CommRails push; `needs_user` is routed to the bot's owning thread so a human unblocks it |

Unified invariants across all four: same task schema, same event stream, same artifact store, same approval gates, same observability. A task started in chat mode and a task started by a bot are indistinguishable in the ledger.

Cross-cutting fifth surface: **Settings → Sessions Computer** (D15) — the desktop-app settings modal panel that shows the containment machine in every tier: status, resource usage (against the cloud-plan allotment in T1; node-agent telemetry in T2; local caps in T3), connected provider accounts with per-lane health, and the needs-user queue. This is also where provisioning/re-provisioning between tiers is driven.
