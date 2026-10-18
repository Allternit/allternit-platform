# Bot voice on the linked phone — W-streams landed (kimi-code session dcd065ce)

- **Date:** 2026-10-18 ~08:15 CDT
- **Scope:** SPEC-real-phone-voice-2026-10-18 + CONTRACT-linked-phone-voice-2026-10-18.md — complete the six-W-stream combine, merge both PRs, apply prod pg migrations, deploy cloud-api, clean up.
- **PRs:** platform [#1658](https://github.com/Allternit/allternit-platform/pull/1658) (merge `ea1a7f8ce0`), ai [#629](https://github.com/Allternit/allternit-ai/pull/629) (merge `012fbb165bc4`). ai.allternit.com Pages production Active on `012fbb1`.

## What was done

1. **Completed the stalled combine** (the prior session died mid-combine in `allternit-wt-lpv-all`): committed its stranded caller-trust refactor in `allternit-api/src/voice_calls.rs` (`label_caller` helper + stranger-label test), merged the never-merged `lpv/cloud` (this restored `migrations_pg/094_linked_phone_calls.sql`, absent from the combine), merged fresh `origin/main` twice.
2. **Resolved merge conflicts** with main's bot-phone work as semantic unions: W5's `deviceId` live-call matching + Call audio sheet coexist with main's `embedded` / `openDeviceInBotPhone` (ai repo: LinkedPhonesSection, PhoneViewport, ConnectedPhoneRow; dropped `usePhoneNav`/`viewPhoneInAci`, dead after main's Open-flow change).
3. **Prod pg migrations by hand** (`ssh root@mail`, psql ON_ERROR_STOP): applied **090–094 in order** — prod was missing 090/091/092/093 (pre-existing gap; main's cloud-api already referenced `registered_phones`, which did not exist in prod). Verified: `voice_linked_call_rooms` created, `voice_calls.number_id` nullable, `line`/`line_kind`/`ended_at` added, `registered_phones.call_bridge` added.
4. **Fixed three CI failures on the PRs** (all reproduced locally first): unescaped `<label>`/`<phone>` in the two developer guides broke mint MDX parsing ("file does not exist"); desktop main-process `fetch` delegation missing the DOM type's `preconnect` property; fabric-session SW cache not bumped for changed PWA assets (→ v128).
5. **Cloud-api production deploy** via `scripts/deploy-cloud-api.sh` after a full-tree rsync to `/opt/allternit-build` (drift repair) + `cargo metadata` check.

## Verification evidence

- Platform PR #1658: all 10 checks green (docs lint, desktop typecheck+build, vitest, typecheck, smoke, audit, gitleaks, ratchet, typography, cache-bump).
- Post-deploy: build-tree md5 == running binary (`6dd0578f…`); `/api/v1/health` 200; **new `GET /api/v1/voice/calls/live` answers 401 (exists, not 404 — proves the linked-calls build is live)**; `/api/v2/artifacts` 401; journal clean; `ALLOW_DEV_TOKEN` absent.
- Remote cloud-api lib suite: 874 passed / 1 failed / 2 ignored — the one failure identified separately in this summary's follow-up (see below).
- docs `check_links.py` 0 problems; `release-preflight.mjs` 58/0; ai repo tsc/eslint clean on the changed components.

## Found, not caused (pre-existing on main, left for their own sessions)

- `infrastructure/vps-node` e2e test target didn't compile (reqwest `multipart` feature) — **fixed here** (one-line), since it blocked workspace checks.
- `cargo check --workspace --all-targets` fails in `domains/kernel/drivers/allternit-rlm` (MemoryFabric/SkillRegistry::new, RLMEnvironment) — untouched by this branch; no CI job runs an all-targets check.
- allternit-ai full-suite vitest has cross-file pollution (main's clean checkout: 7 files / 9 tests fail); CI gates no vitest there.
- Remote `cargo test -p allternit-cloud-api --lib` 1-failure signature matches the 2026-10-10 runbook entry (`migration_tests::migrations_pg_applies_cleanly` — fresh-install-from-migrations_pg is broken on main; prod unaffected). Re-verified during this deploy.

## Honest deferrals (owner: Eoj, per the spec)

- m.allternit.com PWA deploy (manual wrangler step, needs Eoj's go).
- Allternit Desktop build with the call-bridge (needs Eoj's go; release-preflight green).
- Hardware live test once the wired USB-C cable kit arrives.
- The pre-existing breakages listed above deserve their own fixes; this session did not take them on except the vps-node compile blocker.

## Cleanup

All seven `lpv` worktrees removed, `lpv/*` branches deleted local and remote, shared checkouts on `main` at the merge commits. Final state: `scripts/git-discipline-check.sh` PASS (output in session record).
