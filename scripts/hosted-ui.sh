# Resolve the ai.allternit.com workspace UI (private Gizziio/allternit-ai).
# Not the cloud console (platform.allternit.com).
#
# Usage:  HOSTED_UI="$(. scripts/hosted-ui.sh; resolve_hosted_ui "$REPO_ROOT")"
# Env:    ALLTERNIT_AI_PATH  — explicit checkout (else a clean origin/main
#         worktree at ../allternit-ai-wt-desktop-build, created/refreshed here)

resolve_hosted_ui() {
  local root="${1:?repo root}"
  if [ -n "${ALLTERNIT_AI_PATH:-}" ] && [ -f "${ALLTERNIT_AI_PATH}/package.json" ]; then
    (cd "$ALLTERNIT_AI_PATH" && pwd)
    return 0
  fi
  if [ -f "$root/.hosted-ui/package.json" ]; then
    (cd "$root/.hosted-ui" && pwd)
    return 0
  fi
  # Default: a dedicated clean worktree on origin/main — never the shared
  # allternit-ai checkout itself. That checkout sits on whatever commit and
  # WIP other sessions left (once 164 commits behind), and packaging it made
  # every Desktop build "revert" merged UI fixes.
  local source="$root/../allternit-ai"
  if [ -d "$source/.git" ] || [ -f "$source/.git" ]; then
    ensure_ui_build_worktree "$source" "$root/../allternit-ai-wt-desktop-build"
    return $?
  fi
  echo "ai.allternit.com UI not found." >&2
  echo "Clone Gizziio/allternit-ai next to this repo, or set ALLTERNIT_AI_PATH." >&2
  echo "Do not use surfaces/platform.allternit.com — that is the cloud console." >&2
  return 1
}

# Create or refresh <wt> as a detached, clean worktree of <source> at
# origin/main, install its dependencies when the lockfile changed, and print
# its path. Refuses to touch a worktree with uncommitted changes.
ensure_ui_build_worktree() {
  local source="${1:?source checkout}" wt="${2:?worktree path}"
  git -C "$source" fetch --quiet origin main >&2 || echo "note: could not fetch origin/main; using the last fetch" >&2
  if [ ! -e "$wt/.git" ]; then
    git -C "$source" worktree add --quiet --detach "$wt" origin/main >&2 || return 1
  else
    if [ -n "$(git -C "$wt" status --porcelain --untracked-files=no)" ]; then
      echo "UI build worktree $wt has uncommitted changes; not resetting it. Clean it or set ALLTERNIT_AI_PATH." >&2
      return 1
    fi
    git -C "$wt" checkout --quiet --detach origin/main >&2 || return 1
  fi
  local lock_hash stamp="$wt/node_modules/.lock-hash"
  lock_hash="$(shasum "$wt/pnpm-lock.yaml" | cut -d' ' -f1)"
  if [ ! -f "$stamp" ] || [ "$(cat "$stamp")" != "$lock_hash" ]; then
    (cd "$wt" && pnpm install --frozen-lockfile --ignore-scripts >&2) || return 1
    echo "$lock_hash" > "$stamp"
  fi
  (cd "$wt" && pwd)
}

link_oss_platform() {
  local hosted="${1:?hosted ui dir}"
  local oss="${2:?oss platform repo root}"
  ln -sfn "$oss" "$hosted/.oss-platform"
}
