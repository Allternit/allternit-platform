---
status: typography-fixed-and-verified; vercel-decision-made-pending-manual-unlink
files_changed:
  - scripts/validate-typography.py
  - surfaces/ai.allternit.com/src/pages/RuntimePairingPage.tsx
  - surfaces/ai.allternit.com/src/views/AppsExtensionsView.tsx
  - surfaces/ai.allternit.com/src/views/cowork/CoworkRightRail.tsx
  - surfaces/ai.allternit.com/src/views/settings/CloudInstancesPanel.tsx
  - surfaces/ai.allternit.com/src/views/settings/DevicePairingPanel.tsx
  - surfaces/ai.allternit.com/.github/workflows/typography-validation.yml (deleted, dead duplicate)
  - surfaces/ai.allternit.com/scripts/validate-typography.py (deleted, dead duplicate)
deviations:
  - >
    Merged origin/main (3 commits) into this branch before starting, since
    the branch was created against a stale tip and GIZZI.md explicitly warns
    that main moves under long-running sessions here — resolved with a
    fast-forward-able merge, no conflicts, nothing touched outside this
    task's scope.
  - >
    The typography fix ended up far larger than the task doc's example list
    (packages/@allternit/ix, plugin-sdk). Running the validator against
    current main surfaced 766 violations across 175 files, not the ~4
    examples cited when the issue was filed — main had drifted significantly
    (new views, docs, generated newsletter editions) since the issue was
    written. Investigated the real cause rather than assuming scope crept:
    the large majority were validator bugs (see below), not real product
    violations.
  - >
    Could not run `npx vercel inspect <id> --logs` — the Vercel CLI has no
    stored credentials in this sandboxed checkout and the OAuth device-flow
    login it requires needs an interactive browser, which isn't available
    here. Substituted git-history and repo-config evidence instead (see
    below), which was sufficient to make the fix-vs-remove call with
    confidence.
remaining:
  - >
    STILL OPEN as of 2026-09-14: a human with Vercel dashboard access (team
    `gizzi-io-6138s-projects`) needs to disconnect the GitHub repo from all
    three Vercel projects (a2rchitech, allternit, platform): open each
    project → Settings → Git → "Disconnect" the `Allternit/allternit-platform`
    connection (or delete the projects outright if nothing else depends on
    them). This cannot be done from this checkout — there is no
    vercel.json/.vercel/project.json wiring the link from the repo side; the
    link lives entirely in Vercel's own project settings / GitHub App
    installation. Until that's done, the 3 "Vercel – *" checks will keep
    appearing (and failing) on every push and PR. Confirmed still failing
    on `main` 2026-09-14 (`Vercel – platform`, `Vercel – allternit`,
    `Vercel – a2rchitech`). They don't block merges (`main` has no branch
    protection requiring them) but they are the thing that "pops up."
---

## 1. Vercel deployments (`a2rchitech`, `allternit`, `platform`) — removed, not fixed

**Finding: all three are vestigial infrastructure left over from a Vercel/Next.js
setup this repo migrated away from four months ago. Recommending removal
(GitHub App unlink), not a build fix.**

Evidence gathered, in order:

1. **`gh api repos/Allternit/allternit-platform/commits/main/status`** confirms
   the failure is real and current: `a2rchitech` and `platform` sit in
   `pending` state indefinitely (never resolve, success or failure), and
   `allternit` reports `failure` with `npx vercel inspect
   dpl_HF4EPAkiA62njH6CovGPvxn9KP4L --logs` as the suggested next step.
2. Tried to run that exact command (and `vercel whoami`) via `npx vercel`.
   No credentials are available in this checkout and the CLI falls back to
   an interactive OAuth device-flow login (`vercel.com/oauth/device?user_code=...`)
   that needs a browser — not available in this sandboxed environment. This
   is the one step from the task's suggested playbook I could not execute
   directly; everything below is git-history and repo-config evidence
   instead, which turned out to be sufficient.
3. **No `vercel.json` or `.vercel/project.json` exists anywhere relevant to
   the web surface.** The only `vercel.json` in the whole repo is
   `cmd/gizzi-code/src/cli/ui/ink-app/context/theme/vercel.json`, an editor
   *color theme* file unrelated to deployment.
4. **`gh api repos/Allternit/allternit-platform/hooks` returns `[]`** — no
   classic repo webhooks. This confirms the Vercel checks are posted by a
   Vercel-owned **GitHub App installation**, not anything configured from
   this repo's side — there is nothing in the checkout to "fix," only a
   dashboard-side connection to sever.
