#!/usr/bin/env bash
# ensure-sdk-dist.sh — build packages/sdk dist if it is missing or stale.
#
# Why this exists:
#   packages/sdk/dist is gitignored (only dist/gen is tracked, vendored from
#   the canonical release repo). A fresh clone/worktree therefore has a dist/
#   containing ONLY gen/, and any src/ newer than the last build leaves stale
#   output behind. Both cases make `bun run typecheck` fail with TS2307
#   ("Cannot find module '../dist/...'") in packages/sdk/scripts/verify-sdk.ts,
#   and make `bun test` silently resolve @allternit/sdk against a stale build.
#
# What this does:
#   - No-op if dist/index.js exists AND no packages/sdk/src file is newer
#     than it (find -newer check).
#   - Otherwise runs the SDK build (packages/sdk/scripts/build.mjs — plain
#     tsc emit; ~1.8k pre-existing type errors are the known baseline and do
#     not fail the build).
#   - Exits non-zero if the sentinel dist/index.js is still missing after the
#     build, so callers fail loudly instead of typechecking a broken tree.
#   - Beyond mtime: each managed package records the resolved version of its
#     runtime dependencies in a dist/.build-deps.json sidecar at build time.
#     A missing sidecar (dist predates this check) or a snapshot that no
#     longer matches the current resolution (e.g. stale pnpm workspace links
#     silently moved a package's dep from its nested node_modules to a
#     root-hoisted copy of a DIFFERENT major version) forces a rebuild. This
#     is the 2026-09-18 poisoned-dist incident guard: then, os-contracts'
#     build resolved root-hoisted zod v4 instead of its nested zod v3, errored
#     under v4, and still emitted a v4-typed .d.ts that was newer than src —
#     mtime freshness alone called it fresh. os-contracts' build config also
#     sets noEmitOnError so a type-erroring build fails instead of emitting.
#
# Usage: bash script/ensure-sdk-dist.sh   (from anywhere; resolves its own root)
# Set GIZZI_SKIP_SDK_DIST=1 to skip (e.g. CI images that pre-build the SDK).

set -u

if [ "${GIZZI_SKIP_SDK_DIST:-0}" = "1" ]; then
  echo "ensure-sdk-dist: skipped (GIZZI_SKIP_SDK_DIST=1)"
  exit 0
fi

cd "$(dirname "$0")/.."

# dep_snapshot <pkg-dir> — print a JSON map of the package's runtime
# dependencies to their currently resolved versions. Resolution mirrors
# node's lookup: the package's own node_modules first, then each parent dir's
# up to the filesystem root (covers pnpm's root-hoisted fallback). Deps that
# resolve nowhere are recorded as UNRESOLVED, which also triggers a rebuild
# once they become installable.
dep_snapshot() {
  node -e '
    const fs = require("fs"), path = require("path");
    const pkgDir = path.resolve(process.argv[1]);
    const pkg = JSON.parse(fs.readFileSync(path.join(pkgDir, "package.json"), "utf8"));
    const deps = Object.keys(pkg.dependencies || {}).sort();
    const resolved = {};
    for (const dep of deps) {
      let dir = pkgDir, found = null;
      for (;;) {
        const cand = path.join(dir, "node_modules", dep, "package.json");
        if (fs.existsSync(cand)) { found = cand; break; }
        const parent = path.dirname(dir);
        if (parent === dir) break;
        dir = parent;
      }
      resolved[dep] = found
        ? JSON.parse(fs.readFileSync(found, "utf8")).version
        : "UNRESOLVED";
    }
    console.log(JSON.stringify(resolved, null, 2));
  ' "$1"
}

SDK="packages/sdk"
SENTINEL="$SDK/dist/index.js"
SIDECAR="$SDK/dist/.build-deps.json"

if [ ! -d "$SDK/src" ]; then
  echo "ERROR: ensure-sdk-dist: $SDK/src not found (run from the gizzi-code tree)" >&2
  exit 1
fi

