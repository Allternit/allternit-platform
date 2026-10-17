# Session: bonsai-local removal + gateway embeddings proxy (2026-10-16)

## What landed

- **#1559** `feat(gateway): proxy /v1/embeddings to a configured provider` — the LLM gateway's `/v1/embeddings` route now proxies to any operator-configured OpenAI-shaped embeddings backend via `ALLTERNIT_GATEWAY_EMBED_URL` (full base URL incl. `/v1`). Unset keeps the historical safe default `501 allternit.not_configured`; hash-fake embeddings remain fenced behind `ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS=1`. `user` is never forwarded; 60s timeout; upstream failure → `502 allternit.upstream_error`. Docs (`surfaces/docs/api/embeddings.mdx`) rewritten to match with operator-neutral examples; `feature:gateway-embeddings-proxy` added to the dependency map.
- **#1571** `feat(gizzi-code,desktop)!: remove bonsai-local local-model feature` (Eoj: no local model ships with stock production) — deleted the `bonsai-local` media driver + registry entry, `services/bonsai-local/`, the Desktop `bonsai-companion-manager` and all its IPC wiring; removed `bonsai` from the local-family taxonomy heuristics. Release-preflight 55/0. Pairs with **allternit-ai#582**, which removed the UI surface (`bonsai-webgpu`/`bonsai-owned-webgpu` providers, image-plugin wiring; **the image plugin's `defaultProvider` changes from `'bonsai-local'` to `'gpt-image'`**, so stock no longer defaults users' image generation to a local engine) with a clean `tsc --noEmit`.
- **#1578** `ci: fix post-merge gitleaks + docs-lint failures` — allowlisted the memory-drive ledger test fixture (`API_TOKEN=abcdef123456` feeds the `assigns_secret()` canary; triaged false positive, same class as the 2026-09-03 entries); reworded the new embeddings.mdx lines to drop the competitor name; allowlisted `core/skills.mdx` (documents real cross-agent skill directories, same class as the existing `cli/native-sessions.mdx` exception).

## Verification

- `cargo test -p allternit-api --lib embeddings` — 11/11.
- Live smoke on :18013 (scratch data dir): unconfigured → 501; configured → real 768-dim vectors with sane cosine ordering; dead upstream → 502.
- `pnpm docs:lint` — green (also run inside the CI-fix PR).
- `node scripts/release-preflight.mjs` — 55/0 after #1571.
- `pnpm exec tsc --noEmit` in allternit-ai — clean after #582.

## Incidents (honest record)

- The 9ba3c18ee1 "docs" commit on the #1559 branch accidentally carried a block of restaged origin/main files (an earlier `git checkout origin/main -- .` audit step left them in the index; the commit picked them up). Content impact: none (duplicates of identical incoming content), but it produced AA merge conflicts for `ledger.rs`/`ledger_repo.rs`/`netops/hops.rs` (resolved to main's version) and is what surfaced the gitleaks fixture finding in the scan range. Root cause noted for future sessions: after `git checkout <commit> -- .`, run `git reset` (mixed) before staging anything else.
- Merge of #1559 auto-fired while two non-required checks were still running; they failed on main (gitleaks false positive + docs-lint) and were fixed by #1578. No user-facing breakage at any point.

## Deferred / follow-ups

- **Decisions-layer code follow-ups** (from the same day's local-stack work, not part of these PRs): a chat/completions-shaped decision backend so the oracle can hit a local gateway directly instead of gizzi's ~46K-token session packs (75s cold / ~46s warm on an M1 Pro); a task-intake classifier hook at agency `create_run`; `model-routing.json` A:// low-tier mapping to a local 4B remains an Eoj-gated policy change.
- **Desktop rebuild from merged main** (lifecycle step 8) not done in-session; no bonsai companion is installed on the owner machine, so the installed build has nothing to auto-start. Rebuild ships the removal into the installed app.
- **Shared checkout sync skipped**: the shared checkout is being actively edited by concurrent sessions (dirty `.steering/checkpoint.md` stashed as `stash@{0}` "stale steering checkpoint pre-main-sync 2026-10-16"; dirty `.steering/git-discipline-allowlist` in flight). It is 173 behind origin/main; any owning session should `git pull --ff-only` once the tree settles.
- **Prisma schema** still has a `bonsai` enum value (2 hits in the generated client) — needs a DB migration to drop; deliberately not in these PRs.
- Generic BYO local-model infrastructure (ollama/LM Studio under `src/lib/local-models/`) intentionally left in place — separate product-scope decision by Eoj.
- `launchctl setenv` decisions config on the owner machine (oracle model, thresholds, embed URL, `ALLTERNIT_AGENCY_EXECUTE=1`) persists until reboot; re-set or bake into a desktop build.