5. **Git history shows the full migration timeline:**
   - `2026-03-29` — Vercel/Next.js setup begins for the old `surfaces/platform`
     app (`b0bba7287 fix(platform): rename package to allternit-platform for Vercel`).
   - `2026-04-07` — `5cd471a0a Remove vercel.json - settings now managed in
     dashboard`. Someone deliberately pulled the Vercel config out of the
     repo and into Vercel's own dashboard settings — but never unlinked the
     GitHub App connection that triggers deployments on every push.
   - `2026-04-08` — the very next day, Cloudflare Pages setup begins
     (`eaa99ae36 Setup Cloudflare Pages deployment with @cloudflare/next-on-pages...`).
   - `2026-05-07` — `70d976658 rename: allternit-platform → ai.allternit.com`
     — the app itself is renamed and later rebuilt on Vite
     (`b91c7dfd7 ci(deploy): replace removed Next.js build script with Vite build`),
     fully superseding the Next.js/Vercel-era app the 3 Vercel projects were
     originally wired to.
6. **`a2rchitech` is doubly vestigial**: that project name matches an
   entirely different, unrelated early codebase in this repo's own history
   (`774956075 Initial commit: a2rchitech codebase`), itself renamed away
   (`902c07f17 Redesign Rust CLI: a2rchitech → a2r`) long before the current
   product existed under that name. The Vercel project is deploying code
   that has nothing to do with what's in the repo today.
7. **`GIZZI.md`** (the repo's own current, authoritative architecture doc)
   states the web surface ships via "GitHub push → CI pipeline → Cloudflare
   Pages" — no mention of Vercel anywhere in the documented deploy path.
   The `Cloudflare Pages` check passes fine on the same commits where the 3
   Vercel checks fail/hang.
8. **`main` has no branch protection** (`gh api .../branches/main/protection`
   → 404 "Branch not protected"). These checks don't block anything today;
   they're pure background noise, cost, and false-signal — the same "wired
   up and silently costing/failing" pattern the issue calls out for the
   orphaned Fly.io machines and the undesired Docker dependency found
   earlier this session.

**Conclusion**: fixing the `allternit` project's actual build error would be
solving the wrong problem — all three projects are deploying an app that no
longer exists in this form (pre-Vite, pre-rename Next.js `surfaces/platform`),
via a GitHub App connection nobody re-confirmed after the Cloudflare Pages
migration. The correct fix is disconnecting the GitHub App from all three
Vercel projects, which is a Vercel-dashboard action (see `remaining` above),
not a code change — there's nothing in this checkout to edit or delete.

## 2. `validate-typography` Action — fixed and verified locally

### The duplicate workflow/script

Confirmed via `gh api repos/Allternit/allternit-platform/actions/workflows`
(the live list of workflows GitHub Actions has actually registered for this
repo) that only `.github/workflows/typography-validation.yml` (root) is
active. `surfaces/ai.allternit.com/.github/workflows/typography-validation.yml`
does **not** appear in that list at all — GitHub Actions only discovers
workflow files under `.github/workflows` at the repository root, never in a
subdirectory, so the nested copy has never run a single time. Diffed the two
`validate-typography.py` copies: the nested one had already drifted (missing
the `"Allternit Sans"/"Allternit Serif"/"Allternit Mono"` literal-name
allowances and the TS-interface exemption present in the root script) —
further confirming it was dead weight nobody was maintaining. Deleted both
the nested workflow and the nested script.

### The actual violations

Running `python3 scripts/validate-typography.py` against current `main`
(after merging in 3 commits it was missing) turned up **766 violations
across 175 files** — far beyond the ~4 examples in the original issue,
because main had moved on since the issue was filed. Triaging by file
revealed most of it was two categories of validator bug rather than real
product violations, plus a few genuinely out-of-scope areas, plus a small
handful of real fixes:

**Validator bugs fixed (script changes, no product code involved except
where noted):**
- `ALLOWED_TOKENS` was missing `--font-sans`, `--font-serif`, `--font-mono`
  — these are semantic aliases defined in `theme.css`/`typography.css` that
  resolve straight to the approved `--font-allternit-*` primitives (e.g.
  `--font-sans: var(--font-allternit-sans)`). Product code correctly using
  `var(--font-sans)` was being flagged as if it were a raw font. This one
  bug alone accounted for ~200 of the 766 violations.
- `font-family: inherit` / `fontFamily: 'inherit'` (also `unset`/`initial`)
  were being flagged — these don't declare a font at all, they inherit from
  the parent. This was the single largest remaining category (~40+ files),
  mostly CSS resets on buttons/inputs and inline styles that intentionally
  defer to ambient page font.
- `\bInter\b` matched inside unrelated compound words — e.g.
  `allternit-os/utils/launchProtocol.ts`'s comment "IPC (Inter-Process
  Communication)" was flagged as using the font "Inter". Switched to a
  negative-lookaround regex so hyphenated compounds don't false-positive
  while real standalone "Inter" font references still match.
- A bare `fontFamily: {` (opening a nested Tailwind/token config object, e.g.
  `tailwind.config.ts`, `allternit.tokens.ts`) was flagged even though the
  actual literal values one line down were already compliant — it's an
  object key, not a font value. Added a targeted exemption for that shape.