stale=0
if [ ! -f "$SENTINEL" ]; then
  stale=1
  reason="missing $SENTINEL (fresh clone/worktree: only tracked dist/gen is present)"
elif [ ! -f "$SIDECAR" ]; then
  stale=1
  reason="missing $SIDECAR (dist predates the dependency-snapshot check)"
elif find "$SDK/src" -type f -name '*.ts' -newer "$SENTINEL" | grep -q .; then
  stale=1
  reason="packages/sdk/src is newer than $SENTINEL"
elif ! dep_snapshot "$SDK" | diff -q - "$SIDECAR" >/dev/null 2>&1; then
  stale=1
  reason="resolved dependencies changed since the last build (see $SIDECAR)"
fi

if [ "$stale" -eq 1 ]; then
  echo "ensure-sdk-dist: rebuilding packages/sdk dist ($reason)"
  (cd "$SDK" && bun run build)
  if [ ! -f "$SENTINEL" ]; then
    echo "ERROR: ensure-sdk-dist: build finished but $SENTINEL is still missing" >&2
    exit 1
  fi
  dep_snapshot "$SDK" > "$SIDECAR"
  echo "ensure-sdk-dist: dist rebuilt"
fi

# ── @allternit/os-contracts (platform/packages/os-contracts) ────────────────
# src/runtime/fabric/* imports '@allternit/os-contracts' (workspace:*). Only
# its dist is consumable (main/types point at ./dist) and dist is gitignored,
# so a fresh clone/worktree typechecks with 3x TS2307. Build it when missing
# or stale, same contract as the SDK blocks above — plus noEmitOnError in its
# tsconfig.build.json, so a build whose types don't compile (wrong zod major)
# fails loudly here instead of emitting a poisoned dist.
OC_SDK="../../platform/packages/os-contracts"
OC_SENTINEL="$OC_SDK/dist/index.js"
OC_SIDECAR="$OC_SDK/dist/.build-deps.json"

if [ ! -d "$OC_SDK/src" ]; then
  echo "ERROR: ensure-sdk-dist: $OC_SDK/src not found (run from the gizzi-code tree)" >&2
  exit 1
fi

oc_stale=0
if [ ! -f "$OC_SENTINEL" ]; then
  oc_stale=1
  oc_reason="missing $OC_SENTINEL"
elif [ ! -f "$OC_SIDECAR" ]; then
  oc_stale=1
  oc_reason="missing $OC_SIDECAR (dist predates the dependency-snapshot check)"
elif find "$OC_SDK/src" -type f -name '*.ts' -newer "$OC_SENTINEL" | grep -q .; then
  oc_stale=1
  oc_reason="platform/packages/os-contracts/src is newer than $OC_SENTINEL"
elif ! dep_snapshot "$OC_SDK" | diff -q - "$OC_SIDECAR" >/dev/null 2>&1; then
  oc_stale=1
  oc_reason="resolved dependencies changed since the last build (see $OC_SIDECAR)"
fi

if [ "$oc_stale" -eq 1 ]; then
  echo "ensure-sdk-dist: rebuilding platform/packages/os-contracts dist ($oc_reason)"
  # os-contracts builds with noEmitOnError — a type-erroring build (e.g. its
  # zod v3 sources compiled against a root-hoisted zod v4) fails here instead
  # of emitting a poisoned dist, and the script exits non-zero so callers
  # never typecheck against a broken tree.
  if ! (cd "$OC_SDK" && bun run build); then
    echo "ERROR: ensure-sdk-dist: os-contracts build failed (noEmitOnError: dist not emitted)" >&2
    exit 1
  fi
  if [ ! -f "$OC_SENTINEL" ]; then
    echo "ERROR: ensure-sdk-dist: build finished but $OC_SENTINEL is still missing" >&2
    exit 1
  fi
  dep_snapshot "$OC_SDK" > "$OC_SIDECAR"
  echo "ensure-sdk-dist: os-contracts dist rebuilt"
fi

exit 0
