#!/usr/bin/env bash
# install.sh — symlink the steering toolkit onto PATH.
# Idempotent; safe to re-run after `git pull`.
#
# What it installs into ~/.local/bin:
#   steer-*   (parallel-session steering: discover/context/checkpoint/prompt/verify + steer dispatcher)
#
# Agent orchestration itself is the Allternit Factory: `gizzi agents|orchestration|
# workflows|workspace` (the engine, `allternit-factory`, ships with Desktop and
# gizzi-code). There is nothing to install for it here.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPTS_DIR="$HERE/scripts"
BIN_DIR="${ALLTERNIT_BIN_DIR:-$HOME/.local/bin}"

mkdir -p "$BIN_DIR"

installed=()
for script in "$SCRIPTS_DIR"/steer*; do
  [ -f "$script" ] || continue
  name=$(basename "$script")
  chmod +x "$script"
  ln -sf "$script" "$BIN_DIR/$name"
  installed+=("$name")
done

count=${#installed[@]}
echo "installed $count tools into $BIN_DIR: ${installed[*]:-none}"

# Sanity: PATH check
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on PATH — add it to your shell profile" >&2 ;;
esac
