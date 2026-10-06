# A://Labs — Curriculum-as-Code Pipeline

> **STATUS:** Production-ready. 10 courses, 65 modules, 0 audit issues.
>
> **LAST UPDATED:** 2026-09-14 (disk hygiene added)

## Commandment: approved work lands on main — no stranded branches, no stale checkouts

Eoj's rule, effective 2026-09-14: **if work is approved, it is merged to `origin/main` and every local checkout is refreshed before the session ends.** The 2026-09-14 incident: three local checkouts sat 475–1218 commits behind `origin/main` — the Fabric Transport UI had been reshaped and production had been broken and restored by merges that none of the local checkouts contained, so answers given from those checkouts described a product that no longer existed. Approved work that lives only on a `session/*` branch or in one machine's folder does not exist for anyone else.

1. **Merge before you leave.** A session is not done at "PR open." Steps 5–6 of the session lifecycle below are mandatory: merge the PR (merge commit, not squash), then sync the shared checkout. An approved-but-unmerged PR is unfinished work, and the session summary must say so honestly.
2. **No orphaned session branches.** After merge, delete the session branch local AND remote (lifecycle step 9). If you find a stale `session/*`/`ao/*`/`wip/*` branch — including ones that predate this rule — verify it is merged (`git branch --merged origin/main`) and delete it; if it is NOT merged, that is a red flag: surface it in `.steering/checkpoint.md` and to the owner before touching it.
3. **Every long-lived checkout stays on `main`, fast-forwarded.** Machine checkouts (e.g. `allternit-main-check`, `allternit-ao-fabric-*`) live on a local `main` that tracks `origin/main`. First action when opening one for real work: `git fetch origin && git pull --ff-only`. Never start work from a detached HEAD, never from a branch a previous session left behind.
4. **Check freshness before answering questions about the codebase.** If asked "what changed", "does X exist", or "why does X look different" — `git fetch origin` and compare `HEAD..origin/main` before answering. A stale checkout lies. State the SHA you based the answer on.
5. **Worktrees are still the default for implementation** (see below) — this commandment is about where work ENDS: merged to `origin/main`, local checkouts fast-forwarded, session branches deleted.
6. **Show proof, not claims.** "Merged to main" is a claim and has been falsely claimed too often (2026-09-18 audit: the shared checkout sat on a detached HEAD, 93 behind `origin/main`, with 21 unmerged stale branches). Every session ends by running `scripts/git-discipline-check.sh` — it exits non-zero unless the checkout is on `main`, `main == origin/main`, there are no unmerged branches (beyond live worktrees and `.steering/git-discipline-allowlist`), and the tree is clean. Paste its PASS output verbatim into the session summary as the final-state evidence block. If it fails, the session is NOT done and the summary must say honestly what is unfinished. Enforcement is mechanical, not optional: the `git-discipline-gate.sh` Stop hook (registered for every CLI in `.steering/bin/steer-install.sh`, runs before the steering consult) blocks any turn from ending while the shared checkout is detached, behind/ahead `origin/main`, or holding unmerged stale branches — a dirty tree warns but does not block, since another session's in-flight work may legitimately live there.

## Commandment: ai.allternit.com is the workspace, platform.allternit.com is the cloud console

Do not call the workspace "Allternit Cloud" or put it in a repo named `allternit-cloud`.

| Product | Domain | Source | Pages project |
|---|---|---|---|
| Agent workspace | `ai.allternit.com` | private `Allternit/allternit-ai` | `ai-allternit` |
| Cloud console | `platform.allternit.com` | this repo `surfaces/platform.allternit.com` | `allternit-platform` |

- This public repo **must never** `wrangler pages deploy --project-name=ai-allternit`.
- Desktop packages the workspace UI from `allternit-ai` at build time, not from `surfaces/platform.allternit.com`.
- Editing the console does not change Desktop or `ai.allternit.com`.
- The GitHub name `allternit-cloud` was a dump label from the 2026-09-15 OSS split and has been renamed to `allternit-ai`. GitHub redirects the old URL.

## Commandment: disk hygiene — shared build dirs, teardown cleanup, no orphaned artifacts

The 2026-09-14 audit found **234 GB** in `~/Desktop/allternit-workspace`: a 42 GB Rust `target/` in the main clone, a 24 GB `target/` left behind by one dead session worktree, ~9 GB `node_modules` per worktree, and 19 GB of desktop DMGs — with macOS swap squeezed at 77% disk full. Dep/build artifacts are reproducible; keeping them is never worth the space. Rules:

