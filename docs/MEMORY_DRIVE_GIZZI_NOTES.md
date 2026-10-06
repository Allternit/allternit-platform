# Memory Drive — gizzi-code wiring (Phase 1e, 2d, 3b client side)

Status: implemented, focused tests green, uncommitted in `allternit-wt-memory-drive`.
Scope stopped at the coordinator's request; leftovers are listed at the end.

## Files

New, `cmd/gizzi-code/src/runtime/memory/drive/`:
- `format.ts`: a port of the Rust core (`Entry::render/parse`, derived id `entry-<sha256(text\0source\0added)>`, `validate_*`, `rebuild_index`, `remove_entry`, `apply_batch` tree computation). Source rules follow the contract (`/?session=`, `gizzi:session/<id>`, https/loopback, `scheme:label` except unsafe schemes, `imported:unknown`). Also `diffOperations` (stable-id replay intent), `normalizeFiles` (stamps source/date on bare `- fact` lines and moves bullets out of `## Index`), and the managed-folder guard (`twin/`, `cowork/` read-only).
- `secrets.ts`: the existing gitleaks scanner (`teamMemorySync/secretScanner`) plus a port of the server's `mentions_secret` and `scan_secrets`. Runs over every file of a candidate tree. Only returns a boolean.
- `git.ts`: an argv-only git runner. The token goes in the child env `ALLTERNIT_MEMORY_TOKEN` and is read by a `-c credential.helper` that is set after clearing every configured helper. Hooks are off, prompts are off, and inherited `GIT_DIR`, askpass and similar variables are dropped.
- `lock.ts`: an in-process chain plus a cross-process `mkdir` lock, with stale-lock breaking.
- `checkout.ts`: `DriveCheckout`, which handles ensure (clone into a temp dir and rename, or a local init with a seed commit), `write`, `commitWorkingTree`, `sync`, `mergeFrom`, `status`, `history`, `search` and `readFile`.
  - Plumbing commits use a temp index. Nothing calls `reset` or force-pushes.
  - A non-fast-forward push triggers a fetch, a replay of the entry-level diff since the merge base onto the latest files as one commit, and a retry, up to 5 times.
  - Offline: the local commit is kept and `.git/allternit-memory-sync.json` records the pending state and error. After a network failure the process stops retrying network calls for 60 s.
  - A server push refusal is surfaced and not retried. It is detected by `Memory Drive refused this push: <reason>`.
  - Server ref-lock races (`cannot lock ref` and similar) are retried.
  - Invalid hand or tool edits are copied to `<checkout>.rejected/<stamp>/` and the file is restored. A persistent `rejected` notice stays until `gizzi memory sync --dismiss`.
- `paths.ts`: synchronous account resolution and directory layout.
  - Layout: `<data>/memory-drive/<account>/<drive-slug>`.
  - Account is `local` when signed out, `t-<hash>` for `ALLTERNIT_API_TOKEN`, otherwise `u-<hash(userId)>`. The order matches platform-api.
  - Overrides: `GIZZI_MEMORY_DRIVE_ROOT`. A remote memory dir env also moves the root.
  - `GIZZI_MEMORY_DRIVE=0` turns the drive off.
  - `serverDreamingActiveSync()`.
- `platform.ts`: calls to `info`, `mounts`, `settings`, `dreams`, `undoDream` and `mintToken` (`{access, label:"gizzi on <host>"}`). The token store is `<account>/credentials.json`, mode 0600, written atomically.
- `drive.ts`: the `MemoryDrive` namespace.
  - `open(ref)`, `prepare()`, which runs once per process. On first sign-in it merges the signed-out drive up with a real merge commit, then claims the local drive so another account never receives it. It then syncs, refreshes mounts and settings, and syncs the mount checkouts.
  - `remember`, `forget`, `write`, `commitWorkingTree`, `sync`, `status`, `overview`, `contextBlocks`, `search`, `sessionContext` (with a 15 s cache that writes invalidate), `dreams`, `undoDream`.
