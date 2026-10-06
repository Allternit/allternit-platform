# gizzi-code Dependency Audit — 2026-09-03

**Scope:** vulnerability remediation for the gizzi-code CLI ahead of the 2026-09-04 production deploy.
**Baseline:** `bun audit` in `cmd/gizzi-code` reported **86 vulnerabilities (1 critical, 30 high, 47 moderate, 8 low)** — but see "Two dependency graphs" below; that count came from a stale lockfile.

## Two dependency graphs (read this first)

`cmd/gizzi-code` lives inside the Allternit pnpm workspace, yet carries its own
`bun.lock` (`"packageManager": "bun@1.3.14"`). These were **two different
resolutions of two different package.json snapshots**:

- `cmd/gizzi-code/bun.lock` was **stale** — it predates the current package.json
  (missing the `@allternit/gizzi-util` / `@allternit/orchestrator` /
  `@allternit/request-scorer` workspace deps; still contained packages no longer
  present, e.g. `solid-js` / `seroval@1.5.0`). The 86-finding headline came from
  auditing that lock.
- The **actually installed and shipped** tree is the pnpm workspace install
  (`node_modules` → worktree-root `node_modules/.pnpm`, governed by the root
  `pnpm-lock.yaml`). `bun install` cannot run inside `cmd/gizzi-code` (the six
  `workspace:*` deps cannot resolve outside a bun workspace), so real upgrades
  were applied with pnpm and `bun.lock` was regenerated from the final
  package.json (via a scratch workspace) so `bun audit` reflects reality.

All affected/fixed version determinations were verified per installed version
against the OSV API, not just advisory headlines.

## Fixed in the installed/shipped tree (pnpm, `node_modules/.pnpm`)

### Direct dependencies (`cmd/gizzi-code/package.json`)

| Package | From | To | Advisories resolved |
|---|---|---|---|
| `@modelcontextprotocol/sdk` | 1.25.2 | 1.29.0 | GHSA-345p-7cg4-v4c7 (high, cross-client data leak; fix floor 1.26.0). Also dedupes with the copies pulled by `@upstash/context7-mcp` and `@modelcontextprotocol/server-sequential-thinking` |
| `@modelcontextprotocol/sdk` → `@modelcontextprotocol/client`/`server`/`core` | 1.29.0 | 2.3.1 | 2026-10-05: moved to the SDK v2 split packages (MCP 2026-07-28 + legacy fallback). gizzi no longer depends on the v1 package; the bundled stdio servers (`@upstash/context7-mcp`, `server-sequential-thinking`) still pull their own v1 copy |
| `@babel/core` (dev) | 7.28.4 | 7.29.7 | GHSA-4x5r-pxfx-6jf8 (low) |
| `minimatch` | 10.0.3 | 10.2.6 | GHSA-3ppc-4f35-3m26, GHSA-7r86-cg39-jmmj, GHSA-23c5-xmqv-rm74 (high ReDoS family) |
| `axios` | 1.15.0 | 1.20.0 | axios advisory family (incl. GHSA-pmwg-cvhr-8vh7 incomplete-fix in 1.15.0) |
| `sharp` | 0.34.5 | 0.35.4 | GHSA-f88m-g3jw-g9cj (high, libvips CVEs) |
| `@opentelemetry/core` | 2.6.1 | 2.11.0 | GHSA-8988-4f7v-96qf (moderate) |
| `@opentelemetry/resources` | 2.6.1 | 2.11.0 | same advisory via `resources → core` chain (resources 2.6.1 pinned core 2.6.1) |
| `@opentelemetry/sdk-logs` | 0.208.0 | 0.222.0 | same advisory via `sdk-logs → core` chain (0.208.0 pinned core 2.2.0); now links core 2.11.0 / resources 2.11.0 |
| `@anthropic-ai/sdk` | ^0.90.0 (→0.90.x) | ^0.95.0 (→0.95.2) | GHSA-p7fg-763f-g4gf (moderate) |
| `@hey-api/openapi-ts` (dev) | ^0.94.1 (→0.94.5) | ^0.99.0 (→0.99.0) | GHSA-hhx9-57xq-r5rw (moderate); also lifts `@hey-api/json-schema-ref-parser` 1.3.1 → 1.4.4 |

### Transitive (applied via targeted `pnpm.overrides`-equivalent resolution)

