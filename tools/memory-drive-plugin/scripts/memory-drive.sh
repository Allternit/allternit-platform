#!/usr/bin/env bash
# Allternit Memory Drive helper for external agents (Claude Code, Codex, …).
# Format: Agent Memory Repo SPEC.md (MIT). The token is read from the
# environment by a git credential helper; it never appears in argv or URLs.
set -euo pipefail

DIR="${ALLTERNIT_MEMORY_DIR:-$HOME/.allternit/memory-drive}"
BRANCH=main
MAX_TRIES=5

die() { echo "memory-drive: $*" >&2; exit 1; }

g() {
  # shellcheck disable=SC2016
  git -C "$DIR" -c credential.helper= \
    -c 'credential.helper=!f() { test "$1" = get || exit 0; echo username=x-access-token; echo "password=${ALLTERNIT_MEMORY_TOKEN:?set ALLTERNIT_MEMORY_TOKEN}"; }; f' \
    -c user.name="${ALLTERNIT_MEMORY_AUTHOR:-Agent via Allternit}" \
    -c user.email=memory@allternit.invalid "$@"
}

looks_secret() {
  grep -Eiq '(sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{8,}|gho_[A-Za-z0-9]{8,}|xox[bp]-|AKIA[0-9A-Z]{12,}|-----BEGIN [A-Z ]*PRIVATE KEY|allternit_git_[0-9a-f]{8,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.|password|passcode|api[ _]?key|secret key|access token|private key|seed phrase|recovery phrase)' <<<"$1"
}

cmd_clone() {
  [ -n "${ALLTERNIT_MEMORY_URL:-}" ] || die "set ALLTERNIT_MEMORY_URL to the clone URL from Settings → Memory"
  case "$ALLTERNIT_MEMORY_URL" in *@*) die "the clone URL must not contain credentials";; esac
  if [ -d "$DIR/.git" ]; then echo "already cloned at $DIR"; return; fi
  mkdir -p "$(dirname "$DIR")"
  # shellcheck disable=SC2016
  git -c credential.helper= \
    -c 'credential.helper=!f() { test "$1" = get || exit 0; echo username=x-access-token; echo "password=${ALLTERNIT_MEMORY_TOKEN:?set ALLTERNIT_MEMORY_TOKEN}"; }; f' \
    clone --branch "$BRANCH" "$ALLTERNIT_MEMORY_URL" "$DIR"
}

sync_latest() {
  g fetch --quiet origin "$BRANCH"
  # The checkout is dedicated to the drive; local state always follows the
  # server (never the other way: we never force-push).
  g reset --quiet --hard "origin/$BRANCH"
}

cmd_load() {
  [ -d "$DIR/.git" ] || die "not cloned yet; run: $0 clone"
  sync_latest 2>/dev/null || echo "memory-drive: offline, showing the last synced copy" >&2
  cat "$DIR/MEMORY.md"
}

valid_path() {
  case "$1" in
    *..*|/*|.*|*/.*|twin/*|cowork/*|MEMORY.md) return 1;;
    *.md) [[ "$1" =~ ^[A-Za-z0-9._/-]+$ ]];;
    *) return 1;;
  esac
}

apply_line() { # file line id
  local file="$DIR/$1" line="$2" id="$3" topic="${1%.md}"
  mkdir -p "$(dirname "$file")"
  if [ ! -f "$file" ]; then
    printf '# %s\n\n' "$(basename "$topic")" >"$file"
  fi
  if ! grep -qF "; id: $id]" "$file"; then
    printf '%s\n' "$line" >>"$file"
  fi
  if ! grep -qxF -- "- [[$topic]]" "$DIR/MEMORY.md"; then
    grep -qx '## Index' "$DIR/MEMORY.md" || printf '\n## Index\n' >>"$DIR/MEMORY.md"
    printf -- '- [[%s]]\n' "$topic" >>"$DIR/MEMORY.md"
  fi
}

cmd_remember() { # file text source
  [ $# -eq 3 ] || die "usage: $0 remember <topic.md> <text> <source>"
  local path="$1" text="$2" source="$3"
  [ -d "$DIR/.git" ] || die "not cloned yet; run: $0 clone"
  valid_path "$path" || die "use a topic file like facts.md (not MEMORY.md, twin/ or cowork/)"
  text="$(tr -s '[:space:]' ' ' <<<"$text" | sed 's/^ //; s/ $//')"
  [ -n "$text" ] || die "empty memory"
  [ ${#text} -le 4096 ] || die "keep a memory under 4096 characters"
  [[ "$text" =~ \ \[[A-Za-z_][A-Za-z0-9_-]*: ]] && die "the text can't contain [key: …] metadata"
  [[ "$source" =~ ^[A-Za-z0-9._~/:@=\&?+%-]+$ ]] || die "source must be a session link or label without spaces"
  if looks_secret "$text"; then die "that looks like a credential; it was not saved"; fi
  local id line
  id="m-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
  line="- $text [source: $source; added: $(date -u +%Y-%m-%d); id: $id]"
  local try=1
  while :; do
    sync_latest || die "can't reach the drive; nothing was saved"
    apply_line "$path" "$line" "$id"
    g add -A
    g commit --quiet -m "Remember: ${text:0:60}" || true
    if g push --quiet origin "HEAD:$BRANCH" 2>/tmp/memory-drive-push.$$; then
      rm -f /tmp/memory-drive-push.$$
      echo "saved to $path"
      return
    fi
    if grep -q 'refused this push' /tmp/memory-drive-push.$$; then
      sed 's/^remote: //' /tmp/memory-drive-push.$$ >&2; rm -f /tmp/memory-drive-push.$$
      sync_latest || true
      die "the drive refused this memory"
    fi
    rm -f /tmp/memory-drive-push.$$
    [ $try -lt $MAX_TRIES ] || die "the drive kept changing; try again"
    try=$((try + 1))
    sleep $((try))
  done
}

case "${1:-}" in
  clone) shift; cmd_clone "$@";;
  load) shift; cmd_load "$@";;
  remember) shift; cmd_remember "$@";;
  *) echo "usage: $0 clone | load | remember <topic.md> <text> <source>" >&2; exit 2;;
esac
