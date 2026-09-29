# Subscription Capability Fabric — Implementation Plan

**Date:** 2026-09-25
**Reads with:** `SPEC.md` (original architecture), `HARDENING.md` (binding amendments), `REVIEW_CLAUDE.md` (normative schemas/interfaces)
**Repo process:** every phase runs in its own `session/*` worktree per repo AGENTS.md; pnpm only; verification commands below are the merge gate per phase.

---

## 0. Stack and placement (decided)

- **Language:** TypeScript / Node ≥ 20, pnpm workspace. Gateway + workers + adapters + contracts all TS.
- **Home:** daemon in `services/subscription-gateway/`; shared types and SDK in `platform/packages/`; CLI subcommands in the existing `cmd/allternit` CLI.
- **State:** SQLite (WAL) at `~/.allternit/subscriptions/state.db` via `better-sqlite3@13.0.3` (already pinned workspace-wide). No Postgres for the local daemon.
- **Artifacts:** content-addressed store at `~/.allternit/subscriptions/artifacts/<sha256[0:2]>/<sha256>`, quarantined (see HARDENING A8/§A6).
- **Transport:** UDS `~/.allternit/subscriptions/gateway.sock` (0600) default; optional TCP `127.0.0.1:7788` always token-authed. 7788 verified collision-free by grep; add it to the port table in `docs/Operations/QUICK_REFERENCE.md` in P1.
- **Browser:** Playwright via `@allternit/browser-tools`, real Chrome channel, persistent profiles under `~/.allternit/subscriptions/<provider>/profile`, gateway sole owner (HARDENING A6/D4).
- **Streaming vocabulary:** `@allternit/replies-contract` `ReplyEvent`; Thread UIs consume via `@allternit/replies-reducer`.
- **Key boundary:** the fabric's Capability Router sits ABOVE the existing LLM gateway (`cmd/allternit-api/src/llm_gateway/provider_routing.rs`), which routes metered API traffic only. The existing gateway is the fabric's future metered-fallback path — do not duplicate it.
- **Naming:** always "Subscription Gateway" — `gateway` alone is overloaded (8013, `services/gateway/*`, LLM gateway).

## 1. File layout (target state)

