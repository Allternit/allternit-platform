#!/bin/bash
# Automated maintenance cleanup for the allternit workspace.
# Safe to run while agents are working: it only removes stale artifacts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PROJECT_ROOT="${WORKSPACE_ROOT}/allternit"
LOG_FILE="${WORKSPACE_ROOT}/.maintenance-cleanup.log"

log() {
    echo "[$(date -u +"%Y-%m-%dT%H:%M:%SZ")] $1" | tee -a "${LOG_FILE}"
}

log "Starting maintenance cleanup"
log "Workspace root: ${WORKSPACE_ROOT}"

# Report disk usage before
BEFORE=$(df -h /System/Volumes/Data | awk 'NR==2 {print $4}')
BEFORE_PCT=$(df -h /System/Volumes/Data | awk 'NR==2 {print $5}')
log "Disk free before: ${BEFORE} (${BEFORE_PCT})"

# 1. Remove orphaned standalone Rust target directories.
# The main workspace target is intentionally left alone because agents are actively using it.
log "Cleaning standalone Rust targets..."
rm -rf "${PROJECT_ROOT}/cmd/launcher/target"
rm -rf "${PROJECT_ROOT}/factory/engine/target"
rm -rf "${WORKSPACE_ROOT}/distribution/launcher-desktop/target"
rm -rf "${WORKSPACE_ROOT}/distribution/launcher-simplified/target"

# 2. Sweep the main workspace target for artifacts older than 14 days.
log "Sweeping Rust artifacts older than 14 days..."
cd "${PROJECT_ROOT}"
if command -v cargo-sweep >/dev/null 2>&1; then
    cargo sweep -r -t 14 . || log "cargo-sweep completed with warnings"
else
    log "cargo-sweep not installed; skipping artifact sweep"
fi

# 2b. If the target is still too large or disk is tight, cap it by removing
# oldest artifacts first. This preserves recently-used artifacts.
TARGET_SIZE_MB=$(du -sm "${PROJECT_ROOT}/target" | cut -f1)
DISK_FREE_GB=$(df -g /System/Volumes/Data | awk 'NR==2 {print $4}')
MAX_TARGET_MB=25600  # 25 GB
MIN_FREE_GB=15
if command -v cargo-sweep >/dev/null 2>&1 && { [ "${TARGET_SIZE_MB}" -gt "${MAX_TARGET_MB}" ] || [ "${DISK_FREE_GB}" -lt "${MIN_FREE_GB}" ]; }; then
    log "Target is ${TARGET_SIZE_MB}MB / disk free is ${DISK_FREE_GB}GB; capping target at 25GB..."
    cargo sweep -r --maxsize 25GB . || log "cargo-sweep maxsize completed with warnings"
fi

# 3. Prune the pnpm store.
log "Pruning pnpm store..."
if command -v pnpm >/dev/null 2>&1; then
    pnpm store prune || log "pnpm store prune completed with warnings"
else
    log "pnpm not installed; skipping store prune"
fi

# 4. Clean the global shared target if it exists.
# This is a raw target directory, not a Cargo project, so we delete files
# older than 14 days directly. It only contains reproducible build artifacts.
log "Sweeping global shared target..."
if [ -d "${HOME}/.cargo/shared-target" ]; then
    GLOBAL_BEFORE=$(du -sh "${HOME}/.cargo/shared-target" | cut -f1)
    find "${HOME}/.cargo/shared-target" -type f -mtime +14 -delete 2>/dev/null || true
    find "${HOME}/.cargo/shared-target" -type d -empty -delete 2>/dev/null || true
    GLOBAL_AFTER=$(du -sh "${HOME}/.cargo/shared-target" | cut -f1)
    log "Global shared target: ${GLOBAL_BEFORE} -> ${GLOBAL_AFTER}"
fi

# Report disk usage after
AFTER=$(df -h /System/Volumes/Data | awk 'NR==2 {print $4}')
AFTER_PCT=$(df -h /System/Volumes/Data | awk 'NR==2 {print $5}')
log "Disk free after: ${AFTER} (${AFTER_PCT})"
log "Maintenance cleanup finished"
