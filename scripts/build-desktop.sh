#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Allternit Desktop — Full Build Pipeline
#
# Usage:
#   ./scripts/build-desktop.sh [--release] [--skip-platform] [--skip-api] [--skip-electron]
#
# A normal build makes the app for this Mac only (one DMG, this Mac's driver
# Python). --release makes every mac artifact (arm64 + x64, DMG + zip) and
# stages the driver Python for all platforms. The public GitHub release is
# built by .github/workflows/release-desktop.yml (see DESKTOP_RELEASE.md).
#
# Runs through the machine-wide build queue (scripts/build-queue): it waits
# for any other heavy build to finish and records the commits it built.
# ALLTERNIT_BUILD_MAIN=1 moves clean, detached checkouts to the newest
# origin/main when the build starts. Rust sidecars whose sources did not
# change are reused (scripts/bin-cache), and so is gizzi-code when nothing it
# is built from changed; ALLTERNIT_NO_BIN_CACHE=1 rebuilds. Each stage prints
# how long it took, with a summary at the end.
# ─────────────────────────────────────────────────────────────────────────────

set -e

# Detect directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=hosted-ui.sh
. "$SCRIPT_DIR/hosted-ui.sh"
API_DIR="$WORKSPACE_ROOT/cmd/allternit-api"
GIZZI_DIR="$WORKSPACE_ROOT/cmd/gizzi-code"
DESKTOP_DIR="$WORKSPACE_ROOT/surfaces/allternit-desktop"
RESOURCES_DIR="$DESKTOP_DIR/resources"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3.11 || command -v python3)}"

# UI Helpers
ok() { echo -e "\033[32m✓\033[0m $1"; }
warn() { echo -e "\033[33m⚠\033[0m $1"; }
die() { echo -e "\033[31m✗\033[0m $1" >&2; exit 1; }
# Stage times: each step closes the one before it; the summary prints on exit.
BUILD_START=$SECONDS
STAGE_NAME=""
STAGE_START=$SECONDS
STAGE_TIMES=()
end_stage() {
  [ -n "$STAGE_NAME" ] || return 0
  local took=$((SECONDS - STAGE_START))
  STAGE_TIMES+=("$(printf '%4dm%02ds  %s' $((took / 60)) $((took % 60)) "$STAGE_NAME")")
  echo -e "\033[2m  ⏱ $((took / 60))m$((took % 60))s  $STAGE_NAME\033[0m"
  STAGE_NAME=""
}
step() { end_stage; STAGE_NAME="$1"; STAGE_START=$SECONDS; echo ""; echo -e "\033[1;34m▶\033[0m $1"; }
print_stage_times() {
  end_stage
  [ ${#STAGE_TIMES[@]} -gt 0 ] || return 0
  local total=$((SECONDS - BUILD_START))
  echo ""
  echo "Stage times:"
  printf '%s\n' "${STAGE_TIMES[@]}"
  printf '%4dm%02ds  total\n' $((total / 60)) $((total % 60))
}

# Parse flags
SKIP_PLATFORM=false
SKIP_API=false
SKIP_ELECTRON=false
RELEASE=false
for arg in "$@"; do
  case $arg in
    --release) RELEASE=true ;;
    --skip-platform) SKIP_PLATFORM=true ;;
    --skip-api) SKIP_API=true ;;
    --skip-electron) SKIP_ELECTRON=true ;;
  esac
done

# One heavy build at a time, machine-wide. The queue re-runs this script once
# the lane is free (BUILD_QUEUE_HELD=1 then skips this block).
if [ -z "${BUILD_QUEUE_HELD:-}" ]; then
  QUEUE_ARGS=(--repo "$WORKSPACE_ROOT" --repo "${ALLTERNIT_AI_PATH:-$WORKSPACE_ROOT/../allternit-ai-wt-desktop-build}")
  [ "${ALLTERNIT_BUILD_MAIN:-}" = 1 ] && QUEUE_ARGS+=(--sync-main)
  exec "$SCRIPT_DIR/build-queue" run desktop "${QUEUE_ARGS[@]}" -- "$SCRIPT_DIR/build-desktop.sh" "$@"
fi

trap print_stage_times EXIT
cd "$WORKSPACE_ROOT"

# This Mac only unless --release (prepare-allternit-driver.cjs, build-local.cjs).
if [ "$RELEASE" = false ]; then
  export ALLTERNIT_HOST_ONLY=1
fi