```text
platform/packages/subscription-fabric-contracts/     # @allternit/subscription-fabric-contracts
  package.json  tsconfig.json
  src/
    capability.ts        # CapabilityId, CapabilityDef, ArtifactType, model_class
    manifest.ts          # AdapterManifest, PlanDef, PacingProfile
    account.ts           # Account, SessionHealth
    quota.ts             # QuotaPool, QuotaSignal
    task.ts              # Task, TaskAttempt, TaskInput, TaskStatus, TaskError, FailureClass
    artifact.ts          # Artifact, ArtifactFile, retrieval_state
    thread.ts            # ThreadMapping (1:N, epoch, divergence)
    routing.ts           # RouteDecision, RouteCandidate, FabricSnapshot
    events.ts            # AdapterEvent union, ProbeResult, ResumeToken
    index.ts
  test/                  # zod round-trips, schema-version guards

platform/packages/subscription-adapter-sdk/          # @allternit/subscription-adapter-sdk
  src/
    runtime.ts           # ExecutionContext, PageLease, ArtifactSink impl, RedactingLogger
    selectors.ts         # named-locator registry + ordered fallback resolver + drift telemetry
    completion.ts        # awaitCompletion multi-signal detector
    composer.ts          # fillComposer (contenteditable/textarea, insertText), submit
    extract.ts           # extractLastAssistantTurn (DOM → markdown)
    download.ts          # captureDownload, captureImages → ArtifactSink (content-addressed)
    banners.ts           # regex packs → QuotaSignal classification
    auth.ts              # detectAuthState, threadIdFromUrl
    pacing.ts            # Pacer (worker-enforced)
    probe.ts             # probe() framework: auth + critical locators + capability entry points
    declarative.ts       # DeclarativeChatAdapter (config-only chat providers)
    conformance.ts       # shared conformance suite runner
    fixtures.ts          # fixture recorder/loader (sanitized DOM snapshots)
  test/fixtures/         # html snapshots per state: logged-out, idle, streaming, complete,
                         # limit-banner, challenge

services/subscription-gateway/                       # the daemon
  package.json  tsconfig.json  README.md
  src/
    main.ts              # boot: keychain check (HARDENING D3), store open, workers up, HTTP listen
    config.ts            # env + policy file loading (~/.allternit/subscriptions/policy.yaml)
    http/
      server.ts          # UDS + optional TCP, Host/Origin validation, scoped bearer tokens
      routes_tasks.ts    # POST /v1/tasks, GET /v1/tasks?status=&limit= (read-only summaries), GET /v1/tasks/{id}, POST /v1/tasks/{id}/cancel
      routes_events.ts   # GET /v1/tasks/{id}/events (SSE, AdapterEvent stream)
      routes_artifacts.ts# GET /v1/artifacts/{id}, /export, /download (sandboxed preview)
      routes_accounts.ts # connect/disconnect/status, needs_user queue
      routes_capabilities.ts # live registry view (manifests × probes × pools × health)
    store/
      db.ts              # better-sqlite3, WAL, migration runner
      migrations/        # 0001_init.sql … (accounts, pools, tasks, attempts, artifacts,
                         # thread_mappings, events, adapter_stats, tokens)
      queries.ts
    queue/
      scheduler.ts       # per-(provider,account) queues, priority interactive>normal>background
      fairness.ts
    worker/
      supervisor.ts      # spawn/restart workers, watchdog, stall_timeout per capability
      worker.ts          # owns browser context: 1 interactive page + N watch pages
      reconcile.ts       # sent_unconfirmed recovery (HARDENING A2)
      detach.ts          # detached watch polling (resume_token, backoff)
      progress.ts        # watch-page native-progress scraper → progress/heartbeat events (D11)
      desktop_bridge.ts  # desktop-app driver: CDP-first (Electron/WebView2), UIA fallback, vision last (D14, P7)
    router/
      resolve.ts         # pure resolve(task, snapshot) — no I/O, table-testable
      policy.ts          # ordered lanes: subscription > local > credits > metered; sensitivity
      pools.ts           # quota-uncertainty behavior, cooldowns, circuit breaker
      explain.ts         # rejected[] reasons for observability
    events/
      log.ts             # append-only event ledger (mirrors bot_events pattern)
      sse.ts             # fan-out to SSE subscribers
      outbox.ts          # durable per-caller outbox, acked delivery, reconnect replay (D12)
      notify.ts          # terminal-state push: CommRails peer msg, desktop notif, MCP update (D12)
    catalog/
      subs_models.ts     # subs/<provider>:<model_class> picker registry for gizzi-code/UIs (D13)
    artifacts/
      store.ts           # content-addressed writes, quarantine xattr, MIME verify
      preview.ts         # sandboxed HTML preview bundles
    security/
      tokens.ts          # per-caller scoped tokens, keychain storage
      redact.ts          # screenshot/DOM redaction (crop to composer/response, mask PII)
      navlock.ts         # context route: navigation limited to manifest.origins
    adapters/
      chatgpt-web/
        manifest.yaml  selectors/v1.yaml  adapter.ts  fixtures/  README.md
      kimi-web/
        manifest.yaml  selectors/v1.yaml  adapter.ts  pptx.ts  fixtures/  README.md
      claude-web/
        manifest.yaml  selectors/v1.yaml  adapter.ts  fixtures/  README.md
      gemini-web/                          # P6 — DECLARATIVE ONLY (no adapter.ts): extensibility proof
        manifest.yaml  selectors/v1.yaml  fixtures/
      chatgpt-desktop/                     # P7 — ui_bridge_desktop via CDP (Electron/Windows), UIA fallback
        manifest.yaml  selectors/v1.yaml  uia_paths/v1.yaml  adapter.ts  fixtures/  README.md
      kimi-desktop/                        # P7 — ui_bridge_desktop via WebView2 CDP (Tauri), UIA fallback
        manifest.yaml  selectors/v1.yaml  uia_paths/v1.yaml  adapter.ts  fixtures/  README.md
  test/
    router.test.ts       # table-driven pure-router tests
    conformance.test.ts  # all adapters vs fixtures
    store.test.ts
    http.test.ts         # UDS + token + Host/Origin rejection tests

cmd/allternit/ (existing CLI — add subcommands)
  subs list|connect|disconnect|status
  caps list [--provider]
  task run <capability> [--provider auto] [--prompt ...] [--wait]
  artifacts list|open <id>
```

Bot-facing surface (P5): the gateway also exposes an MCP server with `subscription_task_run`, `subscription_task_status`, `artifact_get` so gizzi-code/Claude-Code-style bots use the fabric with zero new client code.

## 2. Phases (each = one agent session, one reviewable PR)

