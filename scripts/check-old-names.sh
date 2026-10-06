#!/usr/bin/env bash
# check-old-names.sh: keep the pre-Factory names out of tracked files.
#
# The Allternit Factory replaced CommRails, allternit-rails, the `ao` engine and
# the ao-* scripts (SPEC §12). Old names are removed, not aliased. This greps
# every tracked text file for them and fails if any turn up outside the
# allowlist. The same script lives in allternit-platform and allternit-ai;
# keep the two copies identical (each repo has its own allowlist).
#
#   scripts/check-old-names.sh           check the repo (exit 0 pass, 1 fail)
#   scripts/check-old-names.sh --list    print the patterns and the allowlist
#   OLD_NAMES_MAX=0 scripts/check-old-names.sh   print every hit, not the first 40 per pattern
#
# Two ways to keep a hit, both reviewed in PRs:
#   - scripts/check-old-names.allow: one git pathspec glob per line, then an
#     optional `@<pattern-id>` (the entry then allows only that pattern),
#     then `# reason`. For history (CHANGELOG, dated notes), the docs
#     migration page, migrations, and vendored upstream code.
#   - the marker `old-names: keep` on the same line, for one line that must
#     name the old thing (a migration reading an old env var, a stored data
#     value, a ledger event name).
#
# Ledger event names and the `.allternit/` data layout are data, not names,
# and are never renamed (old ledgers must replay). The route pattern doesn't
# match `.allternit/rails/…` paths for that reason.
set -euo pipefail
cd "$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"

allow_file="scripts/check-old-names.allow"
marker='old-names: keep'

# id|label|extended regex (matched against the line text, not the path).
patterns=(
  'commrails|CommRails (any case)|[Cc][Oo][Mm][Mm][Rr][Aa][Ii][Ll][Ss]'
  'rails-bin|allternit-rails binary|allternit[-_]rails([^A-Za-z0-9_]|$)'
  'rails-env|old env ALLTERNIT_RAILS_*|ALLTERNIT_RAILS_[A-Z]'
  'gizzi-rails-env|old env GIZZI_RAILS_*|GIZZI_(COMM)?RAILS_[A-Z]'
  'ao-env|old env AO_*|(^|[^A-Za-z0-9_])AO_[A-Z]'
  'herdr-env|old env HERDR_* (outside the pane engine)|(^|[^A-Za-z0-9_])HERDR_[A-Z]'
  'rails-route|old route /rails, /api/rails|(^|[^A-Za-z0-9._~-])/(api/)?rails([/"'"'"'`?#) ]|$)'
  'rails-cli|internal rails CLI|internal (verb-)?rails([^A-Za-z0-9_-]|$)|verb-rails'
  'ao-scripts|ao-* scripts|(^|[^A-Za-z0-9_.-])ao-(spawn|send|watch|kill|status|doctor|consult|registry-sync|spawn-gate|engine|queue|recover|transcript)([^A-Za-z0-9_-]|$)'
  'ao-cli|ao CLI usage|(usage: ao |`ao (spawn|send|watch|status|kill|doctor|queue|serve|harness|peer|fabric)[ `])'
  'old-dirs|~/.ao and ~/.agent-orchestrator dirs|~/\.(ao|agent-orchestrator)([/"'"'"'` ]|$)'
)

# Allowlist: global excludes go to git grep; tagged ones filter per pattern.
excludes=()
tagged=()   # "id<TAB>glob"
if [ -f "$allow_file" ]; then
  while IFS= read -r line; do
    entry="${line%%#*}"
    read -r glob tag _ <<<"$entry" || true
    [ -n "${glob:-}" ] || continue
    if [ -n "${tag:-}" ]; then
      tagged+=("${tag#@}"$'\t'"$glob")
    else
      excludes+=(":(exclude,glob)$glob")
    fi
  done < "$allow_file"
fi

if [ "${1:-}" = "--list" ]; then
  printf 'patterns (id | label | regex):\n'; printf '  %s\n' "${patterns[@]}"
  printf '\nallowlist (%s):\n' "$allow_file"; grep -vE '^[[:space:]]*(#|$)' "$allow_file" | sed 's/^/  /'
  exit 0
fi

# One pass over the tree with every pattern, then sort the hits by pattern.
grep_args=()
for entry in "${patterns[@]}"; do rest="${entry#*|}"; grep_args+=(-e "${rest#*|}"); done
all_hits="$(git grep -nIE "${grep_args[@]}" -- . "${excludes[@]}" 2>/dev/null | grep -vF "$marker" || true)"

max="${OLD_NAMES_MAX:-40}"
[ "$max" = 0 ] && max=1000000
fail=0
for entry in "${patterns[@]}"; do
  id="${entry%%|*}"
  rest="${entry#*|}"
  label="${rest%%|*}"
  re="${rest#*|}"
  globs=""
  for t in ${tagged[@]+"${tagged[@]}"}; do
    [ "${t%%$'\t'*}" = "$id" ] && globs+="${t#*$'\t'}"$'\n'
  done
  hits="$(printf '%s\n' "$all_hits" | RE="$re" GLOBS="$globs" awk '
    function glob2re(g,   r, i, c) {
      r = "^"
      for (i = 1; i <= length(g); i++) {
        c = substr(g, i, 1)
        if (c == "*") { if (substr(g, i + 1, 1) == "*") { r = r ".*"; i++ } else r = r "[^/]*" }
        else if (c == "?") r = r "[^/]"
        else if (index(".+()|^$[]{}\\", c)) r = r "\\" c
        else r = r c
      }
      return r "$"
    }
    BEGIN { n = split(ENVIRON["GLOBS"], gl, "\n"); for (i = 1; i <= n; i++) if (gl[i] != "") res[i] = glob2re(gl[i]) }
    {
      line = $0; t = line; sub(/^[^:]*:[0-9]+:/, "", t)
      if (t !~ ENVIRON["RE"]) next
      path = line; sub(/:[0-9]+:.*$/, "", path)
      for (i in res) if (path ~ res[i]) next
      print line
    }' || true)"
  if [ -n "$hits" ]; then
    n="$(printf '%s\n' "$hits" | wc -l | tr -d ' ')"
    echo "FAIL [$id]: $label ($n line(s))"
    printf '%s\n' "$hits" | cut -c1-200 | sed 's/^/  /' | head -"$max" || true
    if [ "$n" -gt "$max" ]; then echo "  … and $((n - max)) more (OLD_NAMES_MAX=0 shows all)"; fi
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "Old names found. Use the Factory names (gizzi agents|orchestration|workflows|workspace,"
  echo "allternit-factory, ALLTERNIT_FACTORY_*, /api/factory). If a hit is history, a migration"
  echo "or data, allowlist it in $allow_file or mark the line '$marker', with a reason."
  exit 1
fi
echo "PASS: no old names outside the allowlist (scripts/check-old-names.sh)"
