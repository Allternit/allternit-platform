# Checkpoint — Phones 2.0 (session/phones-2)

## Goal
Linked phone gets a computer-equivalent in-app viewport (scrcpy-over-WS, no VNC), free SIM call/text lane for phone-linked users, Telnyx lane unified into the bot phone — shipped on Desktop + ai.allternit.com phone layout + m.allternit.com PWA before any merge. Approved plan 2026-10-17; Factory DAG: dag_391310.

## Just did
- Phase 0: fresh worktrees off origin/main (`allternit-wt-phones-2`, `allternit-ai-wt-phones-2` — note: main checkouts were 141/250 commits stale). Dependency-map impact recorded: `src/components/bot-phone` has NO feature entry in features.json (Phase 5 must add the linked-phone feature + fix phones-bridge curation).
- Launched two parallel build agents: platform (phone-scrcpy protocol, phone-stream manager, IPC extension, agentBusy flag) and ai (phone-stream transports + WebCodecs client, PhoneViewport, PhoneAppsSheet, ACI wiring). Shared renderer↔main contract pinned in both briefs (startStream/stopStream/streamStatus/onStreamMeta/onStreamChunk/onStreamEnd/sendStreamInput/setStreamDisplayPower; H.264 Annex-B; scrcpy pinned version TBD by platform agent against repo-resolved scrcpy binary).

## Next
- Integrate Phase 1 reports → run full test suites both repos → Gate 1 smoke on Eoj's linked Android via Desktop dev-mode (NOT an installed-app build; per build rules, installed app only ever from merged main).
- Phase 2: Rust relay in cmd/allternit-api mirroring computer_ws.rs token/proxy pattern (`/ws/phones/:serial/stream`, HMAC mint, phone_link_sessions registry), Desktop persistent session client, CloudWsTransport, LinkedPhoneOverlay on phone layout/PWA. Local API testing on port 18013 — never 8013.

## Open questions
- DAG bookkeeping: `gizzi workspace node add` runs clean from the worktree but `node list` shows (none) — DAG CLI surface unclear; plan id dag_391310 recorded here as source of truth fallback.
- Desktop release: our branch touches surfaces/allternit-desktop → `node scripts/release-preflight.mjs` must be 26/0 before merge (release-lock commandment).
- PWA deploy stays manual-wrangler + Eoj go-ahead; ai.com auto-deploys on main merge (that merge IS the deploy — gated on Gate 2 passing on all three surfaces first).
- Steering commit-gate may intercept session commits; if it blocks, PRs wait for Eoj instead of force-overriding.

## Constraints worth remembering
- Call audio cannot leave the phone (Android ≥10) — calls dial on the physical phone, browser shows progress; SMS fully two-way via content://sms + isms.
- Desktop dev-mode smoke must set ALLTERNIT_USER_DATA_DIR to a scratch dir; never point data dirs or port 8013 at the installed app's.
- pnpm only in worktrees; CARGO_TARGET_DIR=.shared-target/allternit-wt-phones-2 for any cargo.
