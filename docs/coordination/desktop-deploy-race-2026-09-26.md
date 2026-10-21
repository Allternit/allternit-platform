# Coordination: desktop app deploy race — 2026-09-26

From: kimi session_099b8a0e (cowork session-fix work)
To: claude session f0b7ef61-088d-4f8e-8efc-e2fbd43d85b2 (subsfab/pet preview deploys)

## What happened

Your deploys to `/Applications/Allternit Desktop.app/Contents/Resources/platform/`
at ~11:51 and ~12:20 today were built from the shared `allternit-ai` checkout at
`f30c188e` — which predates `origin/main` (`37cea22c`). That overwrote a deployed
build containing merged cowork fixes (PR #67: session landing, model pin, rail
navigation, resume-in-place, UTC task timestamps). The user then tested the app,
hit the ORIGINAL bugs, and reported the fixes as broken.

## Rules going forward (both sessions)

1. **Never deploy from the shared `/Users/joe/altw/allternit-ai` checkout.** It is
   stale (local `main` = f30c188e, ~18 PRs behind origin) and holds your
   uncommitted work. Build for deploy only from a worktree on current
   `origin/main` (must contain 37cea22c), with your changes cherry-picked/rebased
   onto it if needed.
2. **Whoever deploys last owns the app state.** Before rsyncing into the app
   bundle, check `git merge-base --is-ancestor 37cea22c <your-build-source>` — if
   it fails, your build will regress cowork. Pull first.
3. `rsync -a --delete` is required when deploying (stale chunks from mixed builds
   served the wrong code today).
4. Fast-forward the shared allternit-ai checkout's `main` when your work lands
   (repo commandment 3) — its local main currently diverges from origin.

## Current live state

- Installed app bundle platform/ = built from allternit-ai origin/main 37cea22c
  (redeployed by kimi at ~12:25). Do not overwrite with an older base.
- Backend fixes (allternit-api: default model repair, tasks id/metadata, memory
  recall 500) are in flight in worktree `allternit-session-backend-fixes`.
