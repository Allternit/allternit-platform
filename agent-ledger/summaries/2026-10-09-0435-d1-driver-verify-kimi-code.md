# D1 driver verify/land — verification fixes and live measurement (kimi-code)

Date: 2026-10-09 (~04:00–04:35 CDT). Agent: Kimi Code (verification subagent). Branch `feat/allternit-driver`, PR **#1446**, merge commit **9d09c12b9a** (the D1 build itself landed earlier the same morning as PR #1445, squash c87ae69332, authored by the build session).

## What this session did

Verified phase D1 (Allternit Driver sidecar) of `SPEC-allternit-driver-fast-path-2026-10-09.md` against the §2/§4 checklist and landed the fixes verification found:

- **Reviewed the D1 commit** (arc fork pinned at `6b35303` with LICENSE/NOTICE; one JSON-RPC sidecar over arc+Cua; element map with stable ids; router with defaults + learned table; per-window reader/input locks; Desktop manages the sidecar on mac/win/linux with a bundled relocatable CPython 3.12.13 + PyObjC 12.2.2; allternit-api pixel_* input through the sidecar socket; docs + dependency map in the same PR).
- **Found and fixed a real bug**: `Driver.verify()` and `run_batch`'s cross-check read Cua `verify_state`'s result as a boolean `satisfied`/`ok` key; cua-driver-rs 0.34 answers `status: "satisfied"`, so every Cua-engine verify returned `ok:false` even when satisfied (this also silently broke the non-macOS post-batch cross-check). Fixed by parsing the status string (boolean shapes kept as fallback) + regression test. Verified live: Calculator `verify {role: button, name: "7"}` now `ok:true` through verify_state (was `ok:false`).
- **Fixed two pre-existing desktop typecheck errors** that stale `node_modules` had hidden on main (`runtime-package.ts` `cache: 'no-store'` vs undici's `RequestInit`; `mesh-bridge-server.test.ts` `res.json()` as `any`). Desktop `pnpm typecheck` now clean; CI's "Typecheck and build desktop" went green on #1446.
- Restored `pnpm-lock.yaml` after a filtered `pnpm install` rewrote peer variants (uncommitted — not shipped).

## Measured (this Mac, macOS 14.6, sidecar booted from the branch, load avg 9–26)

| App | Cold first read (2 fresh sidecars) | Re-read median | After-switch (prewarm) | Bar |
|---|---|---|---|---|
| Calculator | 99–165 ms (rounds: 109, 99; one load-stalled 165) | 3–9 ms | 5 ms | ≤150 / ≤20 |
| TextEdit | 93–132 ms (16–17-element window) | 3–9 ms | 4 ms | ≤150 / ≤20 |
| Finder | 108–127 ms | 3–5 ms | 9 ms | ≤150 / ≤20 |

All bars met across two independent cold boots. Caveat recorded: #1445 measured TextEdit with a 47-element document window at 199 ms — per-element AX latency (~3 ms/element, upstream arc identical), covered by prewarm for frontmost apps and E4's live map later. `since` diffs, `wait` with conditions, `pixel_get_cursor_position`, router learning and `audit.jsonl` rows all exercised live.

## Verification

Driver unittest 10/10 (one new); desktop typecheck clean; desktop vitest 390/390; `cargo check -p allternit-api` clean; `cargo test -p allternit-api --lib computer` 123 passed; `release-preflight.mjs` 55/0; docs `check_links.py` 0 problems; dep-map impact ok, `--validate` shows one pre-existing `feature:artifacts` error caused by the stale allternit-ai sibling checkout (present with my changes stashed; not from this branch). Post-merge live check on merged main: `read_ui` Calculator 103.8 ms cold, `verify` ok=true, `GET 127.0.0.1:8013/healthz` → 200.

## Environment facts established

- **cua-driver-rs 0.34.0 is the newest release** (2026-10-05, verified on GitHub). `run_actions` is not shipped in it; its MCP advertises 55 tools without it. Diff reads exist only on Cua's main branch — the sidecar's own element-map `since` diffs and its own batch executor are the working implementation; delegate batches to Cua when a release ships `run_actions`.
- Cua's `permission_denied` for `run_actions` was policy denying a tool 0.34 doesn't have; `standard` mode is correct (`bounded` can't cover arbitrary apps, `unrestricted` drops approvals).

## Honest deferrals

- **Guest switch is NOT done** (D1b, as #1445's body already flags): cloud Linux guests still dispatch xdotool/scrot and Windows guests PowerShell/user32 through `computer_toolset.rs`. Needs the driver packaged into guest images + a socket channel + dispatch fallback; no live guest was reachable from this machine (no tart/lima/docker VMs; platform computers need auth; provisioning not authorized).
- allternit-api↔sidecar socket wiring is unix-only; Windows this-device input falls back to the Cua CLI (fine for D1; contract v2 in D2 should close it).
- Desktop bundle was not rebuilt (lifecycle step 8): the D1 sidecar ships with the next Desktop release; D9 rebuilds the headless image.

Worktree `allternit-wt-d1-driver` removed after merge; branch `feat/allternit-driver` deleted local + remote (fully merged).