| Package | From | To | Advisories resolved |
|---|---|---|---|
| `fast-uri` | 3.1.2 | 3.1.7 | GHSA-f65p-4m7j-42xc, GHSA-jqff-g426-hqxp, GHSA-7p8r-x3mc-p8w7, GHSA-v2hh-gcrm-f6hx, GHSA-q3j6-qgpj-74h6, GHSA-4c8g-83qw-93j6, GHSA-v39h-62p7-jpjc, GHSA-fph4-wmhf-6fwf (SSRF/host-confusion family) — via `ajv` ← `@modelcontextprotocol/sdk` |
| `picomatch` | 2.3.1 | 2.3.2 | GHSA-c2c7-rcm5-vvqj, GHSA-3v7f-55p6-f55p — via `micromatch` ← `@parcel/watcher`; `chokidar 3` ← `@allternit/browser` |
| `brace-expansion` | 1.1.12 / 2.0.2 / 5.0.5 | 1.1.18 / (2.x line removed from tree) / 5.0.9 | GHSA-3jxr-9vmj-r5cp, GHSA-f886-m6hf-6m8v, GHSA-mh99-v99m-4gvg, GHSA-rgw5-rvv9-x895, GHSA-jxxr-4gwj-5jf2 (OOM family) |
| `browserslist` | 4.28.2 | 4.28.8 | GHSA-c83g-rgw3-j3cx, GHSA-73wf-gq98-2v4g — via `@babel/helper-compilation-targets` (dev) |
| `js-yaml` (3.x) | 3.14.2 | 3.15.2 | GHSA-h67p-54hq-rp68, GHSA-52cp-r559-cp3m, GHSA-5p4m-2wfm-xmqj — via `gray-matter` (runtime) |
| `js-yaml` (4.x) | 4.1.1 (pinned by old ref-parser) / 4.2.0 | 4.3.2 in the pnpm tree | same three advisories — via `@hey-api/json-schema-ref-parser` (dev). ⚠️ **bun.lock caveat:** ref-parser 1.4.4 pins `js-yaml: "4.2.0"` *exactly*, so a from-scratch bun resolution (and the committed `bun.lock`) records **4.2.0, which is still affected**. The clean 4.3.2 in the installed pnpm tree exists only because of the transient override below. Bun was tested with scoped `overrides` (`js-yaml@>=4: 4.3.2`) and silently did not apply them. See "Remaining" |
| `body-parser` | 1.20.4 / 2.2.2 | 1.20.6 / 2.3.0 | GHSA-v422-hmwv-36x6 — via `express 4/5` ← `@allternit/orchestrator` chain |
| `path-to-regexp` | 8.4.2 | (unchanged) | already patched — stale bun.lock had 8.3.0 |
| `defu` | 6.1.7 | (unchanged) | already patched |

### SDK bump compatibility (1.25.2 → 1.29.0)

Every module gizzi-code imports was diffed between the two versions
(`client/index`, `client/streamableHttp`, `client/sse`, `client/stdio`,
`client/auth`, `server/index`, `server/stdio`, `server/auth/errors`,
`shared/auth`, `shared/transport`, `types`): **no runtime export present in
1.25.2 was removed in 1.29.0**. Most gizzi-code imports are type-only and
served by ambient declarations in `src/types/global.d.ts`. No importing-code
changes were required. Runtime smoke suite (1065 tests incl. MCP client tests)
passes on the new version.

## Not reachable from gizzi-code (stale-lock findings only)

| Advisory | Package | Verdict |
|---|---|---|
| GHSA-mv8w-475r-vwqw (critical) | `seroval@1.5.x` | Not in gizzi-code's dependency closure (`pnpm why` empty; store copy belongs to other workspace packages). Not bundled into the gizzi-code binary. The regenerated bun.lock no longer contains it. |
| GHSA-5v7r-6r5c-r473 (mod) | `file-type` | Same — not in gizzi-code's closure. |

## Remaining (cannot be fixed inside this change's ownership)

