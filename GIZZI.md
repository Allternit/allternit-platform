# Allternit monorepo — agent orientation

This is the root instructions file for any AI agent working in this repo
(gizzi-code, Claude Code, or otherwise). Read this before starting
user-facing feature work.

## Surfaces — read this before adding any user-facing feature

Allternit ships the same product to **four** places, not three — it's easy
to forget gizzi-code counts as a surface because it doesn't look like one.
**Adding a feature to only one of these is the single most common
cross-repo mistake in this codebase.** Check all four before calling a
feature "done":

| Surface | Path | Relationship | Ships via |
|---|---|---|---|
| **Web** | `surfaces/ai.allternit.com` | Source of truth for the platform UI. Vite + React SPA. | GitHub push → CI pipeline → Cloudflare Pages. **Merging code does not make it live** — it lags until that pipeline actually runs, and today it can *also* lag because the deployed Pages project's own environment/secrets config can silently drift from what's in the repo (see Backend architecture below — this bit us for real on 2026-08-01, production Clerk was off for an unknown period because of exactly this). |
| **Desktop** | `surfaces/allternit-desktop` | Electron wrapper around the *same* web code, not a separate UI. Two distinct modes: **dev** (`NODE_ENV=development`, Electron loads a live web dev server on `localhost:3013`, see `src/main/config.ts`'s `PORTS.DEV_UI`) and **packaged** (bundles a prebuilt copy of the web build via the `prepare:platform-static` script into `resources/platform/`). A web change reaches packaged desktop only after someone reruns that packaging + a new release build — it is not automatic just because the web PR merged. | Its own separate packaging/release build, not the web CI pipeline. |
| **iOS** | `surfaces/allternit-mobile/ios` | Fully separate native Swift/SwiftUI app. Same backend contracts, same Clerk identity as web/desktop, **zero shared UI code**. Conceptually it should have feature parity with web/desktop — that parity has to be built by hand every time, it does not happen automatically or by virtue of "using the same backend." | TestFlight / App Store, its own release cadence. |
| **gizzi-code ("the brain")** | `cmd/gizzi-code` | The CLI/agent harness. Two roles at once: (1) a real surface technical users drive directly from a terminal, and (2) the backing engine the *other* surfaces bridge into for agent capabilities (e.g. iOS's agent-sessions bridge talks to a running gizzi-code instance remotely). A capability added here (a skill, a tool, a command) does not automatically get a UI anywhere else, and a platform feature (chat, canvas, artifacts) does not automatically get a CLI-side equivalent. These two directions silently silo from each other unless someone deliberately wires the bridge both ways — the HTML-artifacts work on 2026-08-01 is the one concrete example so far of doing this deliberately (gizzi-code generates → publishes to the same canvas system the platform already reads). | `gizzi` binary / npm package, own versioning. |

**Practical rule: web + desktop are effectively one codebase, gated on a
packaging/deploy step someone has to actually run** — not automatic just
because code merged. **iOS is a second, independent codebase that needs
every feature built again, natively, by hand.** **gizzi-code is a third,
independent codebase/product that can silently gain capabilities the other
three never surface, or vice versa**, unless the bridge between them is
part of the feature's own scope.

### The feature-scoping gate

Before starting *any* feature (not just "user-facing" ones — a gizzi-code
skill counts), answer this explicitly, in the plan/spec, before writing
code — don't leave it implicit:

1. **What is this, fundamentally — a capability or a UI affordance?**
   A capability (something an agent/CLI can *do*) belongs in `cmd/gizzi-code`
   first. A UI affordance (something a human clicks/taps) belongs in the
   platform surfaces.
2. **If it's a capability**: should any platform surface expose a way to
   trigger or view it (a chat command, a button, a rendered result)? Default
   to "yes, at least view it" unless there's a specific reason a human never
   needs to see it.
3. **If it's a UI affordance**: does it need to exist in web, desktop, *and*
   iOS, or is it genuinely platform-specific (e.g. a native share sheet is
   inherently iOS-only)? Default to "all three" — treat any surface you're
   *not* building it for as an explicit, named exception with a reason, not
   a silent omission.
4. **Record the answer** in the PR/task description as an actual checklist
   (`- [ ] web  - [ ] desktop  - [ ] iOS  - [ ] gizzi-code`, strike through
   what's genuinely out of scope and say why) so a reviewer — human or
   agent — can see the scoping decision was made on purpose, not forgotten.

All four surfaces authenticate against the **same Clerk application**
(`Gizziio`, one publishable key, verified byte-identical across web/desktop/
iOS builds as of 2026-08-01) — identity is unified even when the UI code
and capability set aren't.

## Backend architecture

- `cmd/allternit-api` — the backend each *device* actually talks to day to
  day. Desktop spawns/expects its own local instance
  (`VITE_ALLTERNIT_API_URL=http://127.0.0.1:8013` baked in at desktop build
  time); a paired iOS device reaches a real instance (local or tunneled)
  the same way. It is **not** a centrally-hosted multi-tenant service — it
  is not even a deployed Fly/Railway app. Local-first, per-device SQLite.
- `cmd/allternit-cloud-api` — the actual centrally-hosted service, deployed
  in production on the Contabo VPS (`mail`, `api.allternit.com`) per
  `docs/Operations/CLOUD_API_VPS_DEPLOY.md` (the old Fly.io deployment is
  stale). This is where cross-device tenancy really lives
  (`users.tenant_id`/`role`/`status`, `api_tokens`, `user_sessions`,
  `audit_log` — see `migrations_pg/001_initial.sql`), device-token verification,
  hosted-runtime management, and Clerk webhook sync
  (`src/routes/clerk_webhooks.rs`, added 2026-08-01).
- **The deployed hosted web build (`ai.allternit.com`) currently has no
  backend behind it at all.** `CompanyConfigProvider`
  (`surfaces/ai.allternit.com/src/providers/company-config-provider.tsx`)
  used to call `/api/onboarding/config` unconditionally on every load, which
  always 404'd/fell through to the SPA shell there — fixed 2026-08-01 by
  gating that call behind `isSelfHosted()` (see `src/lib/env.ts`), since
  that endpoint is genuinely desktop/self-hosted-only (local Ollama/LM
  Studio discovery, OS keychain — concepts that don't exist for a hosted
  multi-tenant site).

## Local dev gotchas (hit and fixed 2026-08-01, will recur in fresh checkouts/worktrees)

- **`surfaces/ai.allternit.com` env vars**: Vite only exposes vars prefixed
  `VITE_` or `NEXT_PUBLIC_` to the browser bundle (`envPrefix` in
  `vite.config.ts`, deliberate — "never broaden this to arbitrary process
  environment variables"). A bare `ALLTERNIT_SELF_HOSTED=1` in `.env.local`
  silently does nothing in the browser. Use
  `VITE_ALLTERNIT_PLATFORM_DISABLE_CLERK=1` /
  `NEXT_PUBLIC_ALLTERNIT_PLATFORM_DISABLE_CLERK=1` to bypass Clerk for local
  testing instead.
- **`autoprefixer` is used by `postcss.config.cjs` but not declared** in
  `surfaces/ai.allternit.com/package.json` — fresh installs need it added
  manually or the dev server serves a permanent PostCSS error overlay (HTTP
  200 on the shell, but the page never actually renders — easy to
  misdiagnose as a backend problem).
- **iOS builds need `Frameworks/Mesh.xcframework`**, which is
  gitignored/not checked in — copy it from a working checkout
  (`surfaces/allternit-mobile/ios/Frameworks/`) before building in a fresh
  worktree.
- **iOS defaults to "Allternit Cloud" pairing mode**, not local — pass
  `ALLTERNIT_API_BASE_URL=http://127.0.0.1:8013/api/v1` at `xcodebuild`
  time (build-time, not a launch arg) to point a simulator build at a local
  backend directly.
- **This sandboxed dev environment cannot download large binaries** —
  Electron's postinstall (`cmd/allternit-desktop`) and Playwright's
  Chromium download both silently truncate (exit 0, but the resulting
  `dist/` is a few hundred KB instead of 100MB+). Not a code bug; don't
  spend time debugging it here, it needs a real (non-sandboxed) machine.
- **gizzi-code's own CLI entrypoint** (`bin/gizzi` / `cmd/gizzi-code`) has
  pre-existing undeclared dependencies (`axios`, `figures`, `fuse.js`,
  `lru-cache` resolving to the wrong major version, the
  `@opentelemetry/*` set, `sharp`, `xss`, `yaml`) that block it booting in a
  fresh checkout — fixed once already (`cmd/gizzi-code/package.json`), but
  watch for the same class of issue if new imports get added without a
  matching `package.json` entry, since pnpm hoisting can silently paper
  over it in some checkouts and not others.

## Steering / pipeline

This repo runs its own autonomous `.steering` automation
(spec-checker loop, discovery/build queue) and has a documented history of
**multiple concurrent Eoj/agent sessions editing the same checkout at
once**. Before editing any file, check `git status`/`git diff main` against
what you actually intend to touch, not just what you remember touching —
`main` moving out from under a long-running session mid-work is a known,
recurring, benign event here (resolve with a fast-forward merge, not by
treating it as your own scope creep).

## Living dependency map — feature impact

Before touching a feature, read [docs/dependency-map/README.md](docs/dependency-map/README.md). Run `python3.11 scripts/dependency-map.py --impact <changed paths> --summary` to see the products, features, user journeys and shipped surfaces a change can reach, and put them in your plan (drop `--summary` for the full JSON list). The query is cached against both repos' state, so it is fast after the first run. `python3.11 scripts/dependency-map.py --brief` prints every feature with its status, the user journeys, and the recent product decisions. If you add, remove or move a product, a runtime call, a sidecar or a shipping path, update `docs/dependency-map/products.json` or `runtime-links.json` in the same PR. If you add a feature, change its status or the components it touches, change a user journey, or make a product decision, update `docs/dependency-map/features.json` in the same PR (decisions are dated). Run `--validate` before finishing. Never commit generated map output (graph.json, index.html, dist/admin-site): it lists the private allternit-ai repo and this repo is public. admin.allternit.com rebuilds itself after merge.
