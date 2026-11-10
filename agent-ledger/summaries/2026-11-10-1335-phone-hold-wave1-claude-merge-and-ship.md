# Phone approval hold (Phone and Messages, Wave 1): merge and ship attestation

User authorization: Eoj said "merge all and deploy" on 2026-11-10. That covered the two merges and the six ship steps recorded here, and nothing after them.

Agent: Claude (Claude Code), two sessions on 2026-11-10. The first merged and started the ship; the second finished it.

## What merged

Platform PR https://github.com/Allternit/allternit-platform/pull/2196 (code) merged as 43cc1199d. Platform PR https://github.com/Allternit/allternit-platform/pull/2186 (docs) merged as 9e4f0f0ae. Merge commits.

A phone action with effect pay, delete, send, call, credential, money_app or security_setting becomes an approval hold (`gateway_approvals` row, inbox kind `approval`, `subkind: "phone_action"`). Desktop `dispatch()` enforces it; the runtime (`cmd/allternit-api/src/phone_guard.rs`) decides. A phone hold is Review-only: approve from a notification, Live Activity, widget, control, intent, Siri or shortcut answers `409 review_required`. Contract page: `surfaces/docs/api/phone-holds.mdx`.

## Validation before the merge

`cargo check -p allternit-api -p allternit-cloud-api --tests` clean. allternit-api targeted tests 81 passed plus the 2 new guard tests. Desktop `tsc` main and preload exit 0; `vitest run src/main/phone` 678 of 678. Smoke-boot of allternit-api and cloud-api: no panic. SDK `generate.py --check` up to date. Docs link check 0 problems.

allternit-cloud-api targeted tests: 145 passed, 1 failed, `services::executor_service::tests::execute_run_completes_and_mirrors_events` (`PoolTimedOut` connecting to a database). The file is not touched by this work. It was not compared with main.

## Ship evidence

- Cloud API: deploy run 42837907744 success; `https://api.allternit.com/health` 200; `/v1/phones/x/tool` answers `missing_api_key`.
- Docs: `https://docs.allternit.com/api/phone-holds/` 200 and contains `review_required`.
- Hosted runtime: run 42838116956 failed in Test (the Mac-only Messages test, fixed by #2203). Run 42840905344 (2ac7345e8) success. `GET https://mail.news.allternit.com/api/v1/phones/holds/x` answered 501 before the deploy and 401 after.
- Cloud computers: `Deploy Desktop Cloud VPS` run 42846979543 (04493b4a3, started by hand) success, job `deploy-and-e2e`. Three earlier runs were cancelled because this workflow shares the concurrency group `deploy-allternit-api-contabo` with the hosted runtime deploy.
- Desktop build: b7175 (platform 9e4f0f0ae, ai f92cbc576), build queue ok after 56m46s.
- Runtime packages: Mac `r20261110.1553-73d5f37-727` and Linux `r20261110.1612-7330f65-285`, both live on `runtime.allternit.com/stable/<platform>/latest.json`. Mac binaries were built at 9e4f0f0ae and Linux binaries at 04493b4a3; main moved only by test, packaging, cloud-api, console and docs commits (#2203, #2204, #2201, #2205), checked by path before each publish. Screens are from ai 0ccf7289.
- Installed app on Eoj's Mac: 1.1.13 build 7175, `codesign --verify --deep --strict` passes, `app.asar` contains `guard_unavailable`. After a restart the app applied the Mac package: `runtime/state.json` active `r20261110.1553-73d5f37-727`, and allternit-api and gizzi-code run from `runtime/versions/r20261110.1553-73d5f37-727/bin`. On 127.0.0.1:8013: `/health` 200, `POST /api/v1/phones/guard/check` 401 and `GET /api/v1/phones/holds/x` 401 without a token.
- Linux smoke-boot in the build container: `/health/live` 200, db and jwks ready, holds route 401, 0 panics. `/health` answered 503 there because no Gizzi brain runs in that container.

## Limits

- Not checked on a real phone. The six cases (pay, delete, send, call, password field, bank app) on a linked Android phone and a linked iPhone are not run. Until then Wave 1 is proven by tests and route checks only.
- Not checked: a cloud computer applying the Linux runtime package, and real webhook delivery for `phone.approval.*`.
- The hold's label rules are English only. An action no rule recognises can still run without a hold. The docs say so.
- The program is not finished: this is Wave 1 of 3. The review card on web, PWA, iOS and Android, the Messages entry points and the iPhone on-phone test are not started.
- The shared deploy slot is not changed; that is Eoj's decision.

## Cleanup

Session branches and worktrees were removed after the merge. Old Desktop build files were pruned with `scripts/build-state.sh --prune`; b7175 is the one DMG kept. The Linux builder's inotify setting is back at 128. Its disk is 98% full (5.9 GB free of 290 GB); nothing was deleted there.

One leftover: a source-only worktree, `allternit-workspace/wt-ledger-phone-hold` (detached at 5cdf0e6d9, no build output), made for this record. The terminal lost macOS access to the Desktop folder before it could be used or removed, so this record was committed from a ledger-only clone. Remove it with `git worktree remove ../wt-ledger-phone-hold`.
