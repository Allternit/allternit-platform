# gizzi-tui-parity P11 + desktop rebuild — memdir convergence, bundle fix, new binary

- Session: continuation of session/gizzi-tui-parity closeout items (owner-directed).
- PR: #759 — merged ebedb01b3 (commit 1fb9afa3f)

## What was done

### 1. memdir env-var convergence (the P10 follow-up)
Both real memdir copies (`src/memdir/paths.ts`, `src/cli/ui/ink-app/memdir/paths.ts`)
now honor BOTH `GIZZI_CODE_REMOTE_MEMORY_DIR` (wins) and legacy
`GIZZI_REMOTE_MEMORY_DIR` via `getRemoteMemoryDirOverride()`; the CCR memory-off gate
and `getMemoryBaseDir()` both use it; `spawnUtils` forwards both names to teammates.
4 new tests (`test/memdir/paths-env.test.ts`); 23/23 memdir+memory suites.

### 2. Production-bundle build break found & fixed (root cause, not symptom)
The desktop rebuild (lifecycle step 8) exposed that `build-production.js`'s `@/` alias
fallback tries `src/shared/` FIRST, so `@/memdir/paths` resolved to
`src/shared/memdir/paths.ts` — an auto-generated SHIM with a different memory base
(`~/.config/gizzi/memory/auto`). tsc and `bun test` resolve the same import to the real
`src/memdir/paths.ts`, which is why tests/typecheck/preflight were all green on P10 while
the production binary was broken (missing export → hard build failure; and any earlier
`@/memdir/*` import in the binary silently got shim behavior). Fix: explicit `memdir`
head case in the bundler alias pinning to `src/memdir`. The shims are KEPT verbatim:
`src/shared/utils/{messages,config,gizzimd,attachments,memoryFileDetection,
sessionFileAccessHooks}.ts` import them via RELATIVE paths — that divergence pre-exists
this program and is unchanged here. Follow-up candidate: consolidate or delete the shim
tree and repoint those relative imports.

### 3. Desktop binary rebuilt from merged main (lifecycle step 8, now unblocked)
- gizzi-code production binary (darwin-arm64) → `resources/bin/gizzi-code`
- `npm run build` (main/preload/auth-renderer) + electron-builder DMG, unsigned local
  (`CSC_IDENTITY_AUTO_DISCOVERY=false`, no APPLE_* creds; notarize.cjs skipped by design)
- electron-builder required `ALLTERNIT_BUILD_SUFFIX` (CI sets it to ""; local convention
  is -bNNNN) → built `Allternit-Desktop-1.1.1-b3704-arm64.dmg`
- Bundle verification: strings grep of the packaged sidecar confirms P7/P8 hooks
  (`GIZZI_SETTINGS_HOOKS`), P9 (`quota n/a`), P11 (`getRemoteMemoryDirOverride`);
  `--version` runs (2.1.1)
- One-build rule enforced: b3703 DMG + blockmap deleted; exactly one DMG + one unpacked
  bundle on disk
- NOTE: the installed `/Applications/Allternit Desktop.app` is still the OLD build;
  installing b3704 (drag from DMG) is a manual step for the owner.

## Verification
- bun test memdir+memory: 23/23; typecheck clean; release-preflight 52/0
- `bun run script/build-production.js --target=darwin-arm64`: builds (was broken on main)

## Incidents
- Subagent agent-10 (env-var fix) died on the 5-hour quota limit mid-task; the parent
  implemented the fix directly. Its abandoned worktree (identical partial fix) was
  inspected, confirmed superseded, and removed with its branch.
- release-preflight.mjs does NOT run the production bundle build — the alias/shim break
  class is invisible to it. Candidate hardening: add a bundle-smoke step to preflight.