### P0 — Contracts package
Files: `platform/packages/subscription-fabric-contracts/` complete as laid out above; transcribe REVIEW_CLAUDE.md §S1–S7 + §A1–A2 types verbatim into zod schemas + TS types.
Verify: `pnpm -F @allternit/subscription-fabric-contracts build && pnpm -F @allternit/subscription-fabric-contracts test`.
Gate: schemas round-trip; `ArtifactEvent`/`Task` unions compile; no provider-name literals anywhere in the package.

### P1 — Gateway skeleton + store + security
Files: `services/subscription-gateway/` `main.ts`, `config.ts`, `http/*`, `store/*`, `security/*`, `events/log.ts`, `events/outbox.ts`, `events/notify.ts`. Static router returning "no route" (real router lands P4).
Includes the **completion-push spine** (D12): durable per-caller outbox with acked delivery + reconnect replay, and terminal-state notify (CommRails peer message, desktop notification). SSE push lands here too; progress scraping itself lands in P2/P3.
Also: add 7788 to `docs/Operations/QUICK_REFERENCE.md` port table.
Verify: `pnpm -F subscription-gateway build && pnpm -F subscription-gateway test`; smoke: start daemon, `curl --unix-socket ~/.allternit/subscriptions/gateway.sock -H "Authorization: Bearer $T" http://localhost/v1/capabilities` returns `[]`; TCP without token → 401; bad `Host` → 403; request with `Origin` → 403. Outbox test: client disconnects mid-stream, reconnects, receives missed events exactly once (idempotent on `event_id`).
Gate: keychain-refusal boot path tested (HARDENING D3); migrations idempotent; no caller-polling required to learn a terminal state (proven by test that a disconnected caller gets `completed` via outbox replay).

### P2 — Adapter SDK + conformance + declarative adapter
Files: `platform/packages/subscription-adapter-sdk/` complete; hand-written fixture HTML for the 6 states **plus progress states** (deep-research mid-run with step list, slides mid-generation with partial thumbnails); a fake "fixture-web" declarative adapter in tests.
Includes the **progress-streaming primitives** (D11): generic progress extractors (step lists, counter badges, streaming-text growth) driven by the `progress:` selector group, partial-artifact capture, and the 15 s `progress.heartbeat`.
Verify: `pnpm -F @allternit/subscription-adapter-sdk test` — completion detector against streaming/complete fixtures; progress extractors against mid-run fixtures (step list → `progress` events, partial slides → `artifact.partial`); heartbeat fires with growing `elapsed_s` on a static fixture; banner classification; selector fallback + drift telemetry recorded; `DeclarativeChatAdapter` drives fixture pages end-to-end.
Gate: conformance runner exists and fails loudly on a deliberately-broken selector (negative test); a fixture with no DOM change for >stall_timeout trips `stalled` while a heartbeating one does not.

