# Agent Work Attestation — GitHub production cleanup and move to the Allternit org

**Date:** 2026-10-06 03:00  
**Session ID:** github-cleanup (Claude Code session_01BqpmfCdfhrgGLUteWWHeMT)  
**Branch:** chore/repo-hygiene-2026-10-06, chore/allternit-org-references  
**Agent:** Claude Code (Opus 5.5)  
**Commit:** PR #1322 (`115812e28`), PR #1323 (`5ba12270e`)  
**Ledger entry:** [../LEDGER.md](../LEDGER.md)

## What was done

- **Org move.** Eoj renamed the `A2rchitech` org to `Allternit`. Transferred allternit-platform, allternit-ai, allternit-websites, desktop, gizzi-code, allternit-tts, homebrew-tap, scoop-bucket, allternit-brain and allternit-series from the `Gizziio` user to the org. History, PRs, releases and branches moved with them. Forks, empty repos (AllternitOS, allternit-facility), the stale satellite copies (allternit-sdk, allternit-api-client, allternit-docs, gizzi-code-docs, allternit-assets) and the old ops repos (allternit-ops, allternit-agent-ops) stay on the personal `Gizziio` account.
- **References.** #1323 here, allternit-ai #432, allternit-websites #52, and direct commits to homebrew-tap, scoop-bucket, desktop, gizzi-code, allternit-tts: `Gizziio/<repo>` → `Allternit/<repo>`, brew tap `allternit/tap`, scoop bucket `allternit`. Hosted runtime image → `ghcr.io/allternit/allternit-hosted-runtime`, because an org repo's workflow token cannot push to the personal account's GHCR namespace. iOS settings copy "Gizziio Code" → "Gizzi Code". `agent-ledger/` and `docs/archive/` left unchanged as history.
- **Installer bug.** `publish-allternit-bot.yml` recreates `allternit-bot-latest` on each run, which made it the repo's Latest release. The gizzi installer and self-update read `releases/latest`, so fresh installs resolved that tag as a version and failed. Latest was moved back to `gizzi-code/v2.1.9` by hand; #1322 adds `--latest=false`.
- **Root cleanup (#1322).** Root bot e2e scripts → `scripts/e2e/bot/`; academy JSON → `docs/research/data/`; committed `.logs/`, `.pids/`, `tmp/` removed and gitignored; five dead `.mcp.json` servers removed; REPO_STRUCTURE satellite table rewritten.
- **Branches.** allternit-platform 526 → 29, allternit-ai 382 → 11, allternit-websites 44 → 1. Deleted: branches whose PR merged at the same tip, branches whose PR was closed at the same tip, and branches with no commits outside main. Kept: open PRs, joe-52's worktree keep list, and in-progress branches. Four stale unmerged branches (session/openmaus-botmode-0915, session/tier-a-classifier, session/laya-finetune, wip/cloud-api-quota-oauth-0915) were saved to `Allternit LLC/archive/git-bundles/allternit-platform-stale-wip-2026-10-06.bundle` (verified) before deletion.
- **Releases and tags.** Removed the draft VM images release, v1.1.0, v1.2.10, v1.2.11, gizzi-code releases older than 2.1.0 and the `gizzi-code/2.0.1` duplicate, and 42 `archive/2026-09-13/*` tags. `desktop-*` tags untouched per the release rules.
- **Org profile.** Created `Allternit/.github` with `profile/README.md` and an org-wide `SECURITY.md`.
- Closed the stale winget submission microsoft/winget-pkgs#429931 (GizziCode 2.0.2).

## Verification

- trufflehog (bare-repo mode, all 13 repos, full history): no verified live secrets. gitleaks on this repo: placeholders, plus a Stripe live key in `surfaces/allternit-platform/DEPLOYMENT_SECRETS.md` history (commits 3b2c4cdd, 10171ac1; Eoj is handling it) and the private key of the archived, undeployed link-card service. Eoj chose not to rewrite history.
- Old Gizziio URLs redirect after the transfer: Desktop `releases/latest`, the platform `releases/latest` API (returns gizzi-code/v2.1.9), a gizzi release asset, the hosted-runtime binary, and the brew tap git remote.
- `node scripts/release-preflight.mjs`: 56 passed, 0 failed. `surfaces/docs/scripts/check_links.py`: 0 problems.

## Known gaps / remaining work

- Cloudflare Pages project `ai-allternit` is git-connected to `allternit-ai`. The Cloudflare GitHub app must be installed on the Allternit org (Eoj, web UI) or ai.allternit.com auto-deploys stop.
- The org description still ends mid-sentence ("…executable intelligence. I"). Editing it needs `admin:org`, which this token does not have.
- The hosted runtime image is first published to `ghcr.io/allternit/...` by the workflow run that #1323's merge triggers. Check that run, and set the new package's visibility if it must be public.
- Local clones and worktrees still have `Gizziio/...` remotes. Pushes work through the redirect; update with `git remote set-url` when convenient.
