# Allternit Factory build: rules for every workstream

Owner/orchestrator: the Claude session that spawned you. CommRails plan: `dag_37876` (root node `n_3397`).
Read first: `SPEC.md` and `API.md` in this folder (`~/Desktop/allternit-workspace/scratch/factory/`). Eoj has approved the design and authorized building it, including the builds, typechecks and tests needed to verify your work.

## Branches and PRs (hard rules)
- Base branch: `factory/integration`, in both `Gizziio/allternit-platform` and `Gizziio/allternit-ai`. Your worktree is already on your own branch `factory/<stream>`, created from it.
- When done: commit, push your branch, and open a PR **with base `factory/integration`** (`gh pr create --base factory/integration`). **Never** push to or open a PR against `main`. **Never** merge anything. **Never** deploy (no wrangler, no cloud-api, no Desktop release, no runtime package, no brew/npm publish). Merges to main auto-deploy production. The orchestrator merges into integration, and Eoj gates integration → main.
- End every commit message with:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01Js14Sza8WYCZpMrxwNvrJG`
  End the PR body with:
  `🤖 Generated with [Claude Code](https://claude.com/claude-code)` then a blank line, then `https://claude.ai/code/session_01Js14Sza8WYCZpMrxwNvrJG`.
- Only edit files inside your **territory** (listed in your task). Other streams are working in parallel on the others. If you truly must touch something outside it, keep it minimal and list it under "Outside territory" in your notes.

## Names (decided)
- Product: **Allternit Factory**, run by **Gizzi** (terminal) and **Allternit Desktop** (GUI). The engine binary is **`allternit-factory`** (internal, never typed by users).
- The four parts: `agents`, `orchestration`, `workflows`, `workspace` (CLI: `gizzi agents …`).
- Old names are being **removed, not aliased**: CommRails, `allternit-rails`, `allternit-commrails`, `rails`, `ao`, `ao-*`, "agent orchestrator". Don't add new user-facing text, commands, env vars or routes with old names. Internal code you haven't been asked to rename can keep them until the rename sweep (stream F4).
- Allternit object names stay: Bot, Thread (`standing` / `task`), Campaign, Template, node / `DagNode`, WIH, Gate, Wake, ledger, receipts, vault, Coordinator (Al).
- Env prefix `ALLTERNIT_FACTORY_*`. Route prefix `/api/factory`. Engine port `3011`, socket `~/.allternit/factory/factory.sock`.

## Engineering rules (the determinism contract, SPEC §6)
One writer (Gate) · state is derived (rebuildable projections) · registry reconciled with reality · `--dry-run` on every mutation · idempotent IDs · `--json` everywhere and the exit-code table in API.md · no silent fallbacks · nothing runs unasked · proof comes from judge/receipts, never narrative · bounded loops.

## Machine rules
- **Rust:** `export CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.factory-target`, which all factory streams share. `~/.cargo/config.toml` caps jobs and rustc processes machine-wide. Don't override `RUSTC_WRAPPER` or `CARGO_BUILD_JOBS`, and never `pkill` cargo/rustc. Prefer `cargo check -p <crate>` while iterating. Run `cargo test -p <crate>` for what you touched before committing.
- **allternit-api changes:** smoke-boot the real binary on a fresh database (empty `ALLTERNIT_DB_PATH` / data dir) and hit one of your routes before opening the PR. Check route overlap, since a duplicate route crash-looped prod on 2026-09-30.
- **Migrations:** use the next free number above the highest on `origin/main` **and** `origin/factory/integration` at the time you open the PR (other teams add migrations too). Never run anything against a production database.
- **JS:** `pnpm install --frozen-lockfile` in your worktree if `node_modules` is missing. Run the package's typecheck plus the vitest suites you touched.
- **Disk:** check `df -h /System/Volumes/Data` before big builds. When you finish, delete your worktree's `node_modules/`, `dist/` and any local `target/` (but not `.factory-target`).
- **GitHub Actions:** don't add new workflows or jobs (private-repo minutes are capped).

## UI rules (allternit-ai streams)
- Every UI change ships on all three surfaces in the same PR: Desktop, the ai.allternit.com phone layout (`isPhoneViewport`, PhoneShell, phone.css), and the m.allternit.com PWA (coarse pointer).
- Use existing design tokens. New surfaces are white + `--neutral-fill`, never tan `--bg-primary` / `--surface-panel` / warm hovers. Handle light and dark, keyboard focus, reduced motion, and empty / loading / error states.
- For thinking/loading/streaming/avatar states, read `/Users/joe/.agents/skills/libraries-dev/SKILL.md` first.
- Types for the Factory come from `src/lib/factory/types.ts`. Don't redefine them. If the contract is missing something, add it in your own file and record it in your notes under "Contract change requested".

## Docs
Stream F1 owns `surfaces/docs/` (Mintlify) in the platform repo. Other streams: put every user-facing or dev-facing fact your change needs documented into your notes under "Docs needed". F1 picks those up. If you're a platform stream and your change has a natural doc page already in your territory, update it.

## Testing the pane engine (factory/pane)
- Run its tests with **cargo-nextest**: `cargo nextest run -p allternit-factory-pane`. Upstream (Herdr) runs one process per test, and many pane tests change process-wide environment variables (`XDG_*`, `HERDR_*`). Under plain `cargo test` they run as threads in one process and race, so a few different tests fail each run (detect::manifest, session stop timeouts, autodetect). Under nextest the whole suite passes (3059/3059 on 2026-10-06).
- Install: `curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ~/.cargo/bin` (Linux: `/latest/linux`).
- `cargo test -p allternit-factory-pane` is fine for a single test or one module, not for a pass/fail verdict on the whole crate.

## Reporting (how the orchestrator knows you're done)
- Keep `docs/FACTORY_<STREAM>_NOTES.md` in your worktree, **untracked: never commit it**. Write: what you did, files changed, how you verified it (exact commands + results), "Outside territory", "Contract change requested", "Docs needed", and anything left undone with the reason.
- Last step, after the PR is open: write `docs/FACTORY_<STREAM>.sentinel` containing `status: done` and `pr: <url>`, or `status: blocked` and `reason: <one line>`.
- Budget: aim to finish within about 250k tokens. If you hit a wall (a broken build you can't fix in your territory, or a design question), write the sentinel with `status: blocked` and stop. Don't wander.