- `import.ts`: the legacy frontmatter memdir import, with a plan or an apply.
- `report.ts`: plain-text views shared by the TUI and CLI.

Changed:
- `runtime/memory/memory-service.ts` is now a bridge over the drive. Topic files are entries. A legacy `save` writes one bullet with id `legacy-<name>`, so saving the same name again updates it.
- `runtime/memory/kernel-adapter.ts` keeps `search()` only. The row mirror, `importOnce` and the marker are removed. The index direction is now drive → server.
- `runtime/tools/builtins/memory-write.ts` has a new schema: `action` (save|delete), `text`, `type`, `topic`, `id`, `drive`.
  - Source: `gizzi:session/<ctx.sessionID>`. gizzi has no notion of a cloud session.
  - `memory-recall.ts` searches every drive and adds server recall hits.
- `runtime/session/system.ts`: the memory section is `MemoryDrive.sessionContext()`, which contains each drive's MEMORY.md with its checkout path and mount label.
  - `instruction.ts` no longer auto-loads the memdir or the old global store. It still loads `.gizzi/.openclaw` L1 workspace notes.
- Both memdir path files (`src/memdir/paths.ts` and `src/cli/ui/ink-app/memdir/paths.ts`) now resolve `getAutoMemPath`/`getAutoMemPathFor` to the personal drive checkout. The exceptions are an explicit Cowork override or a settings `autoMemoryDirectory`.
  - New exports: `getMemoryDrivePath`, `isMemoryDriveActive`, `getLegacyProjectMemPath`.
  - `src/shared/memdir/paths.ts` re-exports the main copy, so it picks this up unchanged.
- ink-app:
  - `memdir.ts` `loadMemoryPrompt` has a drive branch. It uses file-edit save instructions, includes mounts, and never runs a bare mkdir on the checkout. KAIROS is skipped when the drive is on.
  - `extractMemories/prompts.ts` has a drive format section, and the extraction commits after the fork writes.
  - `query/stopHooks.ts` commits drive edits at the end of each turn, through the new `services/memoryDrive/commit.ts`.
  - `autoDream/config.ts` is off when the server Dream is on. `consolidationPrompt.ts` has a drive Phase 4. `autoDream.ts` commits after a dream. `consolidationLock.ts` keeps its lock file beside the checkout instead of inside it.
  - `/remember` and the `#` quick-add both go through `MemoryDrive.remember`, which writes to `notes.md`.
  - `/memory` shows the drive overview (sync status and errors, files, recent history). It also has `view <path>`, `log` and `sync`, and editing a drive file in $EDITOR commits it.
  - `/memory-search` searches the drive files.
- CLI: `src/cli/commands/memory.ts`, registered in `registry.ts`.

## User-facing commands and strings (for docs)

- `gizzi memory [status]`, `gizzi memory sync [--dismiss]`, `gizzi memory log [-n N]`, `gizzi memory view <path>`, `gizzi memory search <words>`, `gizzi memory import` (dry run, the default) / `--apply` / `--json`, `gizzi memory dreams`, `gizzi memory undo-dream <id>`. `--drive <ref>` is accepted where it makes sense.
- TUI: `/memory`, `/memory view <path>`, `/memory log`, `/memory sync`, `/memory-search <words>`.
- Sync lines:
  - "Sync: local only (signed out). Run `gizzi login` to sync it to your Allternit account."
  - "Sync: N changes waiting to sync — <reason>"
  - "Sync: up to date · last sync …"
  - "The memory server refused the change: <reason>"
  - "Some memory edits were not saved: <path> (<reason>). Your text was kept in <dir>.rejected."
- Env: `GIZZI_MEMORY_DRIVE=0` (off), `GIZZI_MEMORY_DRIVE_ROOT`, `GIZZI_MEMORY_KERNEL=0`.
- Import receipt: `imports/gizzi-memdir.md`, which has headings only and so is never indexed. Imported rows get `source: imported:unknown`, `added` = the file mtime, and `origin: gizzi-memdir`. The source files are never changed.

