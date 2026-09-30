# Agent Gateway shipped to main + production; live checks (step 5) handed off

- **Date/Time:** 2026-09-30 22:00 CDT
- **Agent:** claude (Opus 5.5), session "Agent Gateway final stretch"
- **Handoff doc:** `allternit-workspace/HANDOFF-agent-gateway-2026-09-30.md` (top blocks)

## What was done (attested)

| Area | PRs |
|---|---|
| Rust contract fixes R1–R10, vendor memory, outbound channel logging, `gateway_acceptance` e2e walks | platform #1013, ai #291 |
| QUICKSTART replayed for real; 6 breaks fixed (pnpm start, `?offline=1` conformance, provider errors, CDP ports 9231/9232, 503 GATEWAY_OFFLINE, failed open → UNBOUND) | platform #1018 |
| main synced into integration (1 conflict, coordinator) + web mock fix | platform #1018, ai #304 |
| **Integration → main (Eoj approved)**; prod DB snapshotted first | platform #1019, ai #305 |
| CI green pass on main: gitleaks fixtures, Discord test flake, gizzi bubblewrap + AcpGateVerdict typecheck, hosted runtime image (context + pnpm overrides), frozen lockfile in build-desktop.sh | #1020, #1021, #1022, #1023, #1024, #1035 |

Production: allternit-api on Contabo at V201 (V198–V201 applied 2026-09-30 13:57Z), service healthy; gateway routes 401 unauthenticated from the internet. Snapshot: `/var/lib/allternit-api/backups/allternit-pre-V198-20260930T134621Z.db` (sha256 d0df7a30…) + local copy in `allternit-workspace/prod-backups/`.

Tests on the tree that shipped: Rust full `allternit-api --lib` 1571 pass / 0 fail; web full vitest 629/629 files, 4645 tests; subscription-gateway 483; contracts 59; agent-gateway 27; aai-sdk 9; Python 5. Details: `docs/gateway/ACCEPTANCE.md`.

## Outstanding work — step 5 live checks (PICK UP HERE)

All need Eoj's accounts or consent; none can run unattended. Ask Eoj before each.

| # | Check | Prereq | Pass = |
|---|---|---|---|
| 1 | ChatGPT dots | joe-bb redeployed the Sessions gateway with platform #1030 (dots use the preferred Ready ChatGPT subscription); `SUBS_GATEWAY_DOTS_CONSENT=1` | a vendor-bound bot turn round-trips, events in the thread ledger with vendor attribution |
| 2 | Claude Desktop + ChatGPT.app via AX transport (#991) | Eoj grants Accessibility to `ax-bridge`, OKs each app | same as 1, per app; bot cannot answer a vendor approval |
| 3 | Grok Bot (optional re-run) | Grok relaunched with `--remote-debugging-port=9231` (new default) | QUICKSTART §3 calls answer |
| 4 | Slack / Teams / Discord / WhatsApp + Muse | real workspace / tenant / server / Meta business number | inbound → thread, outbound remote id stored, reconnect no dupes, Muse shows vendor + transport |
| 5 | OpenClaw | OpenClaw running on :18789 | `?offline=1` already passes; live conformance ok:true |
| 6 | Full product path (web → API → Sessions computer → gateway → vendor) | a bound Sessions computer | a turn from the web app lands with provenance |

Out of scope by decision: Claude via CDP (vendor-blocked), Claude Managed Agents (no key; plumbing for users' own keys).

Record results in `docs/gateway/ACCEPTANCE.md` (flip the `blocked:` rows).