| Advisory | Package | Blocker |
|---|---|---|
| GHSA-x5fp-wj9c-mxmx, GHSA-4mjr-xmp4-gh2g (mod) | `qs@6.15.x` | `express@5.2.1` / `body-parser@2.3.0` pin `qs ~6.15.1`; fix needs qs 6.16.0 (OSV-clean). Blocked until express ships a `~6.16` range. Reachable at runtime via the `@allternit/orchestrator` chain. |
| undici 5.x family (mod/low) | `undici@5.29.0` | `@actions/core@1.11.1` / `@actions/github@6.0.1` (both already latest) require `undici ^5`; 5.29.0 is the last 5.x and is still flagged. The `undici ^6` chain (context7-mcp) already resolves to clean 6.28.0. |
| GHSA-hq66-cqwq-w95j (high) | `pdfjs-dist@5.7.x` | Dependency of workspace package `@allternit/sdk` (`sdk/allternit-sdk/package.json`, `pdfjs-dist ^5.4.54`; fix needs ≥6.2.108). Outside this change's file ownership — **owner of `sdk/allternit-sdk` should bump pdfjs-dist to ^6.2.108**. |
| GHSA-52cp-r559-cp3m, GHSA-5p4m-2wfm-xmqj (high) | `js-yaml@4.2.0` (only in the committed `bun.lock`) | `@hey-api/json-schema-ref-parser@1.4.4` (latest) exact-pins `js-yaml: "4.2.0"`; fix needs upstream ref-parser release pinning ≥4.3.0. The **installed pnpm tree is clean** (4.3.2 via transient override), so the shipped binary is unaffected; a fresh `bun install` from the lock would regress to 4.2.0 until ref-parser publishes or an override is adopted. |
| GHSA-866g-f22w-33x8 (low) | `@ai-sdk/provider-utils@3.0.21` | Entire stable 3.x line (latest 3.0.36) is within the affected range `<=3.0.97`; fix requires the 5.x line, which cascades into `@ai-sdk/*` 2.0.x / `ai` 5.0.x major upgrades — too large a blast radius for the deploy window. |
| GHSA-5xrq-8626-4rwp (critical), vite/esbuild family (mod/high) | `vitest@1.6.1`, `vite@5.4.21`, `esbuild@0.21.5` | Dev-dependencies of *linked workspace members* (`@allternit/browser`, `@allternit/governor`, `@allternit/computer-use-protocol` — pulled in via `@allternit/orchestrator`). They are **not installed in gizzi-code's pnpm tree** (pnpm does not install devDeps of linked workspace packages) and are **not bundled** into the binary; they appear only in the bun-workspace-style audit of member package.json files. Fixing requires bumps in those packages' own package.json files (other owners). |

## Methodology notes (transient workspace fixes)

> **RESOLVED 2026-09-07:** `surfaces/ai.allternit.com/package.json` now depends on
> the correct workspace name `@allternit/office-suite` and `pnpm-lock.yaml` was
> regenerated; pnpm resolution is clean. The note below is kept for history.
>
> ~~The worktree's pnpm resolution was already broken at HEAD, independent of this
> change: `surfaces/ai.allternit.com/package.json` depends on
> `@allternit/allternit-office-suite@workspace:*`, a package name that exists
> nowhere in the monorepo (the directory `platform/packages/allternit-office-suite`
> is named `@allternit/office-suite`), and the committed `pnpm-lock.yaml`
> predates the `better-sqlite3` override in `pnpm-workspace.yaml`. Both block any
> lockfile-touching pnpm operation.~~

To apply the upgrades, both files were **temporarily** adjusted (dead dep line
removed; `better-sqlite3` override line removed; targeted security overrides
added), `pnpm install`/`pnpm update --filter @allternit/gizzi-code` were run,
and both files were then **restored byte-identical** (md5-verified, git-clean).
The security overrides used (recommended for permanent adoption in the root
`pnpm-workspace.yaml` — outside this change's ownership):

```yaml
'fast-uri@3.1.2': 3.1.7
'picomatch@2.3.1': 2.3.2
'js-yaml@3.14.2': 3.15.2
'js-yaml@4.1.1': 4.3.2
'js-yaml@4.2.0': 4.3.2
'body-parser@1.20.4': 1.20.6
'body-parser@2.2.2': 2.3.0
'brace-expansion@2.0.2': 2.1.4
```

## Collateral / follow-ups

- Root `pnpm-lock.yaml` was updated by the pnpm runs (it governs the installed
  tree; it was already un-installable at HEAD due to the two breakages above,
  and remains so until a workspace owner fixes them — no regression).
- `cmd/gizzi-code/bun.lock` regenerated from the final package.json (mirrors
  the pnpm tree's key versions; `bun audit` now audits the real dependency set).

## Verification

- `NODE_OPTIONS=--max-old-space-size=8192 ./node_modules/.bin/tsc --noEmit` —
  only failures are 7 pre-existing errors in `packages/sdk/scripts/verify-sdk.ts`
  (imports of unbuilt `packages/sdk/dist/*` artifacts — another owner's area,
  unrelated to dependencies; present at HEAD).
- `bash script/ci-smoke-test.sh` — **841 pass / 0 fail / 224 skip** (1065 tests, 86 files, exit 0).
- `bun run build` + `./dist/gizzi-code --version` — see deploy report.
- `bun audit` before: **86 (1 critical / 30 high / 47 moderate / 8 low**, stale lock). After, on the
  regenerated lock: **24 (1 / 8 / 12 / 3)**. The 1 critical (`vitest <3.2.6`) is a devDep of
  linked workspace members only — not installed in gizzi-code's pnpm tree, not bundled. The 8 high
  are: js-yaml (2, lock-only — see "Remaining"), vite/esbuild family (5, same not-bundled caveat),
  pdfjs-dist (1, other owner's package).