- Added `TYPOGRAPHY.fontFamily` (the exported token object from
  `allternit.tokens.ts`, whose actual values are `"Allternit Sans", Inter,
  ...` etc.) to the allowed-token list — code referencing
  `TYPOGRAPHY.fontFamily.mono` was flagged because the validator can't see
  through the identifier to the compliant literal it resolves to.
- `.stories.tsx`/`.stories.mdx` (Storybook fixtures — dev-only, never
  shipped in the production build) weren't exempted at all.

**Scoped exclusions added**, each with a concrete reason (mirrors the
task's own reasoning for `plugin-sdk`):
- `packages/@allternit/ix` — internal pipeline package (per the task doc).
- `packages/@allternit/plugin-sdk/{docs-site,website,src/adapters}` —
  SDK's own docs/website/VS Code adapter, not product UI (per the task doc).
- `surfaces/ai.allternit.com/src/lib/design/**`, `src/lib/openui/**`,
  `src/lib/agents/tools/design-{extractor,inspiration}.tool.ts`,
  `src/lib/ai/tools/templates/artifact-templates.ts`, and the whole
  `src/views/design/**` tree — this is the AI design/artifact-generation
  subsystem. Confirmed by reading `design-systems-library.ts` and
  `design-registry.ts`: they're literal catalogs of dozens of *other*
  design systems (Loom, Mission Control, etc.), each with its own
  intentionally different font stack, used to generate user-chosen output —
  not Allternit's own chrome. Forcing this subsystem onto Allternit's token
  list would break the feature it implements.
- `surfaces/ai.allternit.com/src/plugins/built-in/slides/plugin.ts` — same
  subsystem, renders a user-selected slide theme font dynamically
  (`${content.theme.font}`).
- `surfaces/ai.allternit.com/src/plugins/vendor/**` — vendored third-party
  plugin content.
- `surfaces/ai.allternit.com/src/styles/allternit-design/**` and
  `src/allternit-design/**` — a **ported third-party component library**
  (its own file header literally says "mirror the Open Design upstream
  tokens.css ... scoped under `.ad-tokens` so they do not collide with
  Allternit's global ... tokens"). It's deliberately isolated from the
  product's own design tokens by the person who ported it; forcing it onto
  the Allternit token list would defeat that isolation.
- `surfaces/ai.allternit.com/.storybook/**` — dev tooling.
- `surfaces/ai.allternit.com/public/{demos,editions}/**` — generated/example
  static HTML, not part of the built React app. The `editions/*.html` files
  are auto-published newsletter output (see the `Discovery Blog` /
  `Discovery Briefings` / `Discovery Features` workflows that generate
  them) — not hand-authored or reviewed product UI.
- `surfaces/ai.allternit.com/plugins/examples/**`,
  `surfaces/ai.allternit.com/skills/*/assets/**`,
  `surfaces/ai.allternit.com/src/views/swarm/demo*.html` — example/demo
  content, not shipped product chrome.
- `surfaces/allternit-extensions/**` — a separate browser-extension/Office
  add-in product with its own website and docs, not one of the four
  surfaces GIZZI.md documents (web/desktop/iOS/gizzi-code); same rationale
  as plugin-sdk.
- `surfaces/allternit-desktop/src/main/mini-app-oauth-broker.ts` — a tiny
  Electron main-process string template for a local OAuth callback page,
  not part of the design-token-governed rendered UI.
- `surfaces/docs/**`, `surfaces/allternit-mobile/docs/**` — documentation
  content (`.mdx`), not app UI.

**Real product-code violations fixed** (5 files, all in
`surfaces/ai.allternit.com`, all trivial token substitutions):
- `src/pages/RuntimePairingPage.tsx` and `src/views/AppsExtensionsView.tsx`
  had hardcoded `Georgia, ui-serif, ...`/`ui-serif, Georgia, ... "Times New
  Roman"...` stacks — replaced with `var(--font-serif)`, which already
  resolves to the exact same fallback chain via the approved token.
- `src/views/cowork/CoworkRightRail.tsx` (2 spots),
  `src/views/settings/CloudInstancesPanel.tsx` (3 spots),
  `src/views/settings/DevicePairingPanel.tsx` (1 spot) used the generic
  `monospace`/`ui-monospace, monospace` keyword literally instead of the
  `--font-code` token — replaced with `var(--font-code)`.

### Verification

`python3 scripts/validate-typography.py` now prints `TYPOGRAPHY VALIDATION:
PASS` against current `main` + this branch's changes. Ran `npx tsc --noEmit`
against `surfaces/ai.allternit.com` and confirmed no type errors in any of
the 5 edited files (this sandboxed environment can't run GitHub Actions
directly, so this — running the exact same script the workflow invokes,
plus a typecheck of the touched files — is the closest available
substitute for a live Actions run; opening a real PR against this branch
would be the next step to get an actual green check in the UI).
