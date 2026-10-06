# Fix GitHub issue #6 — CI failing on `main`

Issue: https://github.com/Allternit/allternit-platform/issues/6

## Context

Confirmed before filing: `main`'s current tip fails CI independent of any recent PR content (checked via `gh api repos/Allternit/allternit-platform/commits/main/status` before merging PRs #2-#5, #7, #9 — same failures were already there). Two independent problems:

### 1. Three Vercel project deployments failing: `a2rchitech`, `allternit`, `platform`

**Investigate this first, don't assume "fix the deployment" is even correct.** This repo's `GIZZI.md` (root) documents the web surface's real, current deploy path as **GitHub push → CI pipeline → Cloudflare Pages** — no mention of Vercel anywhere in the documented architecture. The `Cloudflare Pages` check on these same PRs passes fine. This smells like the same class of problem found earlier this session with 8 orphaned Fly.io machines and an undesired Docker dependency that "crept into the codebase" — infrastructure nobody currently wants that's still wired up and silently costing/failing.

Steps:
1. `gh api repos/Allternit/allternit-platform/commits/main/status` to confirm current failure state, then `npx vercel inspect <deployment-id> --logs` (deployment ids available via `gh pr checks <any-open-PR-number>`) for each of the 3 failing projects to see the actual failure reason (build error? missing env var? expired token?).
2. Check for any `vercel.json`, `.vercel/project.json`, or Vercel GitHub App webhook configuration in the repo or its GitHub App settings that would explain why these 3 deployments even trigger on every push/PR.
3. Ask (don't assume): are these 3 Vercel projects still wanted? Check `git log` for when Vercel config was introduced and whether it predates or postdates the Cloudflare Pages setup GIZZI.md describes as current. If they look vestigial (superseded by Cloudflare Pages, not referenced by any current deploy documentation, failing for a config/auth reason rather than a real build regression), the correct fix may be **removing the Vercel integration** (unlink the GitHub repo from those 3 Vercel projects, remove any workflow/config triggering them) rather than debugging a deployment nobody uses. If they turn out to be real and wanted (e.g. `platform` serves something Cloudflare Pages doesn't), fix the actual failure instead.
4. Document which path you took and why in the notes file — this is a judgment call the issue explicitly asks you to make with evidence, not guess.

### 2. `validate-typography` GitHub Action failing

`.github/workflows/typography-validation.yml` runs `python scripts/validate-typography.py`, which scans `apps/ src/ components/ pages/ app/ surfaces/ packages/ styles/` recursively for raw `font-family:`/`fontFamily:` usage (and a few hardcoded font names) not going through the approved design-token list (`ALLOWED_TOKENS` in the script). Current failures are all under `packages/@allternit/ix` and `packages/@allternit/plugin-sdk` — e.g. `packages/@allternit/ix/src/pipeline/llm-to-ix.ts:834`, `packages/@allternit/plugin-sdk/docs-site/index.html:10,39,107`, `packages/@allternit/plugin-sdk/src/adapters/vscode/index.ts:163`, `packages/@allternit/plugin-sdk/website/src/pages/index.module.css:151`.

**Also worth noting before fixing**: there appear to be two near-duplicate copies of both the workflow and the script — one at repo root (`.github/workflows/typography-validation.yml`, `scripts/validate-typography.py`) and one nested under `surfaces/ai.allternit.com/.github/workflows/typography-validation.yml` / `surfaces/ai.allternit.com/scripts/validate-typography.py`. Check whether both actually run (nested `.github/workflows` inside a subdirectory typically does NOT get picked up by GitHub Actions unless something special is configured — verify this rather than assume), and whether they've drifted from each other. Consolidate or clarify if one is dead weight.

Steps:
1. Read `scripts/validate-typography.py` in full, and the actual violating lines it's flagging.
2. Decide per-violation: is `packages/@allternit/ix` (an internal pipeline package, not one of the four product surfaces) and `packages/@allternit/plugin-sdk` (a developer-facing SDK with its own docs-site/VS Code extension/website, arguably legitimately exempt from the *product's* typography system) actually meant to be covered by this validator? If not, add a scoped exclusion (e.g. exclude `packages/@allternit/plugin-sdk/docs-site`, `packages/@allternit/plugin-sdk/website`, `packages/@allternit/plugin-sdk/src/adapters` — these are tooling/docs, not product UI) rather than force-fitting design tokens into unrelated tooling. If they genuinely should comply, fix the actual `font-family` usages to use the `--font-allternit-*` tokens.
3. Get the workflow green on a real PR (open one against a throwaway branch or note how you verified, since this environment may not be able to run GitHub Actions locally).

## Constraints

- This is unrelated to the surface-audit implementation work happening in parallel elsewhere in this repo (other worktrees/branches) — do not touch anything under `surfaces/allternit-mobile/ios/`, `cmd/gizzi-code/`, or `docs/SURFACE_AUDIT*`, `docs/CHANGESET_REVIEW*`, `docs/AUTOMATION_TASKS*`.
- No destructive actions without justification: if removing the Vercel integration, explain exactly what "removing" means (unlinking a GitHub App connection is done via Vercel's dashboard/API, not by deleting files — if it's not something you can do from this checkout, document precisely what a human needs to click/run and why, rather than leaving it half-done).
- Real evidence over assumption throughout — this whole task exists because two speculative options were identified but not resolved; resolve them with actual investigation.

## Deliverable

`docs/learnings/CI_ISSUE_6_NOTES.md`, YAML frontmatter (`status`, `files_changed`, `deviations`, `remaining`), then prose: what you found in the Vercel logs, the fix-vs-remove decision and why, what you found about the duplicate typography workflow, and the resolution. That file existing = done.