# Cargo output: the shared target dir (AGENTS.md: every session exports
# CARGO_TARGET_DIR) or the default ./target. Looking only in ./target made
# every stage after a shared-cache compile fail with "binary not found".
CARGO_OUT="${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}"
case "$CARGO_OUT" in /*) ;; *) CARGO_OUT="$WORKSPACE_ROOT/$CARGO_OUT" ;; esac

# reuse <cache name> <dest>: copy the last build of a sidecar to <dest> when
# none of its sources changed. remember <cache name> <cargo binary>: keep a
# fresh build for next time (keyed by cargo's <binary>.d source list).
reuse() { "$SCRIPT_DIR/bin-cache" fetch "$1" "$2" --root "$WORKSPACE_ROOT"; }
remember() { "$SCRIPT_DIR/bin-cache" store "$1" "$2" "$2.d" --root "$WORKSPACE_ROOT" || warn "bin-cache: could not keep $1"; }

# ── 1. Workspace UI (ai.allternit.com) ──────────────────────────────────────
# Built once, by prepare-platform-static in step 5 (`pnpm run dist`), with the
# Desktop env (local gateway, Clerk key). This step only picks the checkout and
# pins it; building it here as well cost a second full Vite build every time.
if [ "$SKIP_PLATFORM" = false ]; then
  step "Preparing workspace UI checkout (ai.allternit.com)…"
  PLATFORM_DIR="$(resolve_hosted_ui "$WORKSPACE_ROOT")" || die "ai.allternit.com UI not found"
  link_oss_platform "$PLATFORM_DIR" "$WORKSPACE_ROOT"
  # Packaging refuses a UI behind origin/main (prepare-platform-static.cjs).
  # A UI that IS main now stays acceptable when main moves during this build.
  git -C "$PLATFORM_DIR" fetch --quiet origin main 2>/dev/null || true
  UI_HEAD="$(git -C "$PLATFORM_DIR" rev-parse HEAD 2>/dev/null || true)"
  if [ -n "$UI_HEAD" ] && [ "$UI_HEAD" = "$(git -C "$PLATFORM_DIR" rev-parse origin/main 2>/dev/null)" ]; then
    export ALLTERNIT_UI_MAIN_AT_START="$UI_HEAD"
  fi
  # prepare-platform-static builds this same checkout.
  export ALLTERNIT_AI_PATH="$PLATFORM_DIR"
  if [ "$SKIP_ELECTRON" = true ]; then
    # No packaging step will build it: build it here.
    (cd "$PLATFORM_DIR" && NEXT_PUBLIC_ALLTERNIT_DESKTOP_AUTH=1 pnpm run build)
    [ -d "$PLATFORM_DIR/dist" ] || die "Platform build failed — output directory not found at $PLATFORM_DIR/dist"
    ok "Platform static export → $PLATFORM_DIR/dist"
  else
    ok "Workspace UI → $PLATFORM_DIR (built once when packaging)"
  fi
fi

# ── 2. Build Gizzi Code Binary ───────────────────────────────────────────────
step "Building gizzi-code binary…"
# Detect target triple for this machine
ARCH=$(uname -m | sed 's/arm64/arm64/;s/x86_64/x64/')
OS=$(uname | tr '[:upper:]' '[:lower:]' | sed 's/darwin/darwin/')
GIZZI_TARGET="${OS}-${ARCH}"   # e.g. darwin-arm64

# Reused when nothing it is built from changed: every tracked file of
# gizzi-code and the workspace packages it depends on, the lockfile and bun.
cd "$WORKSPACE_ROOT"
mkdir -p "$RESOURCES_DIR/bin"
if ! reuse "gizzi-code-$GIZZI_TARGET" "$RESOURCES_DIR/bin/gizzi-code"; then
  # bun install inside cmd/gizzi-code fails: it is a pnpm workspace package
  # (workspace:* deps). Match CI (gizzi-code-quality.yml): install/build from
  # the repo root, then compile the binary.
  pnpm --filter "@allternit/gizzi-code^..." run build
  pnpm --filter "@allternit/gizzi-sdk" run build
  (cd "$GIZZI_DIR" && bun run script/build-production.js --target="$GIZZI_TARGET")

  GIZZI_BIN="$GIZZI_DIR/dist/gizzi-code"
  # The build script might suffix it with the target, check both
  if [ ! -f "$GIZZI_BIN" ]; then
      GIZZI_BIN="$GIZZI_DIR/dist/gizzi-code-$GIZZI_TARGET"
  fi

  [ -f "$GIZZI_BIN" ] || die "gizzi-code build failed — binary not found at $GIZZI_BIN"

  cp "$GIZZI_BIN" "$RESOURCES_DIR/bin/gizzi-code"
  GIZZI_DIRS="$(pnpm -r --filter "@allternit/gizzi-code..." exec pwd 2>/dev/null | sed "s#^$WORKSPACE_ROOT/##" | paste -sd, -)"
  if [ -n "$GIZZI_DIRS" ]; then
    "$SCRIPT_DIR/bin-cache" store-dirs "gizzi-code-$GIZZI_TARGET" "$GIZZI_BIN" --root "$WORKSPACE_ROOT" \
      --dirs "$GIZZI_DIRS" --files pnpm-lock.yaml,package.json,pnpm-workspace.yaml --tool "bun --version" \
      || warn "bin-cache: could not keep gizzi-code"
  fi
fi
chmod +x "$RESOURCES_DIR/bin/gizzi-code"
ok "gizzi-code → $RESOURCES_DIR/bin/gizzi-code"

# ── 2a. Vendor allternit-factory (the Factory engine Desktop runs; its pane
#        engine runs every terminal: allternit-api /terminal, gizzi /pty) ──
step "Vendoring allternit-factory…"
if ! reuse allternit-factory "$RESOURCES_DIR/bin/allternit-factory"; then
  (cd "$WORKSPACE_ROOT" && cargo build --release -p allternit-factory)
  FACTORY_BIN="$CARGO_OUT/release/allternit-factory"
  [ -f "$FACTORY_BIN" ] || die "allternit-factory build failed — binary not found at $FACTORY_BIN"
  cp "$FACTORY_BIN" "$RESOURCES_DIR/bin/allternit-factory"
  remember allternit-factory "$FACTORY_BIN"
fi
chmod +x "$RESOURCES_DIR/bin/allternit-factory"
ok "allternit-factory → $RESOURCES_DIR/bin/allternit-factory"

# ── 2b. Vendor ripgrep (GrepTool backend, Claude Code layout) ───────────────
step "Vendoring ripgrep…"
RG_LAYOUT="$(uname -m | sed 's/x86_64/x64/')-$(uname | tr '[:upper:]' '[:lower:]' | sed 's/darwin/darwin/')"
RG_SRC="$GIZZI_DIR/vendor/ripgrep/$RG_LAYOUT/rg"
if [ ! -f "$RG_SRC" ]; then
  "$GIZZI_DIR/script/vendor-ripgrep.sh" || die "ripgrep vendor download failed"
fi
mkdir -p "$RESOURCES_DIR/bin/vendor/ripgrep/$RG_LAYOUT"
cp "$RG_SRC" "$RESOURCES_DIR/bin/vendor/ripgrep/$RG_LAYOUT/rg"
chmod +x "$RESOURCES_DIR/bin/vendor/ripgrep/$RG_LAYOUT/rg"
ok "ripgrep → $RESOURCES_DIR/bin/vendor/ripgrep/$RG_LAYOUT/rg"

# ── 2c. Build Voice Service Sidecar (Rust + sherpa-onnx) ────────────────────
step "Building bundled voice service (sherpa-onnx)…"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
cd "$WORKSPACE_ROOT"
if reuse voice-service "$RESOURCES_DIR/bin/allternit-voice-service" && reuse allternit-tts "$RESOURCES_DIR/bin/allternit-tts"; then
  VOICE_REUSED=true
else
  VOICE_REUSED=false
  cargo build --release -p voice-service -p allternit-tts
  VOICE_BIN="$CARGO_OUT/release/voice-service"
  [ -f "$VOICE_BIN" ] || VOICE_BIN="$CARGO_OUT/release/allternit-voice-service"
  [ -f "$VOICE_BIN" ] || die "Voice service build failed — binary not found at $VOICE_BIN"
  cp "$VOICE_BIN" "$RESOURCES_DIR/bin/allternit-voice-service"
fi
chmod +x "$RESOURCES_DIR/bin/allternit-voice-service"

# The voice service must not contain espeak-ng (GPL-3.0): TTS runs in the
# separate allternit-tts program (services/voice-tts, GPL-3.0-or-later),
# which the voice service starts as a child from the same directory.
bash "$WORKSPACE_ROOT/scripts/check-voice-no-gpl.sh" "$RESOURCES_DIR/bin/allternit-voice-service"
if [ "$VOICE_REUSED" = false ]; then
  TTS_BIN="$CARGO_OUT/release/allternit-tts"
  [ -f "$TTS_BIN" ] || die "allternit-tts build failed — binary not found at $TTS_BIN"
  cp "$TTS_BIN" "$RESOURCES_DIR/bin/allternit-tts"
  remember voice-service "$VOICE_BIN"
  remember allternit-tts "$TTS_BIN"
fi
chmod +x "$RESOURCES_DIR/bin/allternit-tts"

# sherpa-onnx/onnxruntime link statically into both binaries (crate default
# `static` feature): no dylib/DLL to stage next to them. Voice models
# download on first use at runtime into ~/.allternit/models/voice/ —
# nothing model-related ships in the app.
ok "voice service → $RESOURCES_DIR/bin/allternit-voice-service + allternit-tts (sherpa-onnx, static)"

# ── 3. Build Rust API ────────────────────────────────────────────────────────
if [ "$SKIP_API" = false ]; then
  step "Building allternit-api (Rust)…"
  mkdir -p "$RESOURCES_DIR/bin"
  if ! reuse allternit-api "$RESOURCES_DIR/bin/allternit-api"; then
    cd "$API_DIR"
    cargo build --release

    # Map binary name (Cargo uses underscores, we prefer dashes for distribution)
    # In workspace builds, binary is in the root target dir
    API_BIN="$CARGO_OUT/release/allternit-api"
    [ -f "$API_BIN" ] || API_BIN="$CARGO_OUT/release/allternit_api"
    [ -f "$API_BIN" ] || API_BIN="$API_DIR/target/release/allternit-api"
    [ -f "$API_BIN" ] || API_BIN="$API_DIR/target/release/allternit_api"
    [ -f "$API_BIN" ] || die "API build failed — binary not found at $API_BIN"

    cp "$API_BIN" "$RESOURCES_DIR/bin/allternit-api"
    remember allternit-api "$API_BIN"
  fi
  chmod +x "$RESOURCES_DIR/bin/allternit-api"
  ok "allternit-api → $RESOURCES_DIR/bin/allternit-api"

  # 3a. Local model engine (services/local-engine) backing Model Lab telemetry.
  step "Building local-engine (Rust)…"
  LOCAL_ENGINE_DIR="$WORKSPACE_ROOT/services/local-engine"
  if ! reuse local-engine "$RESOURCES_DIR/bin/allternit-local-engine"; then
    (cd "$LOCAL_ENGINE_DIR" && cargo build --release)

    # Cargo names the bin `local-engine` (see [[bin]] in its Cargo.toml); stage
    # it as allternit-local-engine so resources/bin keeps one naming convention.
    LOCAL_ENGINE_BIN="$CARGO_OUT/release/local-engine"
    [ -f "$LOCAL_ENGINE_BIN" ] || LOCAL_ENGINE_BIN="$LOCAL_ENGINE_DIR/target/release/local-engine"
    [ -f "$LOCAL_ENGINE_BIN" ] || die "local-engine build failed — binary not found at $LOCAL_ENGINE_BIN"

    cp "$LOCAL_ENGINE_BIN" "$RESOURCES_DIR/bin/allternit-local-engine"
    remember local-engine "$LOCAL_ENGINE_BIN"
  fi
  chmod +x "$RESOURCES_DIR/bin/allternit-local-engine"
  ok "local-engine → $RESOURCES_DIR/bin/allternit-local-engine"
fi

# ── 3b. Phone Link Apple ID signer (macOS) ─────────────────────────────────
# A standalone cargo workspace (cmd/allternit-sideload). Staged here so it is
# reused when unchanged; prepare-sideload.cjs then finds it already staged.
if [[ "$OSTYPE" == "darwin"* ]] && [ "${ALLTERNIT_SKIP_SIDELOAD:-}" != 1 ]; then
  step "Building allternit-sideload (Rust)…"
  SIDELOAD_DIR="$WORKSPACE_ROOT/cmd/allternit-sideload"
  if ! reuse allternit-sideload "$RESOURCES_DIR/bin/allternit-sideload"; then
    cargo build --release --manifest-path "$SIDELOAD_DIR/Cargo.toml"
    SIDELOAD_TARGET="$(cargo metadata --format-version 1 --no-deps --manifest-path "$SIDELOAD_DIR/Cargo.toml" | "$PYTHON_BIN" -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
    SIDELOAD_BIN="$SIDELOAD_TARGET/release/allternit-sideload"
    [ -f "$SIDELOAD_BIN" ] || die "allternit-sideload build failed — binary not found at $SIDELOAD_BIN"
    cp "$SIDELOAD_BIN" "$RESOURCES_DIR/bin/allternit-sideload"
    # Its own workspace and lockfile: keyed by its folder, not the root Cargo.lock.
    "$SCRIPT_DIR/bin-cache" store-dirs allternit-sideload "$SIDELOAD_BIN" --root "$WORKSPACE_ROOT" \
      --dirs cmd/allternit-sideload --tool "rustc -vV" || warn "bin-cache: could not keep allternit-sideload"
  fi
  chmod +x "$RESOURCES_DIR/bin/allternit-sideload"
  ok "allternit-sideload → $RESOURCES_DIR/bin/allternit-sideload"
fi

# ── 4. Download Lume Virtualization (macOS) ──────────────────────────────────
if [[ "$OSTYPE" == "darwin"* ]]; then
  step "Downloading Lume virtualization binary…"
  if [ -f "$DESKTOP_DIR/scripts/prepare-lume.cjs" ]; then
    (cd "$DESKTOP_DIR" && node scripts/prepare-lume.cjs) || die "prepare-lume.cjs failed"
    ok "Lume staged via prepare-lume.cjs"
  else
  LUME_VERSION="v0.3.9"
  # Strip the 'v' for the filename
  LUME_VER_SHORT=${LUME_VERSION#v}
  
  # Map uname architecture to Lume's naming
  LUME_ARCH=$(uname -m)
  if [ "$LUME_ARCH" = "x86_64" ]; then
    LUME_ARCH="x64"
  fi
  
  LUME_URL="https://github.com/trycua/cua/releases/download/lume-${LUME_VERSION}/lume-${LUME_VER_SHORT}-darwin-${LUME_ARCH}.tar.gz"
  LUME_TMP="/tmp/lume-${LUME_VERSION}.tar.gz"
  
  if [ ! -f "$RESOURCES_DIR/bin/lume" ]; then
    echo "Fetching $LUME_URL…"
    curl -L "$LUME_URL" -o "$LUME_TMP"
    tar -xzf "$LUME_TMP" -C "$RESOURCES_DIR/bin"
    chmod +x "$RESOURCES_DIR/bin/lume"
    rm "$LUME_TMP"
    ok "Lume → $RESOURCES_DIR/bin/lume"
  else
    ok "Lume already present at $RESOURCES_DIR/bin/lume"
  fi
  fi
fi

# ── 5. Build Electron App ───────────────────────────────────────────────────
if [ "$SKIP_ELECTRON" = false ]; then
  step "Building Electron app bundle…"
  cd "$DESKTOP_DIR"
  
  # Ensure dependencies are current. Frozen: a plain `pnpm install` re-resolves
  # and rewrote the shared pnpm-lock.yaml on every Desktop build (bumping jest's
  # @babel/core 7 -> 8); a lockfile that is really out of date should fail here.
  pnpm install --frozen-lockfile

  # Package: `dist` stages every sidecar, builds the UI and the main process
  # (`npm run build`) once, then runs electron-builder.
  pnpm run dist

  ok "Electron app built → $DESKTOP_DIR/release/"
fi

# ── 5. Patch SHA256 checksums into manifest.ts ───────────────────────────────
MANIFEST_FILE="$DESKTOP_DIR/src/main/manifest.ts"

patch_checksum() {
  local platform_key="$1"
  local binary_path="$2"

  [ -f "$binary_path" ] || { warn "Binary not found for $platform_key: $binary_path"; return; }

  if command -v sha256sum &>/dev/null; then
    local checksum
    checksum=$(sha256sum "$binary_path" | awk '{print $1}')
  else
    local checksum
    checksum=$(shasum -a 256 "$binary_path" | awk '{print $1}')
  fi

  # Replace the empty-string value for this key in the checksums block
  # Pattern: '<platform_key>':   '' → '<platform_key>':   '<sha256>'
  # Uses a delimiter that won't appear in keys or hashes (#)
  sed -i.bak "s#'${platform_key}':   *''#'${platform_key}':   '${checksum}'#g" "$MANIFEST_FILE"
  ok "Checksum patched → $platform_key: $checksum"
}

step "Patching SHA256 checksums into ${MANIFEST_FILE}…"

BINARY_PATH="$RESOURCES_DIR/bin/allternit-api"
if [ -f "$BINARY_PATH" ]; then
  # Detect platform key for manifest (arch-os)
  ARCH=$(uname -m | sed 's/arm64/aarch64/;s/x86_64/x86_64/')
  OS=$(uname | tr '[:upper:]' '[:lower:]' | sed 's/darwin/macos/')
  PLATFORM_KEY="${ARCH}-${OS}"
  patch_checksum "$PLATFORM_KEY" "$BINARY_PATH"
  rm -f "${MANIFEST_FILE}.bak"
else
  warn "Binary not found at $BINARY_PATH — checksums not patched."
fi

echo ""
ok "Build complete! App: $DESKTOP_DIR/release/"