## Verification (exact)

- `bun test --timeout 120000 --preload ./test/preload.ts test/memory/ test/runtime/memory/ test/memdir/`: **44 pass, 0 fail**. This covers:
  - the new `test/memory/drive/{checkout,drive,import}.test.ts`
  - the rewritten `test/memory/memory-service.test.ts` and `test/runtime/memory/kernel-adapter.test.ts`
  - the existing quick-add, remember-target, abort-leak and paths-env tests
- After that run I added two checkout tests (server refusal, managed folders): **2 pass**.
- `test/commands/completions.test.ts`, `test/memory/quick-add.test.ts` and `test/memory/remember-target.test.ts`: **20 pass, 0 fail**.
- The tests cover:
  - two sessions writing different entries, where both land
  - concurrent same-id writes from separate checkouts, where the rejected push is re-read, nothing is lost, and only one line remains
  - concurrent first init of one checkout
  - account isolation
  - offline pending, then sync on the next session
  - first-login merge without force, where the old remote head stays an ancestor
  - secrets, a malformed source, traversal and transcript paths all blocked with no commit
  - hand edits committed, and invalid ones quarantined
  - import dry run, apply and idempotence
  - MEMORY.md and mount injection
  - the strings the TUI and CLI read
  - the server refusal reason being surfaced
- The concurrent-writer test flaked once (1 in 4 runs) under load 70–117. The cause was the bare remote's `cannot lock ref` being classified as a refusal. Fixed: it is now retried.
- `bun run typecheck`: **could not complete in this worktree** for an environmental reason. `ensure-sdk-dist.sh` fails building `platform/packages/os-contracts` (`src/spine.ts TS2554`, noEmitOnError). The cause is the borrowed `node_modules` (see below), not this change.
  - A direct `tsc --noEmit` was still running when I stopped. Result: see "Typecheck" at the end of this file.
- Environment: this worktree had no `node_modules`. I symlinked `node_modules` and `cmd/gizzi-code/node_modules` to `../allternit-wt-events-p6/…`, which has current deps. Both paths are gitignored or excluded. **Remove both symlinks before you clean up the worktree.**
  - A `bun install --frozen-lockfile` inside `cmd/gizzi-code` failed: workspace deps only resolve from the root.

## Not done (left for the orchestrator)

1. Rerun `bun run typecheck` once the worktree has a real root `bun install`. The SDK/os-contracts dist step failed only because of the borrowed node_modules.
2. The ink-app memdir copy cannot be imported under bun test (a known hang). The `/memory` TUI component and `memoryDriveSubcommand` are only covered through `report.ts` and `MemoryDrive.overview`.
3. TEAMMEM team-memory sync (build flag off) and agent-specific memory (`agent-memory/`) still use their own frontmatter directories. They are not routed through the drive.
4. A cloud-session source (`/?session=<id>`) is not used, because gizzi has no notion of a server-known session. Every gizzi write uses `gizzi:session/<id>`.
5. Offline commits are replayed onto the newer remote as one commit. Only the first-login merge keeps the local commits in history, as a merge commit.
6. No live server run. Everything ran against scratch bare remotes and a mocked platform fetch.
7. Docs: the strings above need to go into `surfaces/docs/guides/memory-drive.mdx` and the CLI docs, which you own. `/memory` links to `https://docs.allternit.com/guides/memory-drive`.

## Typecheck

`node_modules/.bin/tsc --noEmit` (the full gizzi-code project, without the ensure-sdk-dist step) gave **0 errors, exit 0** after I fixed 2 errors, both in my files: the metadata shape in `memory-write.ts` and a fetch cast in `kernel-adapter.test.ts`. After those fixes the touched tests are **51 pass, 0 fail** (`test/memory/ test/runtime/memory/ test/memdir/ test/commands/completions.test.ts`).