### P3 — chatgpt-web adapter (chat + image) — first live slice
Files: `adapters/chatgpt-web/` + worker/queue/reconcile wiring in gateway (worker supervisor, scheduler, detach, reconcile from §1 tree land here — they're dead code until a real adapter exists).
Capabilities: `chat.create`, `chat.continue`, `image.generate`. Includes: probe, reconcile, thread-divergence check, pacing, temp-chat default (D5), download capture → artifact store.
Manual gate (human-driven, per HARDENING): `allternit subs connect chatgpt` → visible window, user logs in, probe READY; `allternit task run chat.create --prompt "…" --wait` streams via SSE; kill -9 mid-stream → restart → reconcile adopts or flags ambiguous (never double-submits — verify in provider UI); image task → artifact row with sha256 + local preview.
Automated verify: fixtures for chatgpt-web pass conformance.
**This phase replaces the `chatgpt-image` lane (D6):** media-router's ChatGPT free lane repointed at the gateway; old profile retired only after this slice runs stable.

### P4 — Router + quota pools + observability + subs model catalog
Files: `router/*` real implementation, `pools.ts`, event-ledger queries, `adapter_stats` measurement, `/v1/capabilities` live view, `catalog/subs_models.ts`, CLI `caps`/`subs status`.
Includes the **model-selector catalog** (D13): connected accounts publish `subs/<provider>:<model_class>` entries (with health badges) through the same provider-registry path gizzi-code uses for CLI-subscription entries (`provider_routes.rs`), so the picker shows e.g. "ChatGPT (subscription) · Reasoning" alongside metered models; selecting one submits a `chat.message` fabric task with `options.model_class`.
Verify: table-driven router tests (unknown-pool ranking, degraded skip for background, cooldown backoff ladder, circuit breaker trip, sensitivity exclusion, `rejected[]` explanations); simulated `model_downgraded` event flips pool state; catalog test: entries appear only when health is `ready|degraded`, carry the right `model_class` → task mapping, and hide when `needs_user`.
Gate: lint rule banning provider-name literals in `router/`+`queue/` passes (HARDENING X6).

### P5 — kimi-web (chat + presentation→pptx) + claude-web (chat) + MCP surface
Files: `adapters/kimi-web/` (chat + `presentation.create` with detached execution — Kimi slides run provider-side), `adapters/claude-web/` (chat only), MCP server in gateway.
Includes the **detached-progress proof** (D11): while slides generate, the watch page streams Kimi's native step progress + partial slide thumbnails as `progress`/`artifact.partial` events, with heartbeats; completion is detected by the worker and pushed (D12) — the demo is a caller that submits, goes silent, and gets the pptx delivered without ever asking.
Verify: conformance for both; live: detached `presentation.create` frees the interactive slot within 60s of acknowledgment (prove by running chat task on kimi while slides generate); progress events visible on SSE during generation; pptx lands in artifact store and opens in the office suite (`platform/packages/office-pptx-*` can render it for preview).
Gate: MCP tools callable from a gizzi-code session; refusal path surfaces without fallback (D7); zero-poll completion demonstrated.
Stability gate: no further Kimi artifact families until pptx capture is stable ~2 weeks (HARDENING C).

### P6 — Extensibility proof (gemini-web, declarative-only) + surface integration
Files: `adapters/gemini-web/` with NO adapter.ts (manifest + selectors + fixtures only, driven by `DeclarativeChatAdapter`); surface integration per HARDENING §M — desktop connection-status / needs-user panel in `surfaces/allternit-desktop`, `subs/*` picker entries + progress-feed rendering in chat mode (ai.allternit.com/desktop), cowork approval cards for `external_publish`/metered, MCP already live from P5 (code mode). Single panel per surface, nothing more (HARDENING C).
Verify: gemini-web passes conformance with zero TypeScript — if any code hook is needed, that's a finding against the SDK, fix the SDK not the adapter. Chat-mode slice: pick `subs/chatgpt:reasoning`, watch live progress in-thread, receive artifact without polling.
Gate: human reviews each surface; docs updated (`docs/specs/subscription-fabric/` + this plan marked with as-built deltas).

### P7 — "Allternit Sessions" machine: provisioning tiers + desktop-app adapters
The containment machine (D15) becomes real in all three tiers, plus the desktop lane (D14).
Files:
- **Managed image + provisioning:** reproducible "Allternit Sessions" image — **Windows guest** (required for the desktop lane per `DESKTOP_BRIDGE_RESEARCH.md`; macOS image only for web-lane-only deployments) with gateway, Chrome, provider desktop apps pre-installed at pinned versions, secret store, telemetry. T1 Hosted: provisioning onto the user's Allternit-cloud VPS allotment (`cmd/allternit-computer-cloud` / cloud-wizard). T2 BYOC: enrollment + image deploy through the node agent (`cmd/allternit-node`). T3 Local-contained: local VM via `drivers/apple-vf` / `drivers/firecracker`, falling back to the `bot_desktop_sandboxes` pattern; boot-on-demand/sleep-when-idle.
- **Settings → Sessions Computer panel** in `surfaces/allternit-desktop`: machine status, CPU/RAM/disk/bandwidth against plan allotment (T1) or node-agent telemetry (T2) or local caps (T3), connected accounts with per-lane health, needs-user queue, tier re-provisioning flow, account/state migration via encrypted state bundle.
- **Desktop bridge:** `worker/desktop_bridge.ts` — stack per `DESKTOP_BRIDGE_RESEARCH.md`: ① Playwright `connectOverCDP` against the provider desktop apps (Electron via `--remote-debugging-port`; Kimi/Tauri via WebView2 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`), reusing the web adapters' selector packs where the app renders the same web UI; ② UIA fallback (FlaUI/pywinauto) via `uia_paths/v1.yaml`; ③ self-hosted vision agent (Agent S2 or UI-TARS-desktop, Apache-2.0) as last resort. Same full adapter contract: streaming events, progress scraping (D11 — DOM-level on CDP), multi-signal completion, at-most-once submit, non-spending probe. App-update watchdog: pinned app versions, Electron-fuse detection via probe, update only after conformance passes.
- **Desktop adapters:** `adapters/chatgpt-desktop/` + `adapters/kimi-desktop/`; router dual-lane policy (web default, desktop auto-fallback on `network_error`/geo-block/`provider_down`/challenge loops, shared pools + pacing per account, D14).
Manual gate: provision a T1 machine from a test allotment; **first-deployment CDP verification** — prove `connectOverCDP` against ChatGPT-Desktop-Windows and Kimi-WebView2 on the real image (research flagged both as mechanism-sound but publicly unproven; if either fails, that app ships UIA-first and the gap is recorded); authenticate each provider once through the streamed display; then unattended — a web-lane failure (simulated geo-block → `provider_down`) fails over to the desktop lane for the same capability without caller involvement, with identical progress/completion streaming (D11/D12). Also: T3 local-contained provisioning on a dev machine proves the free-tier path.
Verify: desktop-bridge conformance (fixtures: CDP-driven DOM snapshots + UIA fallbacks); contract parity — the same conformance suite runs against `chatgpt-web` and `chatgpt-desktop`; router test: dual-lane fallback never double-submits; settings panel shows correct telemetry in all three tiers (mocked allotment for T1); fuse-watchdog test — simulated app update without CDP support trips `ui_drift`, not silent failure.
Gate: machine rebuilds from image unattended in every tier; no session material outside the machine; kill-switch wipes it (A6.9); the desktop app never offers an "run uncontained on this desktop" option — that path does not exist.

### Later (schema-ready, explicitly not scheduled)
Metered-fallback execution via `llm_gateway`; multi-account per provider; concurrency >1; headless mode; remaining Kimi families (document/spreadsheet/website); avoided-cost economics UI; website.publish behind the D9 approval gate; remaining desktop adapters (claude-desktop, gemini-desktop) following the P7 pattern.

## 3. Testing strategy (standing, all phases)

- **Unit:** router (pure, table-driven), quota state machine, selector resolver, completion detector, redaction.
- **Conformance:** every adapter vs its fixtures in CI; a new adapter cannot merge red. Fixture recorder produces sanitized snapshots (MHTML) for the 6 canonical states.
- **Contract:** zod round-trip + schema-version guards so a contract bump fails adapters compiled against old shapes.
- **Live canary (opt-in, probe-only):** nightly per connected account; never submits; catches UI drift before users do.
- **Live E2E (human-gated):** the P3/P5 manual gates above; run headful, real accounts, small prompts, counted against pacing budgets.
- Repo rules: no dev servers left running, worktree teardown per AGENTS.md, `scripts/git-discipline-check.sh` PASS at session end.

## 4. Risk register (top 5, with owner)

| Risk | Mitigation | Where enforced |
|---|---|---|
| Double-submit spends quota twice | at-most-once + reconcile, no blind retry | worker + SDK `markSubmitted` (A2) |
| Provider silently downgrades model | `observed_model` per attempt → pool degraded | quota pools (A4/P4) |
| Selector drift breaks adapter silently | probe canary + fallback-strategy telemetry + circuit breaker | SDK selectors + worker (P2/P4) |
| Local daemon abused via browser/DNS-rebinding | UDS default, token always, Host/Origin validation | http server (P1) |
| Silent completion gap — caller must ask to discover done/failed | push-based completion: worker detects, outbox + CommRails/MCP/desktop notify deliver; polling is debug-only | events/outbox + notify (D12, P1) |
| Dead-feeling UX on long provider-side tasks | native progress scraping + 15 s heartbeats + partial artifacts; stall watchdog reads `last_change_at` | worker/progress + SDK extractors (D11, P2/P5) |
| Web surface geo-blocked / restricted, entitlement stranded | desktop-app lane (no URL to block) as automatic fallback for the same entitlement | desktop_bridge + dual-lane router policy (D14, P7) |
| Desktop automation brittleness (framework drift, app updates killing CDP) | CDP-first stack (DOM, not pixels) per research; pinned app versions + fuse watchdog + probe canary; desktop lanes only on the pinned-image Sessions machine | D14/D15 + P7 conformance + update watchdog |
| Vague containment story — "whose machine is this?" | single managed image + three explicit provisioning tiers (Hosted allotment / BYOC node agent / local-contained VM); Settings → Sessions Computer panel makes it visible in every tier | D15 + P7 settings panel |
| Account flagged by provider | real Chrome, human pacing budgets, challenge → halt + human, no headless-by-default | pacing + session health (A5/D4/D10) |
