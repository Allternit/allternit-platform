#!/usr/bin/env bash
# Heuristic: every Settings route page must render through SPage (or DockingBlockTitle / ProvideDockScroll),
# so its title docks into the bar like iOS. Prints routes off the standard; the last line is the count.
set -u
cd "$(dirname "$0")/.."
SRC=feature/settings/src/main/java/com/allternit/feature/settings
SHELL_SRC=feature/shell/src/main/java
ROOT="$SRC/SettingsRoot.kt"
bad=0
routes=$(sed -n '/^        when (page) {/,/else -> SPlaceholder/p' "$ROOT" | grep -E '^[[:space:]]+"[^"]+" ->')
while IFS= read -r line; do
  route=$(sed -E 's/^[[:space:]]*"([^"]+)".*/\1/' <<<"$line")
  call=$(sed -E 's/^[[:space:]]*"[^"]+" -> //' <<<"$line")
  if [[ "$call" == host.connections* ]]; then
    fn=ConnectionsSettingsScreen
    f=$(grep -rl "fun $fn" "$SHELL_SRC" | head -1)
  else
    fn=$(sed -E 's/^([A-Za-z0-9_]+).*/\1/' <<<"$call")
    f=$(grep -lE "fun $fn\b" "$SRC"/*.kt | head -1)
  fi
  if [[ -z "${f:-}" ]]; then echo "OFF  $route -> $fn (definition not found)"; bad=$((bad+1)); continue; fi
  # the function body: from its declaration to the next top-level declaration
  body=$(awk -v fn="fun $fn" 'index($0,fn){on=1;print;next} on&&/^(internal |private |public )?(fun|val|class|object) /{exit} on{print}' "$f")
  # ResourcePage / ResourcePicker are themselves SPage wrappers (ResourcePages.kt)
  if grep -qE 'SPage\(|DockingBlockTitle\(|ProvideDockScroll\(|ResourcePage\(|ResourcePicker\(' <<<"$body"; then continue; fi
  echo "OFF  $route -> $fn ($f)"; bad=$((bad+1))
done <<<"$routes"
echo "off-standard routes: $bad"
