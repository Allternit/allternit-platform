# CLI background task visibility and lifetime

Factory tracking: dag:dag_13911 / wih:wih_7174.

## What changed

PR #1532 (merge 5aca0433dcab6f4d92bcb3a8c4a952f4d3244baa) bridges Claude CLI task_started/task_progress/task_notification and actual async tool results into the shared BackgroundTask store. A foreground reply no longer tears down the owning CLI while work remains. Reads are bounded to 64 KB and validated provider task-output paths. Targeted cancellation waits for the provider control response. Crashes show interrupted work. Restricted-bot approvals still apply after the foreground reply.

PR #1533 (merge d915fde3f30fe277599e760c2e63d7c2f23bfd54) fixes documentation lint exceptions for named runtime/local vendor integrations. PR #1534 (merge 33b05dd12b968bd9af48e32bd70ed7fd50283219) fixes the real provider's task-stopped-before-control-acknowledgement order and permits a bounded graceful exit to persist state.

## Verified evidence

- 60 tests / 0 failures across seven relevant files (312 assertions), including real Bun subprocess stream/control interaction, concurrent jobs, a delayed final-stop acknowledgement, EOF persistence, crashes, output bounds, path validation, rejected stops and retained permission denial.
- Typecheck passes; production CLI compilation passes. Desktop release preflight: 55 passed / 0 failed. Dependency map validates. Documentation links: 531 nav entries / 526 pages / 0 problems. Full docs lint and Mintlify build pass.
- Installed complete Desktop bundle 1.1.7.5638, CLI 2.2.6, embedded source commit 33b05dd12b968bd9af48e32bd70ed7fd50283219. Existing staged UI remains current allternit-ai 56ba714693c89b265ccb73d9b53ef95790347f0a; no renderer feature change was needed because this uses the canonical shared tasks API/events.
- Live media QA blk1analh appeared running after its chat reply, streamed actual ffmpeg output, and completed after 1m16s. The generated media-check-report.md confirms all four real promo exports fully decoded. Final live Stop test b7qy5ifau cancelled after 15s, retained output and had no connection error.
- The unpublished 2.2.5 release workflow was cancelled when live Stop testing exposed the acknowledgement race. No 2.2.5 GitHub release was published.

## Known limits and incidents

Claude CLI 2.1.300 may coalesce an old task-notification reminder and a genuine next request into the same user content block on resume. The provider can ignore the request as automated input, causing no-model-output. A clear retry worked in the observed case. This provider prompt behavior is not claimed fixed; user and developer docs record it. Lifecycle tracking, completion and targeted Stop are independently verified.

The shared allternit checkout could not fast-forward because other sessions have overlapping uncommitted checkpoint, dependency-map, lockfile and docs changes. Nothing was stashed, reset or replaced. Repo-wide git discipline reports the stale shared main and pre-existing unmerged branches; this session does not claim that check passes. Older Desktop build artifacts in another session's worktree are left with their owner. Our replaced build 5634 artifacts and temporary installed-app backup were removed after verification.

Recordly remained stopped throughout this fix. No social post, new recording, user-data reset, installed-app binary patch, or Cloudflare deployment was performed. The complete app bundle was replaced and restarted.

## Release channel status

The 2.2.6 release workflow 38682536821 succeeded in every job, including quality, all five platforms, main npm publish and GitHub Release. npm latest reports 2.2.6. Homebrew tap PR #2 merged (c7b6bbf), the owner machine upgraded successfully, and gizzi --version reports 2.2.6 with embedded commit 33b05dd12b968bd9af48e32bd70ed7fd50283219. All five archives were downloaded and SHA256-verified; packaging manifests were refreshed with script/update-packaging-hashes.sh 2.2.6. Ruby syntax, Arch shell syntax and JSON/version/hash assertions pass.

The verified Desktop bundle and DMG now live in the canonical shared generated release directory (build 5638). This session removed 106 dependency directories, its replaced DMG and app backup. The Factory task evidence is preserved with the telemetry campaign before deleting the temporary checkout. Other agents' source edits and owned build artifacts remain untouched.
