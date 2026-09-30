#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Allternit Desktop — Full Build Pipeline
#
# Usage:
#   ./scripts/build-desktop.sh [--skip-platform] [--skip-api] [--skip-electron]
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
VOICE_DIR="$WORKSPACE_ROOT/services/voice"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3.11 || command -v python3)}"

# UI Helpers
ok() { echo -e "\033[32m✓\033[0m $1"; }
warn() { echo -e "\033[33m⚠\033[0m $1"; }
die() { echo -e "\033[31m✗\033[0m $1" >&2; exit 1; }
step() { echo ""; echo -e "\033[1;34m▶\033[0m $1"; }

# Parse flags
SKIP_PLATFORM=false
SKIP_API=false
SKIP_ELECTRON=false
for arg in "$@"; do
  case $arg in
    --skip-platform) SKIP_PLATFORM=true ;;
    --skip-api) SKIP_API=true ;;
    --skip-electron) SKIP_ELECTRON=true ;;
  esac
done

cd "$WORKSPACE_ROOT"

# Cargo output: the shared target dir (AGENTS.md: every session exports
# CARGO_TARGET_DIR) or the default ./target. Looking only in ./target made
# every stage after a shared-cache compile fail with "binary not found".
CARGO_OUT="${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}"
case "$CARGO_OUT" in /*) ;; *) CARGO_OUT="$WORKSPACE_ROOT/$CARGO_OUT" ;; esac

# ── 1. Build Platform static export ──────────────────────────────────────────
# The Electron app loads the hosted platform by default; the static export is
# built later by prepare-platform-static (Vite) for offline fallback. The
# legacy Next.js standalone server build is skipped because the platform is
# now a Vite app and the standalone output is no longer consumed.
if [ "$SKIP_PLATFORM" = false ]; then
  step "Building workspace UI (ai.allternit.com)…"
  PLATFORM_DIR="$(resolve_hosted_ui "$WORKSPACE_ROOT")" || die "ai.allternit.com UI not found"
  link_oss_platform "$PLATFORM_DIR" "$WORKSPACE_ROOT"
  cd "$PLATFORM_DIR"
  NEXT_PUBLIC_ALLTERNIT_DESKTOP_AUTH=1 pnpm run build
  
  PLATFORM_OUT="$PLATFORM_DIR/dist"
  [ -d "$PLATFORM_OUT" ] || die "Platform build failed — output directory not found at $PLATFORM_OUT"
  ok "Platform static export → $PLATFORM_OUT"
fi

# ── 2. Build Gizzi Code Binary ───────────────────────────────────────────────
step "Building gizzi-code binary…"
# Detect target triple for this machine
ARCH=$(uname -m | sed 's/arm64/arm64/;s/x86_64/x64/')
OS=$(uname | tr '[:upper:]' '[:lower:]' | sed 's/darwin/darwin/')
GIZZI_TARGET="${OS}-${ARCH}"   # e.g. darwin-arm64

# bun install inside cmd/gizzi-code fails: it is a pnpm workspace package
# (workspace:* deps). Match CI (gizzi-code-quality.yml): install/build from
# the repo root, then compile the binary.
cd "$WORKSPACE_ROOT"
pnpm --filter "@allternit/gizzi-code^..." run build
pnpm --filter "@allternit/gizzi-sdk" run build
cd "$GIZZI_DIR"
bun run script/build-production.js --target="$GIZZI_TARGET"

GIZZI_BIN="$GIZZI_DIR/dist/gizzi-code"
# The build script might suffix it with the target, check both
if [ ! -f "$GIZZI_BIN" ]; then
    GIZZI_BIN="$GIZZI_DIR/dist/gizzi-code-$GIZZI_TARGET"
fi

[ -f "$GIZZI_BIN" ] || die "gizzi-code build failed — binary not found at $GIZZI_BIN"

mkdir -p "$RESOURCES_DIR/bin"
cp "$GIZZI_BIN" "$RESOURCES_DIR/bin/gizzi-code"
chmod +x "$RESOURCES_DIR/bin/gizzi-code"
ok "gizzi-code → $RESOURCES_DIR/bin/gizzi-code"

# ── 2a. Vendor allternit-mux (PTY daemon gizzi auto-spawns for /pty) ────────
step "Vendoring allternit-mux…"
(cd "$WORKSPACE_ROOT" && cargo build --release -p allternit-mux)
MUX_BIN="$CARGO_OUT/release/allternit-mux"
[ -f "$MUX_BIN" ] || die "allternit-mux build failed — binary not found at $MUX_BIN"
cp "$MUX_BIN" "$RESOURCES_DIR/bin/allternit-mux"
chmod +x "$RESOURCES_DIR/bin/allternit-mux"
ok "allternit-mux → $RESOURCES_DIR/bin/allternit-mux"

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

# ── 2b. Build Voice Service Sidecar (Rust + whisper.cpp) ────────────────────
step "Building bundled voice service (whisper.cpp)…"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
cd "$WORKSPACE_ROOT"
cargo build --release -p voice-service
VOICE_BIN="$CARGO_OUT/release/voice-service"
[ -f "$VOICE_BIN" ] || VOICE_BIN="$CARGO_OUT/release/allternit-voice-service"
[ -f "$VOICE_BIN" ] || die "Voice service build failed — binary not found at $VOICE_BIN"
cp "$VOICE_BIN" "$RESOURCES_DIR/bin/allternit-voice-service"
chmod +x "$RESOURCES_DIR/bin/allternit-voice-service"

bash "$VOICE_DIR/build-whisper.sh"
WHISPER_CLI="$VOICE_DIR/dist/whisper-cli"
[ -x "$WHISPER_CLI" ] || die "whisper-cli build failed — binary not found at $WHISPER_CLI"
cp "$WHISPER_CLI" "$RESOURCES_DIR/bin/whisper-cli"
chmod +x "$RESOURCES_DIR/bin/whisper-cli"

ok "voice service → $RESOURCES_DIR/bin/allternit-voice-service + whisper-cli"

# ── 3. Build Rust API ────────────────────────────────────────────────────────
if [ "$SKIP_API" = false ]; then
  step "Building allternit-api (Rust)…"
  cd "$API_DIR"
  cargo build --release
  
  # Map binary name (Cargo uses underscores, we prefer dashes for distribution)
  # In workspace builds, binary is in the root target dir
  API_BIN="$CARGO_OUT/release/allternit-api"
  [ -f "$API_BIN" ] || API_BIN="$CARGO_OUT/release/allternit_api"
  [ -f "$API_BIN" ] || API_BIN="$API_DIR/target/release/allternit-api"
  [ -f "$API_BIN" ] || API_BIN="$API_DIR/target/release/allternit_api"
  [ -f "$API_BIN" ] || die "API build failed — binary not found at $API_BIN"

  mkdir -p "$RESOURCES_DIR/bin"
  cp "$API_BIN" "$RESOURCES_DIR/bin/allternit-api"
  chmod +x "$RESOURCES_DIR/bin/allternit-api"
  ok "allternit-api → $RESOURCES_DIR/bin/allternit-api"

  # 3a. Local model engine (services/local-engine) backing Model Lab telemetry.
  step "Building local-engine (Rust)…"
  LOCAL_ENGINE_DIR="$WORKSPACE_ROOT/services/local-engine"
  (cd "$LOCAL_ENGINE_DIR" && cargo build --release)

  # Cargo names the bin `local-engine` (see [[bin]] in its Cargo.toml); stage
  # it as allternit-local-engine so resources/bin keeps one naming convention.
  LOCAL_ENGINE_BIN="$CARGO_OUT/release/local-engine"
  [ -f "$LOCAL_ENGINE_BIN" ] || LOCAL_ENGINE_BIN="$LOCAL_ENGINE_DIR/target/release/local-engine"
  [ -f "$LOCAL_ENGINE_BIN" ] || die "local-engine build failed — binary not found at $LOCAL_ENGINE_BIN"

  cp "$LOCAL_ENGINE_BIN" "$RESOURCES_DIR/bin/allternit-local-engine"
  chmod +x "$RESOURCES_DIR/bin/allternit-local-engine"
  ok "local-engine → $RESOURCES_DIR/bin/allternit-local-engine"
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

  # Build the renderer and main process
  pnpm run build

  # Package with electron-builder
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

step "Patching SHA256 checksums into $MANIFEST_FILE…"

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