1. **Shared Rust target dir.** Every session exports `CARGO_TARGET_DIR="$HOME/Desktop/allternit-workspace/.shared-target"` (created on first use) before running cargo. One physical build cache for the whole workspace; cargo's file lock serializes concurrent builds across sessions (this also caps the rustc-process RAM spikes). Never `cargo clean` the shared dir to "fix" a build — diagnose instead.
2. **pnpm only.** Worktrees install deps with `pnpm install` only. Never `npm install` / `npm ci` in a worktree — it physically duplicates what the pnpm global store already hardlinks. (Root uses pnpm 10 via `packageManager` in `package.json`.)
3. **Teardown cleanup is part of step 9.** Before `git worktree remove`, the session deletes its own `node_modules/`, `target/` (never the shared one), `release/`, and `dist/` directories — all reproducible from lockfiles. A worktree is not "removed" until its artifacts are.
4. **Desktop artifacts — exactly ONE built version on disk at any time (hard requirement).** `surfaces/allternit-desktop` keeps exactly one verified DMG, exactly one unpacked app bundle (`release/mac-arm64/...`), and the current sidecar set in `resources/bin/`. Multiple concurrent terminals keep rebuilding the app; if old builds linger, stale versions get run and bug reports describe code that no longer exists. Therefore: the moment a new bundle/DMG is verified (lifecycle step 8), **every** older DMG, `.blockmap`, and unpacked app bundle is deleted in the same session — not archived, not "kept until later", regardless of which terminal or session produced it. A build is not done until it is the only one on disk. The same rule covers gizzi-code binaries; `bash scripts/build-state.sh --prune` does the deletion (see the "one current build" commandment).
5. **Session-start disk gate.** Before creating a session worktree, check free disk (`df -h`). If the workspace is over 150 GB or free space is under 50 GB, stop and surface it: cleanup is the task, not a side effect. Growth without cleanup is how the 234 GB happened.
6. **Kill what you started.** Dev servers, `vite preview`/`npm/pnpm run dev`, tests (a `bun test` once hung for 29+ hours), playwright/driver processes, python launchers, and background `tail`s are session-scoped: terminate them before the session ends. A process left running after its session dies is orphaned (reparented to launchd); orphaned processes with executables under the workspace are the ones that piled up for days and fed the TCC/Desktop attribution failures. The workspace janitor (`~/.allternit/bin/workspace-janitor.sh`, launchd `com.allternit.workspace-janitor`, every 6h) TERM→KILLs orphans older than 12h as a backstop — being janitor-killed is session debt, not an accident.

## Commandment: desktop-v1.1.1 release lock

`desktop-v1.1.1` (2026-09-09) is the **locked, known-running desktop release** — the first release whose bundled `allternit-api` + `gizzi-code` include the native-sessions routes, and the product of a 14-run repair night (see `agent-ledger/summaries/2026-09-09-0601-relfix6-20260908-kimi-code-desktop-release-repair.md`). It cost that much because each breakage was only visible after the previous one burned a 40–60 min CI run. The rules below exist so that never happens again:

1. **Never repoint, move, or delete the `desktop-v1.1.1` tag.** It is the reference known-good state. New releases get new tags (`desktop-v*`).
2. **Any change touching the desktop release path must keep the release workflow green before merge.** This includes `.github/workflows/release-desktop.yml`, `surfaces/allternit-desktop/` (electron-builder config in `package.json`, `scripts/prepare-*.cjs`, `scripts/notarize.cjs`, `build/`), `services/voice/`, `services/local-engine/`, `cmd/gizzi-code/script/build-production.js`, and the workspace deps those builds rely on. Run `node scripts/release-preflight.mjs` from the repo root before merging such a change — if it is not 26/0, the release is broken; do not merge.
3. **Sidecars are hard requirements, not extras.** `prepare-platform-static.cjs` requires `allternit-api`, `gizzi-code`, `allternit-voice-service`, `whisper-cli`, and `mesh-node` in `resources/bin/` per platform. If you change how one is built (or delete its source tree, as the voice-cleanup did), you must update the producing workflow step AND `scripts/release-preflight.mjs` in the same PR. A workflow that references a nonexistent file is a release failure even if unit tests pass.
4. **New features must compile in the production configurations, not just dev.** gizzi-code is bundled by `bun run script/build-production.js` (Bun.build — which does NOT apply tsconfig `paths` to dynamic imports; optional native packages are resolved via `bundlePlugin.onResolve` stubs). Rust sidecars are built per-target in CI (`x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) — verify `cargo build --release` for your crate locally before merging.
5. **Windows-specific gotchas are load-bearing:** node-gyp needs `GYP_MSVS_VERSION=2022` (the runner's VS18 is invisible to it), pnpm spawns need `shell: true` (.cmd shims), and NSIS hard-fails on any missing `extraFiles`/`license` path. Do not "clean up" these without a green Windows run.
6. If a merge to main breaks the release workflow, **the fix is mandatory before the next desktop release is cut** — open a tracked blocker (issue or `agent-ledger` entry naming the failing run) immediately, and repoint the latest `desktop-v*` tag only after the run is green, never to dodge a failure. While the workflow is red, **release-path work is paused; all other repo work continues** — this rule must not gate unrelated sessions.

## Commandment: one current build per product — look before you build (gizzi-code + Allternit Desktop)

Eoj's rule, effective 2026-09-26. Several terminals build and ship gizzi-code and Desktop at the same time, and each one has been overwriting the others. On 2026-09-26 alone: a 12:21 Desktop install from a stale branch overwrote merged PRs #68/#69; a 16:20 `platform/` sync into `~/Applications/Allternit Desktop Preview.app` overwrote merged #77/#78; Homebrew gizzi-code 2.1.2 shipped without the already-merged #762/#763; a gizzi 2.1.1 session kept running after the upgrade; and 16 stale gizzi binaries (back to March) sat in `dist/`. The fix is uniform: every agent looks at the current build state first, builds only from main, ships through the official channels, and deletes what the new build replaces.

1. **Look first — mechanically enforced.** Before any build, release, or install of either product, run `bash scripts/build-state.sh` from the repo root. It reports: builds ACTIVE in other sessions right now; the latest release tag, its commit, and how many merged commits are still unreleased; what Homebrew/npm/the installed apps actually contain; which running processes use an old copy; and every STALE artifact across all worktrees. The `guard-build.sh` PreToolUse hook (registered for Claude Code, codex, kimi and gizzi by `.steering/bin/steer-install.sh`) blocks build/release/install commands unless the report ran in the last 30 minutes, and blocks a second concurrent build of the same product. Override only deliberately: `BUILD_GUARD_OFF=1`.
2. **Build only from latest `origin/main`.** Fetch first and build from main (or a commit main contains). A binary from an unmerged session branch is never installed into an app, released, or copied over someone else's build. If your change isn't merged, it isn't shippable.
3. **One build per product at a time.** If the report shows an ACTIVE build of the same product, wait for it or coordinate with its session (`ListAgents`/`SendMessage`). Don't start a parallel one.
4. **Ship through the official channels — all of them, every time.**
   - **gizzi-code:** bump `cmd/gizzi-code/package.json` on main → push tag `gizzi-code/vX.Y.Z` (CI `publish-gizzi-code-npm.yml` publishes npm + the GitHub Release) → update `Allternit/homebrew-tap` `Formula/gizzi-code.rb` (version + every platform sha256 from the release assets) → `bash cmd/gizzi-code/script/update-packaging-hashes.sh X.Y.Z` to move the in-repo manifests (homebrew/scoop/arch/rpm/deb/winget) to the new version and hashes, committed via PR → `brew upgrade gizzi-code` on the owner machine → `gizzi --version` matches. The Desktop sidecar is the second channel: rebuild Desktop with `resources/bin/gizzi-code` built from the same commit (lifecycle step 8).
   - **Allternit Desktop:** DMG from main per lifecycle step 8, installed by replacing `/Applications/Allternit Desktop.app` — not alongside it; official releases via a new `desktop-v*` tag under the release-lock commandment.
   - Never patch an installed app in place (copying a binary or `platform/` into any `Allternit Desktop*.app`), and never keep a second installed copy such as a "Preview" app — both drift from main and hide which build is running.
5. **New replaces old, in the same session.** Once the new build is verified, run `bash scripts/build-state.sh --prune`: it deletes stale gizzi binaries and Desktop DMGs/bundles in the shared checkout, your worktree, and workspace-level backup dirs, and runs `brew cleanup gizzi-code`. It refuses while any build is active, never touches the installed app, the current Homebrew version, or user-data backups. Stale copies in another session's worktree: message that session instead of deleting them (worktree ownership is absolute).
6. **Old copies still running.** The report flags processes on a replaced version (`OLD`). Don't kill another session's process; tell that session to restart.
7. **Verify by commit, not by date.** gizzi-code binaries embed `GIZZI_BUILD_SHA` (from `script/build-production.js`); the report prints it for every copy. A ship is done when the installed/released copy shows the commit you intended.

## Session worktrees (default for ALL repo work)

Every agent session in this repo works in its OWN linked worktree — never in the shared main checkout. On your first prompt (or SessionStart), a hook injects the ritual: create-or-reuse `<repo>-session-<id>` on branch `session/<id>` and `cd` into it. A PreToolUse guard blocks `git commit/checkout/switch/merge/push/rebase/reset` and `branch -d` in the shared checkout (escape for human/orchestrator merges: `STEER_GUARD_OFF=1`). Rationale: concurrent sessions sharing one HEAD collide on branches, commits, and dirty files. gizzi-code additionally has native `--worktree` support (`src/shared/utils/worktree.ts`); making it default-on is tracked as phase W2. Linked worktrees pass all guards automatically (detected via the git dir path).

**Local gateway port ownership.** 8013 belongs to the installed Desktop (and VPS). `allternit-api` binds it **only when `ALLTERNIT_API_PORT=8013` is set explicitly** — the unset default is 18013 so a stray `cargo run` cannot SIGTERM the installed gateway (the Fabric "relay offline" failure class). 18013 is a fuse, not a second product you run. Never export `ALLTERNIT_API_PORT=8013` from a session shell unless you deliberately intend to replace the installed app's gateway.

**Local data-dir ownership.** There is one Desktop sqlite: Electron userData `<appData>/@allternit/desktop/allternit`, migrated only by the installed binary. A worktree binary must never point `ALLTERNIT_DATA_DIR` at that path (V47/V83/V86/V93 mismatch). Unpackaged / `npm run dev` is not a second product: set `ALLTERNIT_USER_DATA_DIR` / `ALLTERNIT_DATA_DIR` to a scratch you own, or the process gets an ephemeral temp dir. Packaged ignores both env vars, matching `ALLTERNIT_API_PORT`. `--user-data-dir` (Playwright) is left alone. Cargo-run of `allternit-api` without Electron still defaults to `<appData>/allternit` — that is also not the product DB; never point it at `@allternit/desktop`.

**Worktree ownership is absolute.** A worktree belongs to the session that created it (or, for long-lived non-session worktrees like `allternit-desktop-preview`, to the machine/owner). Never run `git checkout`/`switch`/`reset`/`merge`/`rebase` in a worktree you did not create. To consume newer main in a worktree you don't own, move **forward only**: `git fetch origin && git checkout --detach origin/main` (or `git pull --ff-only` if it tracks main) — never sideways to another branch, never backwards to an older commit. Wiping a sibling session's uncommitted work by re-pointing its checkout is how the 2026-09-09 shell-rail session lost a full edit pass. If you believe a worktree needs a different state, write your findings in `.steering/checkpoint.md` and leave the checkout alone.

## Session lifecycle — the full repo process (do ALL of it, every session)

Agents that stop at "code works in my worktree" leave debt for the next session. A session is not done until all of this is done. Canonical example: session `0f55144a` (2026-09-07, PR #105).

1. **Worktree.** Create `<repo>-session-<id>` on branch `session/<id>` from latest `main`; `cd` into it. Never edit the shared checkout (it may hold other sessions' uncommitted in-flight work — leave that untouched).
2. **Plan.** After scoping with the owner, enter the work into the Factory WIH DAG (`gizzi workspace plan new`, then per-node `gizzi workspace node claim`) — see "Planning and task tracking" below for the >2-step rule. A plan file may be drafted as scratch while scoping, but the DAG is the source of truth once it exists. Update `.steering/checkpoint.md` (`Goal` / `Just did` / `Next` / `Open questions`) at every milestone.
3. **Implement and verify.** Every claim checked before you make it: typecheck, unit tests, `cargo check`/`cargo test` for Rust, and a live smoke test (run the server, `curl` the endpoints) for anything behavioral. Note pre-existing breakage as pre-existing; don't silently fix unrelated files.
4. **Commit and push.** Logical commits (conventional-ish prefixes: `feat(...)`, `fix(...)`, `docs(ledger): ...`), push the session branch to origin. Never commit directly on main except step 7.
5. **PR and merge.** `gh pr create` with a real summary + verification evidence, `gh pr merge <n> --merge` (merge commit, not squash — keeps session chunk history). Record the PR number and merge SHA.
6. **Sync main.** In the shared checkout: `git pull --ff-only` (the pull is allowed; only mutating git verbs are guarded).
7. **Attest.** In the shared checkout write the dated summary `agent-ledger/summaries/YYYY-MM-DD-HHMM-<session-id>-<agent-family>-<topic>.md` (what was done / how it works / verification evidence / incidents / honest deferrals) and append the one-line entry to `agent-ledger/LEDGER.md`. Commit directly on main as `docs(ledger): attestation for session/<id>` with `STEER_GUARD_OFF=1 git ...` and push. The ledger is a signed record — be honest about what was deferred.
8. **Rebuild the desktop binary from merged main.** After the attestation lands, rebuild so the runnable preview matches the canonical codebase — a binary built from a pre-merge or wiped worktree state is stale even when the source ritual is complete. In `surfaces/allternit-desktop`: ensure the sidecar preflight is satisfied (`resources/bin/` — copy sidecars from the shared checkout or run the repo-root `scripts/build-desktop.sh` stages; `ALLTERNIT_ALLOW_MISSING_API=1` / `ALLTERNIT_ALLOW_MISSING_LOCAL_ENGINE=1` only as explicit escapes), then `npm run build` (main/preload/auth-renderer) and `npx electron-builder --mac dmg` with `CSC_IDENTITY_AUTO_DISCOVERY=false` for unsigned local builds (no APPLE_ID creds). Verify the bundle actually contains the change (`grep -r "…" "release/mac-arm64/Allternit Desktop.app/Contents/Resources/platform/assets/"`), keep the previous DMG until the new one is verified, then delete the old binary. Skip this step only when the session did not touch anything the desktop bundles. Run `bash scripts/build-state.sh` before this step and `--prune` after it; if the session changed `cmd/gizzi-code`, also ship the gizzi-code release through its channels (npm + GitHub Release + Homebrew tap) per the "one current build" commandment.
9. **Clean up.** `git worktree remove`, delete the session branch local AND remote, delete scratch logs/build artifacts you created, and confirm final state (`git status`, `git worktree list`). Then run `scripts/git-discipline-check.sh` — if it does not PASS, the session is not done (commandment 6). Resumable-state exception: if the session is interrupted before merge, leave the worktree + branch + checkpoint intact so another agent can resume.

## Session landing — worktree cleanup

A session's worktree is temporary scaffolding, not a permanent workspace. Clean up so the machine does not accumulate orphaned worktrees, branches, or scratch files.

### Ongoing hygiene

Clean as you go, but never discard work that might be needed to resume.

- **Checkpoint frequently.** Commit meaningful progress and push the `session/<id>` branch to origin often so an interrupted session does not lose work.
- **Clean only disposable scratch.** During the session, delete temporary logs, debug dumps, and downloaded artifacts as soon as they are no longer needed.
- **Protect active work.** Do not delete a worktree, branch, or uncommitted changes that contain unfinished but viable work. If you are unsure whether something is still needed, leave it and document its purpose in `.steering/checkpoint.md` or the session summary.
- **Leave resumable state.** If the session stops for any reason, another agent (or a resumed session) should be able to inspect `git status`, `git branch`, and `git worktree list` and understand what was in progress.

### Final cleanup

Final cleanup happens only after the change is safely in the canonical codebase.

- **Merge first, then clean up.** Push and merge the change to the GitHub codebase, then merge it into the local `main` checkout, before doing any cleanup.
- **Write the session attestation.** Before deleting the worktree, record what was done, how it works, the commit SHA, and any unfinished work. Create a dated summary file in `agent-ledger/summaries/` using the naming convention `YYYY-MM-DD-HHMM-<session-id>-<agent-family>-<brief-topic>.md`. Append a short entry to `agent-ledger/LEDGER.md` that links to the summary file. Treat this as a signed ledger: be honest about what was actually completed versus what was deferred or left unfinished.
- **Delete the session worktree.** Once the work is merged and no longer needed, remove the `<repo>-session-<id>` worktree directory and delete the `session/<id>` branch. Do not leave stale session worktrees on the machine.
- **Remove scratch artifacts.** Delete local logs, temporary scripts, build outputs, downloaded dependencies, and debug files that are not intended to be committed.
- **Restore the original branch.** Return to the branch you started from unless the task explicitly required switching branches.
- **Verify the final state.** Before ending the session, run `scripts/git-discipline-check.sh` (and `git worktree list`) and confirm nothing unexpected remains. Its PASS output is the evidence block for the session summary.
- **No local technical debt.** The machine should be left in the same clean state it was in before the task started, with no orphaned branches, worktrees, or leftover files.

## Steering checkpoints

This repo is wired for hook-based steering: when an agent session working here ends a turn, a `Stop` hook consults a **separate steering agent** (a different model family, reached through `gizzi orchestration steer consult`) — but only if `.steering/checkpoint.md` changed since the last review. So at every meaningful checkpoint (subtask finished, design decision made, before a risky change), update `.steering/checkpoint.md`: `Goal`, `Just did`, `Next`, `Open questions`. The steering agent's answers/guidance come back injected as a `[steering]` message — treat them as authoritative and act on them before continuing. Additionally, `git commit`/`git push` pass through a hard gate: they only execute after the steering agent approves. See `.steering/README.md`. Kill switch: `touch .steering/off`.

## Planning and task tracking

Multi-step work is tracked in the Allternit Factory WIH DAG, deterministically —
not by agent discretion. Ratified per `factory/engine/spec/DAG_AS_DEFAULT_TASK_SYSTEM.md`.

- **The rule.** If a session expects to take more than two steps, or its work
  will be picked up, reviewed, or continued by another session, it must be
  represented as DAG nodes under a `plan` before execution: `gizzi workspace
  plan new "<goal>"`, broken into nodes (`gizzi workspace node add|claim|close`). Work of two steps or less may stay
  ephemeral (no DAG required).
- **The DAG is the source of truth.** A markdown plan file is scratch for
  drafting only; the moment the DAG exists, node statuses replace the
  checklist. Session handoffs reference `dag:<dag_id>` / `wih:<wih_id>`, never
  a plan-file path.
- **Track by node status.** Todos live as DAG node statuses (`NEW` → `READY` →
  `RUNNING` → `DONE`/`FAILED`); readiness comes from `ready_nodes`, not from a
  checklist in a markdown file.
- **Verify against the DAG.** Before calling a task complete, review the plan's
  nodes and ensure every node is `DONE` or explicitly deferred with a reason.
- **No dual tracking.** Do not maintain a parallel plan file alongside the DAG.
  One source of truth; the temporal boundary is: plan file = pre-DAG scratch
  only.

## Agent creation checklist

> **STATUS:** Canonical schema, registry contract, harness wiring, surface filtering, automation bridge, and workspace artifacts are implemented and passing verification as of 2026-07-02.
>
> See [`AGENT_CREATION_CHECKLIST.md`](./docs/AGENT_CREATION_CHECKLIST.md) for the canonical schema, harness wiring, workspace artifacts, mode surfaces, and verification steps that every agent must satisfy.

## What Is This?

A://Labs is Allternit's learning platform. It turns the Allternit codebase into interactive, self-contained HTML course modules that are synced to Canvas LMS. The entire pipeline is automated — from code analysis → module generation → Canvas publishing → progress tracking.

## Quick Stats

| Metric | Value |
|--------|-------|
| Total Courses | 10 (7 original + 3 ADV) |
| Total Modules | 65 |
| Total Assignments | 51 |
| Canvas Launch Audit | 0 issues |
| Interactive Demo Modules | 10 |

## Course Catalog

| Code | Title | Tier | Course ID | Modules |
|------|-------|------|-----------|---------|
| ALABS-CORE-COPILOT | Build AI-Assisted Software | CORE | 14593493 | 7 |
| ALABS-CORE-PROMPTS | Prompt Engineering | CORE | 14593495 | 7 |
| ALABS-OPS-N8N | Orchestrate Agents & Automations | OPS | 14593499 | 9 |
| ALABS-OPS-VISION | Computer Vision for Agent Systems | OPS | 14593501 | 6 |
| ALABS-OPS-RAG | Local RAG & Document Intelligence | OPS | 14593503 | 7 |
| ALABS-AGENTS-ML | ML Models as Agent Tools | AGENTS | 14593505 | 6 |
| ALABS-AGENTS-AGENTS | Multi-Agent Systems & Orchestration | AGENTS | 14593507 | 7 |
| ALABS-ADV-PLUGINSDK | Build Plugins for Allternit | ADV | 14612851 | 4 |
| ALABS-ADV-WORKFLOW | The Allternit Workflow Engine | ADV | 14612861 | 3 |
| ALABS-ADV-ADAPTERS | Provider Adapters & Unified APIs | ADV | 14612869 | 3 |

## Directory Structure

```
allternit/
├── AGENTS.md                              ← YOU ARE HERE
├── alabs-generated-courses/               ← Generated HTML modules
│   ├── ALABS-ADV-PLUGINSDK-module1.html
│   ├── ALABS-ADV-PLUGINSDK-module2.html
│   ├── ALABS-ADV-PLUGINSDK-module3.html
│   ├── ALABS-ADV-PLUGINSDK-bridge.html
│   ├── ALABS-ADV-WORKFLOW-module1.html
│   ├── ALABS-ADV-WORKFLOW-module2.html
│   ├── ALABS-ADV-WORKFLOW-bridge.html
│   ├── ALABS-ADV-ADAPTERS-module1.html
│   ├── ALABS-ADV-ADAPTERS-module2.html
│   ├── ALABS-ADV-ADAPTERS-bridge.html
│   ├── quizzes/                          ← Quiz JSON files for Canvas Quiz API
│   │   ├── pluginsdk-m1.json
│   │   ├── workflow-m1.json
│   │   └── adapters-m1.json
│   └── analysis/                         ← Package analysis outputs
│       ├── package-analysis.json
│       ├── curriculum-map.json
│       └── platform-course-outline.json
├── alabs-generated-courses/demos/        ← Standalone demo site (was top-level alabs-demos/)
│   ├── index.html                        ← Auto-generated landing page
│   └── cowork-integration-preview.html
├── alabs-module-template/                ← Shared template system
│   ├── shell/shell.html                  ← Common CSS + JS wrapper
│   ├── scripts/build.ts                  ← Build: content JSON → HTML
│   ├── scripts/convert-existing.ts       ← Migrate old modules to new format
│   └── README.md
├── archive/alabs-curator/                ← ARCHIVED 2026-07-22: generalizable CLI scaffold (never finished; stub publish)
├── scripts/                              ← Pipeline scripts
│   ├── sync-course-from-package.ts       ← Main sync (fixed page_url bug)
│   ├── sync-incremental.ts               ← Hash-based incremental sync
│   ├── canvas-quiz-sync.ts               ← Canvas Quiz API integration
│   ├── progress-tracker.ts               ← Poll Canvas → SQLite progress
│   ├── add-module-challenges.ts          ← Adds challenge assignments
│   ├── launch-audit.ts                   ← Validates all courses
│   ├── polish-adv-courses.ts             ← One-shot polish for ADV courses
│   ├── generate-demo-index.ts            ← Auto-builds demo landing page
│   ├── analyze-packages.ts               ← Codebase → topics/challenges
│   ├── platform-as-course.ts             ← Platform → course outline
│   └── fix-unpublished-modules.ts        ← Publishes + sets prerequisites
├── (workspace UI) Allternit/allternit-ai   ← LabsView / demos live there, not here
└── .agents/skills/
    └── alabs-course-pipeline/
        └── SKILL.md                      ← Agent skill for pipeline usage
```

## The Pipeline

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  Code Analysis  │────▶│ Module Generate │────▶│  Canvas Publish │
│  (TypeScript)   │     │  (Agent Swarms) │     │  (REST API)     │
└─────────────────┘     └─────────────────┘     └─────────────────┘
         │                       │                       │
         ▼                       ▼                       ▼
  analyze-packages.ts     Agent prompts in          sync-incremental.ts
  platform-as-course.ts   .agents/skills/           canvas-quiz-sync.ts
                          alabs-course-pipeline/    progress-tracker.ts
                          SKILL.md
```

## How To: Common Tasks

### 1. Generate a New Module

Use an agent swarm. The skill at `.agents/skills/alabs-course-pipeline/SKILL.md` has the full prompts. Short version:

```bash
# Read source package
# Generate interactive HTML module
# Write to alabs-generated-courses/ALABS-ADV-{COURSE}-module{N}.html
```

Module requirements:
- Self-contained single HTML file
- Dark theme (`#0b0b0c` bg, tier-colored accent)
- JetBrains Mono + Inter typography
- Progress bar + fixed nav + scroll-reveal
- Syntax-highlighted code blocks
- 3 interactive quizzes with instant feedback
- 1 Canvas animation (DAG, scheduler, circuit breaker, etc.)
- Capstone project section

### 2. Sync Module to Canvas

**Incremental (recommended):**
```bash
cd ~/Desktop/allternit-workspace/allternit
npx tsx scripts/sync-incremental.ts \
  --html-file alabs-generated-courses/ALABS-ADV-WORKFLOW-module2.html \
  --course-id 14612861 \
  --module-title "Module 2: The Scheduler & Execution Model" \
  --position 2
```

**Legacy (if incremental fails):**
```bash
npx tsx scripts/sync-course-from-package.ts \
  --html-file alabs-generated-courses/ALABS-ADV-WORKFLOW-module2.html \
  --course-id 14612861 \
  --module-title "Module 2: The Scheduler & Execution Model" \
  --position 2
```

> ⚠️ **KNOWN BUG & FIX:** The original `sync-course-from-package.ts` generated `page_url` from the module title, which could mismatch Canvas's URL slug. It was fixed to use `page.url` from the Canvas API response. If you see `invalid page_url parameter`, the script needs this fix.

### 3. Publish Module & Set Prerequisites

```bash
npx tsx scripts/fix-unpublished-modules.ts
```

This publishes all unpublished modules and sets sequential prerequisites (M1 → M2 → M3 → Bridge).

### 4. Add Canvas Quiz (Real Scoring)

Create a quiz JSON:
```json
{
  "title": "Module 1 Quiz: Topic Name",
  "questions": [
    {
      "question": "What is...?",
      "options": ["A", "B", "C", "D"],
      "correctIndex": 1,
      "explanation": "Because..."
    }
  ]
}
```

Sync to Canvas:
```bash
npx tsx scripts/canvas-quiz-sync.ts \
  --course-id 14612851 \
  --module-title "Module 1: Plugin SDK Architecture" \
  --quiz-json alabs-generated-courses/quizzes/pluginsdk-m1.json
```

Result: Real Canvas Quiz with scoring, attached to the module.

### 5. Add Module Challenge

Edit `scripts/add-module-challenges.ts` — add entry to `CHALLENGE_ASSIGNMENTS` map:
```typescript
'ALABS-ADV-WORKFLOW': {
  'Module 2: The Scheduler & Execution Model': {
    title: 'Challenge: Build a Retry-Aware Scheduler',
    description: '...',
  },
},
```

Run:
```bash
npx tsx scripts/add-module-challenges.ts
```

### 6. Run Launch Audit

```bash
npx tsx scripts/launch-audit.ts
```

Checks all 10 courses for:
- Unpublished modules
- Missing prerequisites
- Module/item counts
- Assignment completeness

### 7. Update Demo Site

```bash
npx tsx scripts/generate-demo-index.ts
```

Scans `alabs-generated-courses/` and `alabs-generated-courses/demos/` → generates `alabs-generated-courses/demos/index.html`.

### 8. Analyze a Package for Topics

```bash
npx tsx scripts/analyze-packages.ts --package platform/packages/plugin-sdk
```

Outputs:
- `alabs-generated-courses/analysis/package-analysis.json` — exports, types, complexity
- `alabs-generated-courses/analysis/curriculum-map.json` — auto-generated syllabus

### 9. Track Student Progress

```bash
npx tsx scripts/progress-tracker.ts --user-id 12345
```

Polls Canvas for module completion → updates SQLite `certifications` table.

## Canvas API Constraints

- **Free For Teacher plan** on `canvas.instructure.com`
- `POST /accounts/self/courses` returns **403** — course creation requires browser automation (Playwright)
- Token is hardcoded in scripts (production would use env var)
- Rate limits: ~100 requests/minute

## Database Schema (Certifications)

```sql
CREATE TABLE certifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  userId TEXT NOT NULL,
  courseCode TEXT NOT NULL,
  progress INTEGER DEFAULT 0,
  completedAt TEXT,
  status TEXT DEFAULT 'in_progress',
  updatedAt TEXT,
  UNIQUE(userId, courseCode)
);
```

## Design System (Module Template)

| Token | Value | Usage |
|-------|-------|-------|
| `--bg` | `#0b0b0c` | Page background |
| `--accent` | Tier color | Highlights, badges, links |
| `--accent-dim` | `rgba(color, 0.15)` | Subtle backgrounds |
| `--text` | `#e5e5e5` | Primary text |
| `--text-secondary` | `#a1a1aa` | Secondary text |
| Font body | Inter | All text |
| Font code | JetBrains Mono | Code blocks |

Tier colors:
- CORE: `#3b82f6` (blue)
- OPS: `#8b5cf6` (purple)
- AGENTS: `#ec4899` (pink)
- ADV: `#f59e0b` (amber/gold)

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `invalid page_url parameter` on sync | URL slug mismatch | Use `sync-incremental.ts` or the fixed `sync-course-from-package.ts` |
| `pnpm` commands fail | Workspace name conflict (`@allternit/visual-state` in two places) | Use `npx tsx` directly instead of `pnpm` |
| `better-sqlite3` migration fails | Native bindings missing | Use system `sqlite3` CLI for migrations |
| Canvas quiz not attaching | Module doesn't exist yet | Sync the HTML module first, then attach quiz |
| Agent generation timeouts | Large codebase to analyze | Give agent specific file paths, not broad globs |

## Agent Swarm Prompt Template

When generating a new module, use this structure:

```
Generate an interactive HTML course module for A://Labs ADV tier.

Source: Read [specific files]
Title: Module N: [Topic]
Course: ALABS-ADV-[COURSE]
Tier: ADV (amber #f59e0b)

Required sections:
1. Hero
2. The Problem
3. [3-5 content sections with real code]
4. 3 Interactive Quizzes
5. Capstone Project

Design: Self-contained, dark theme, JetBrains Mono + Inter,
  progress bar, fixed nav, scroll-reveal, syntax highlighting,
  ONE Canvas animation, fully inline CSS/JS.

Output: ~/Desktop/allternit-workspace/allternit/alabs-generated-courses/ALABS-ADV-[COURSE]-module[N].html
```

## Future Work (Backlog)

- [x] Migrate Canvas token from hardcoded to env-based — **Done.** Use `scripts/canvas-token.ts` (`getCanvasToken()` reads `CANVAS_TOKEN` / `CANVAS_API_TOKEN` with legacy fallback).
- [x] Add "Platform as Course" (ALABS-PLATFORM: 5 modules, ~15 hours) — **Done.** Outline saved to `alabs-generated-courses/platform-course-outline.json` and `alabs-generated-courses/analysis/platform-course-outline.json`; course added to platform `FALLBACK_COURSES`.
- [x] Auto-extract quiz JSON from generated modules (instead of hand-writing) — **Done.** Run `npx tsx scripts/extract-quizzes-from-modules.ts`.
- [ ] Migrate all existing modules to shared template shell (reduces size ~60%)
- [ ] ~~Complete `alabs-curator` CLI (generalize for any codebase)~~ — archived to `archive/alabs-curator/` 2026-07-22 (scaffold with stub publish, superseded by `scripts/alabs-course-pipeline.ts`); restore from archive if revived
- [ ] Build module generation directly into template system (agents output JSON, build script wraps)
- [ ] Add completion webhooks (Canvas → platform notifications)

## Tool Belt, MCP, and ACI Documentation

Phase 4 added public docs for the agent runtime surfaces. When working on tools, MCP integrations, or computer-use features, consult the relevant reference first:

- [`docs/public/tools/tool-belt.md`](./docs/public/tools/tool-belt.md) — Native Tool Belt: `web_search`, `web_fetch`, `bash`, `code_execution`, `memory`, `str_replace_editor`, and `computer`.
- [`docs/public/tools/mcp.md`](./docs/public/tools/mcp.md) — Attaching MCP servers, server-side execution, bundled/remote directory pattern, and tunnel security.
- [`docs/public/tools/strict-tool-use.md`](./docs/public/tools/strict-tool-use.md) — Strict JSON Schema validation and grammar-constrained inputs.
- [`docs/public/tools/native-sessions.md`](./docs/public/tools/native-sessions.md) — Native sessions: picking up CLI agent sessions (Claude Code, Codex, Kimi, …) in gizzi-code, Desktop, and the web app; pickup/fetch/export and supported CLIs. Operator/API reference: [`docs/NATIVE_SESSIONS.md`](./docs/NATIVE_SESSIONS.md).
- [`docs/public/aci/index.md`](./docs/public/aci/index.md) — Allternit Computer Interface overview, browser automation, and vision coordinates.
- [`docs/public/guides/build-a-tool.md`](./docs/public/guides/build-a-tool.md) — Step-by-step guide for registering custom tools.

## Key Contacts / Context

- **Canvas Instance:** Free For Teacher, `canvas.instructure.com`
- **Node Version:** v25.6.1 with `tsx`
- **Database:** SQLite (`better-sqlite3`) + PostgreSQL (Prisma)
- **Workspace UI:** private `Allternit/allternit-ai` (Vite + React). Console is `surfaces/platform.allternit.com/`.
- **Course IDs:** See catalog table above
- **Generated modules:** Stored in `alabs-generated-courses/`
- **Demo site:** `alabs-generated-courses/demos/index.html` — works offline

---

**If you are an agent reading this:** You have everything you need to generate, sync, quiz, audit, and track courses. Do NOT start from scratch. Build on what's here.

---

## Platform Integration

### Demo Files in Platform

Demo HTML files must be copied to the platform's public directory to be served:

```bash
cp alabs-generated-courses/demos/*.html ../allternit-ai/public/demos/
```

The `LabsView.tsx` "Try Demo" buttons link to `/demos/ALABS-ADV-{COURSE}-module1.html` which resolves to `public/demos/` in the Vite app.

### Keeping Demos In Sync

After generating new modules:
1. Copy to `alabs-generated-courses/demos/`
2. Copy to `Allternit/allternit-ai` `public/demos/`
3. Regenerate index: `npx tsx scripts/generate-demo-index.ts`
4. Copy updated index to both locations

### Platform UI Updates

When adding new courses/modules, update:
- `Allternit/allternit-ai` `src/views/LabsView.tsx` — `ALABS_COURSES` array
- Module counts, descriptions, demo URLs

---

## Allternit Factory — agent communication and coordination

This repo uses the **Allternit Factory** as its unified communication and coordination substrate (it replaced the older work engine and agent orchestrator; old commands, routes and env names are removed, not aliased — see `surfaces/docs/factory/migration.mdx`):

- `factory/engine/` — Rust library crate (`allternit-factory-engine`); `cmd/allternit-factory/` is the engine binary (internal, never typed by users).
- `cmd/allternit-api/src/rails/mod.rs` — the in-process Factory core router (`factory_core_router()`), mounted at `/api/factory` (peers, steer, mail, ledger, receipts, wihs, plans, leases, gate, vault, …).
- `cmd/gizzi-code/src/runtime/gizzi-core/services/railsPeer.ts` — gizzi-code peer registration + HTTP inbox poller.
- `cmd/gizzi-code/src/cli/ui/ink-app/components/RailsInboxBridge.tsx` — bridges polled envelopes into the TUI mailbox.

Every local agent session can register itself as a **peer** under `.allternit/peers/`. Peers can discover each other and send plain-text messages — the Allternit equivalent of Claude Code's `ListAgents` / `SendMessage`. Messages never leave the machine. UDS sockets are supported for direct push; gizzi-code uses HTTP polling of the durable Bus inbox. Any CLI can participate by registering and polling the HTTP inbox; `.allternit/mux` is not required for Factory messaging. Terminal bots started with `gizzi agents up` are registered automatically.

### HTTP surface (`/api/factory`, served by allternit-api)

- `POST /api/factory/peers` — register a peer (`{ name, cwd, vendor }`).
- `GET /api/factory/peers` — list peers.
- `POST /api/factory/peers/:name/send` — send a message to a peer by name.
- `POST /api/factory/peers/:name/heartbeat` — keep a peer marked active.
- `POST /api/factory/steer/checkpoint` — hash `.steering/checkpoint.md` and emit a `SteeringCheckpoint` ledger event when it changes.
- `POST /api/factory/steer/consult` — build steering context and consult the configured backend.
- `POST /api/factory/steer/commit-gate` — commit/push approval consult.

The full route table is in `surfaces/docs/factory/architecture.mdx` ("Routes allternit-api serves in process").

From the shell, people use `gizzi`:

```bash
gizzi agents ps                                  # peers and bots, reconciled with live panes
gizzi orchestration send <bot@team> "<message>"  # message a bot
gizzi orchestration steer checkpoint|consult|commit-gate --cwd <dir>
gizzi agents doctor                              # harnesses, panes, transport
```

The engine's hidden maintenance CLI covers the raw peer commands (`register`, `list`, `send`, `heartbeat`, `inbox`): `allternit-factory internal core peer <verb> …`.

From gizzi-code, the runtime exposes:

- `ListPeers` (alias `ListAgents`) — discover local agent peers.
- `SendMessage` (alias `SendMessageToPeer`) — send to a Factory peer by name, with teammate-mailbox fallback.

### Peer mode in gizzi-code

Factory peer registration is default-on (set `ALLTERNIT_FACTORY_PEER=0` to opt out). It registers the session as `gizzi-<sessionId>` with the Factory API and polls the HTTP inbox for peer messages. The process also exports:

- `ALLTERNIT_FACTORY_PEER_NAME`
- `ALLTERNIT_FACTORY_INBOX`

### Verification

- `cargo test -p allternit-factory-engine` and `cargo test -p allternit-factory`
- `cargo build -p allternit-api`
- `bun run typecheck` in `cmd/gizzi-code`
- `cmd/gizzi-code/test/rails-peer-e2e.ts` registers two peers, lists them, and confirms Bus/UDS message delivery.

### Docs

- `surfaces/docs/factory/overview.mdx` — what the Factory is; `commands.mdx` — every `gizzi` command; `migration.mdx` — old → new names.
- `docs/programs/rails/RAILS_PRODUCT_UPDATE_SYSTEM_PROMPT.md` — the pre-Factory product-update prompt (history; superseded by the Factory docs).

## Agent email rail (services/mailflare)

`services/mailflare/` is a **vendored fork** of [hieunc229/mailflare](https://github.com/hieunc229/mailflare) that gives agents real internet email (inbound webhook → Factory mail threads; outbound via the Factory mail review gate). Conventions:

- It is a plain **npm** project with its own `package-lock.json` and OpenNext/Cloudflare build — like `services/open-connector`, it is **excluded from the pnpm workspace** (`!services/mailflare` in `pnpm-workspace.yaml`). Never add it to the workspace; root `pnpm install` ingesting it breaks its Next.js build.
- Verify changes with `npm run build` (lint has pre-existing upstream errors; don't add new ones). Type checking is `ignoreBuildErrors`-gated upstream, so run `npx tsc --noEmit` when touching TS.
- Per-installation deploys go to the installing user's own Cloudflare account via `services/mailflare/setup.sh`.
- Full architecture, ops, and reputation guidance: `docs/AGENT_EMAIL_RAIL.md`.
