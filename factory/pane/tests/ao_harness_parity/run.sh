#!/usr/bin/env bash
# Byte-parity harness: Allternit ops harness-sync — JS reference implementation
# vs `ao harness` (Rust port, P4 of the ao v3 runtime build).
#
# For every scenario this script:
#   1. builds one template fixture tree (scratch HOME + source skills +
#      fixture manifest + fake tool binaries),
#   2. copies it into two identical trees,
#   3. runs the REAL JS (`harness-sync.js`, copied unmodified next to a fixture
#      `harness.json`) against one tree and `ao harness` (with
#      AO_HARNESS_MANIFEST pointing at the same fixture manifest) against the
#      other, under identical HOME/PATH/LC_ALL,
#   4. diffs stdout, stderr, exit codes, and the full resulting trees
#      (normalizing only `syncedAt` in .allternit-harness.json files).
#
# Scenarios cover: cold sync (all 16 tools present), warm status, idempotent
# re-sync, dry-run sync, uninstall, uninstall dry-run, --tools filter,
# absent tools (skipped rows), pre-seeded broken/drift states, cursor
# frontmatter, commented .jsonc, symlinked source skill, localeCompare-order
# hash collisions, flat skillsFormat, error exits, plus two conformance gates
# and a live smoke gate (real $HOME, read-only).
#
# Usage: run.sh [-v] [--skip-live]   (builds the ao binary unless AO_BIN is set)
set -uo pipefail

PARITY_DIR="$(cd "$(dirname "$0")" && pwd)"
AOE_ROOT="$(cd "$PARITY_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$PARITY_DIR/../../../../.." && pwd)"
BRAIN_OPS="${BRAIN_OPS:-$HOME/Desktop/Allternit/Allternit Brain/Ops}"
VERBOSE=0
SKIP_LIVE=0
for arg in "$@"; do
  [ "$arg" = "-v" ] && VERBOSE=1
  [ "$arg" = "--skip-live" ] && SKIP_LIVE=1
done

export PATH="/opt/homebrew/opt/zig@0.15/bin:$PATH"
NODE_BIN="$(command -v node)" || { echo "SKIP: node not found"; exit 2; }
export LC_ALL=en_US.UTF-8

if [ -z "${AO_BIN:-}" ]; then
  AO_BIN="$REPO_ROOT/target/debug/ao"
  if [ ! -x "$AO_BIN" ]; then
    echo "building ao binary..." >&2
    (cd "$REPO_ROOT" && cargo build -p herdr --bin ao) >&2 || { echo "BUILD FAILED"; exit 2; }
  fi
fi

TDIR="$(mktemp -d /tmp/ao-harness-parity-XXXXXX)"
PASS=0
FAIL=0
# Per-scenario overrides (set before calling run_pair).
SCEN_PATH=""
SCEN_MANIFEST=""

cleanup() {
  if [ "$FAIL" -eq 0 ] && [ -z "${KEEP_TMP:-}" ]; then
    rm -rf "$TDIR"
  else
    echo "scratch kept at $TDIR (FAIL=$FAIL)"
  fi
}
trap cleanup EXIT

pass() { PASS=$((PASS + 1)); echo "PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $1"; }

# ---------------------------------------------------------------------------
# Fixture builders
# ---------------------------------------------------------------------------

# Synthetic source skills with case-torture siblings (localeCompare stress),
# nested dirs, and a symlinked skill dir.
build_src_skills() {
  local src="$1"
  mkdir -p "$src/alpha"
  printf '# Alpha\n\nAlpha skill body.\n' > "$src/alpha/SKILL.md"

  mkdir -p "$src/billing-agent/scripts" "$src/billing-agent/docs"
  printf '# Billing Agent\n' > "$src/billing-agent/SKILL.md"
  printf 'print("billing")\n' > "$src/billing-agent/scripts/compute_hours.py"
  printf 'placeholder\n' > "$src/billing-agent/scripts/screenshot.js"
  printf '# Billing notes\n' > "$src/billing-agent/docs/CHANGELOG.md"

  mkdir -p "$src/client-report"
  printf '# Client Report\n' > "$src/client-report/SKILL.md"
  printf 'render\n' > "$src/client-report/render_quote.py"
  printf 'shot\n' > "$src/client-report/screenshot.js"

  mkdir -p "$src/deploy"
  printf '# Deploy\n' > "$src/deploy/SKILL.md"

  mkdir -p "$src/quote"
  printf '# Quote\n' > "$src/quote/SKILL.md"

  mkdir -p "$TDIR/skill-targets/link-skill"
  printf '# Linked Skill\n' > "$TDIR/skill-targets/link-skill/SKILL.md"
  ln -s "$TDIR/skill-targets/link-skill" "$src/link-skill"
}

build_rules_file() {
  # Trailing whitespace + multiple trailing newlines: the rules block builder
  # trimEnds, so this exercises the normalization.
  printf '  # Allternit Harness Rules\n\nDo the things. Follow the gates.  \n\n\n' > "$1"
}

# Fixture manifest: the REAL Ops/harness.json with only the machine-specific
# absolute paths rewritten into the scratch tree (tool cfg paths are
# ~-relative and resolve against scratch HOME automatically).
build_fixture_manifest() {
  local src="$1" out="$2"
  AO_PARITY_MCP_PATH="$TDIR/ops/index.js" "$NODE_BIN" -e '
    const fs = require("fs");
    const [src, out, mcpPath] = process.argv.slice(1);
    const m = JSON.parse(fs.readFileSync(src, "utf8"));
    m.source.skillsDir = `${process.env.TDIR_REF}/src-skills`;
    m.source.rulesFile = `${process.env.TDIR_REF}/rules.md`;
    m.source.mcpServer.args = [mcpPath];
    for (const key of ["grok", "agy", "opencode", "qwen"]) {
      const args = m.tools[key].mcp.addArgs;
      for (let i = 0; i < args.length; i++) {
        if (args[i].endsWith("Ops/index.js")) args[i] = mcpPath;
      }
    }
    fs.writeFileSync(out, JSON.stringify(m, null, 2) + "\n");
  ' "$src" "$out" "$TDIR/ops/index.js"
}

# Fake cli-kind tool binaries: deterministic file effects no matter which side
# execs them. They implement just enough of `mcp add`/`mcp remove` to mirror
# the real CLIs' config-file effects.
build_fake_bins() {
  local bin="$1"
  mkdir -p "$bin" "$TDIR/fake-lib"

  for tool in agy opencode qwen codebuddy workbuddy openclaw hermes dsh qoder grok; do
    printf '#!/bin/bash\nexec bash "$AO_FAKE_LIB/%s.sh" "$@"\n' "$tool" > "$bin/$tool"
    chmod +x "$bin/$tool"
  done
  export AO_FAKE_LIB="$TDIR/fake-lib"

  # Non-cli tools that are only ever DETECTED via which() — never exec'd.
  for tool in codebuddy workbuddy openclaw hermes dsh qoder; do
    printf '#!/bin/bash\nexit 3\n' > "$TDIR/fake-lib/$tool.sh"
  done

  # grok: TOML config at ~/.grok/config.toml.
  cat > "$TDIR/fake-lib/grok.sh" <<'EOF'
#!/bin/bash
set -u
cfg="$HOME/.grok/config.toml"
section="mcp_servers.allternit-ops"
block="[mcp_servers.allternit-ops]
command = \"node\"
args = [\"$AO_PARITY_MCP_PATH\"]"
case "$1 $2" in
  "mcp add")
    mkdir -p "$(dirname "$cfg")"
    [ -f "$cfg" ] || : > "$cfg"
    python3 - "$cfg" "$section" "$block" <<'PY'
import sys
path, section, block = sys.argv[1], sys.argv[2], sys.argv[3].replace("\\n", "\n")
lines = open(path).read().split("\n")
out, i, replaced = [], 0, False
while i < len(lines):
    if lines[i].strip() == f"[{section}]" and not replaced:
        out.append(block.rstrip("\n"))
        i += 1
        while i < len(lines) and not lines[i].startswith("["):
            i += 1
        replaced = True
    else:
        out.append(lines[i]); i += 1
if not replaced:
    open(path, "w").write(open(path).read().rstrip() + "\n\n" + block)
else:
    open(path, "w").write("\n".join(out))
PY
    ;;
  "mcp remove")
    [ -f "$cfg" ] || exit 3
    python3 - "$cfg" "$section" <<'PY'
import sys
path, section = sys.argv[1], sys.argv[2]
lines = open(path).read().split("\n")
out, i, found = [], 0, False
while i < len(lines):
    if lines[i].strip() == f"[{section}]" and not found:
        i += 1
        while i < len(lines) and not lines[i].startswith("["):
            i += 1
        found = True
    else:
        out.append(lines[i]); i += 1
if not found:
    sys.exit(3)
text = "\n".join(out)
while "\n\n\n" in text:
    text = text.replace("\n\n\n", "\n\n")
open(path, "w").write(text)
PY
    ;;
  *) exit 3 ;;
esac
EOF

  # agy / qwen (plain {command, args} shape) and opencode (commandArray shape).
  cat > "$TDIR/fake-lib/agy.sh" <<EOF
#!/bin/bash
exec "$NODE_BIN" "$TDIR/fake-lib/json-cli.mjs" "\$HOME/.gemini/config/mcp_config.json" mcpServers plain "\$@"
EOF
  cat > "$TDIR/fake-lib/qwen.sh" <<EOF
#!/bin/bash
exec "$NODE_BIN" "$TDIR/fake-lib/json-cli.mjs" "\$HOME/.qwen/settings.json" mcpServers plain "\$@"
EOF
  cat > "$TDIR/fake-lib/opencode.sh" <<EOF
#!/bin/bash
exec "$NODE_BIN" "$TDIR/fake-lib/json-cli.mjs" "\$HOME/.config/opencode/opencode.jsonc" mcp array "\$@"
EOF

  cat > "$TDIR/fake-lib/json-cli.mjs" <<'EOF'
// Fake json-config cli tool: <configPath> <serversKey> <shape> mcp <add|remove> <name> [...]
import fs from "node:fs";
import path from "node:path";
const [configPath, serversKey, shape, mcp, verb, name] = process.argv.slice(2);
const mcpPath = process.env.AO_PARITY_MCP_PATH;
if (mcp !== "mcp" || !mcpPath) process.exit(3);
let obj = {};
try { obj = JSON.parse(fs.readFileSync(configPath, "utf8")); } catch {}
obj[serversKey] = obj[serversKey] || {};
if (verb === "add") {
  if (shape === "array") {
    obj[serversKey][name] = { type: "local", command: ["node", mcpPath] };
  } else {
    obj[serversKey][name] = { command: "node", args: [mcpPath] };
  }
} else if (verb === "remove") {
  if (!obj[serversKey][name]) process.exit(3);
  delete obj[serversKey][name];
} else {
  process.exit(3);
}
fs.mkdirSync(path.dirname(configPath), { recursive: true });
fs.writeFileSync(configPath, JSON.stringify(obj, null, 2) + "\n");
EOF
}

# Pre-seeded per-tool state for the cold-sync scenario. $1 = tree root.
seed_cold_state() {
  local h="$1/home"
  mkdir -p "$h/.claude" "$h/.codex" "$h/.kimi-code" "$h/.grok" "$h/.gizzi" \
           "$h/.gemini/antigravity-cli" "$h/.gemini/config" "$h/.openclaw/workspace" \
           "$h/.config/opencode" "$h/.config/gizzi-code" "$h/.qwen" "$h/.qoder"
  # codex: unrelated TOML content, no harness block; AGENTS.md with user text.
  printf '[profile]\nmodel = "gpt-5"\n' > "$h/.codex/config.toml"
  printf '# My Codex Notes\n\nKeep this.\n' > "$h/.codex/AGENTS.md"
  # kimi: UNSORTED json keys + broken entry; AGENTS.md has markers but stale.
  printf '{"zebra": 1, "mcpServers": {"allternit-ops": {"command": "python", "args": ["/old.js"]}}, "apple": true}\n' \
    > "$h/.kimi-code/mcp.json"
  printf '<!-- allternit-harness:start -->\nOLD RULES\n<!-- allternit-harness:end -->\n' \
    > "$h/.kimi-code/AGENTS.md"
  # gizzi: unsorted keys, no mcp section (commandArray shape on fix).
  printf '{\n  "$schema": "https://gizzi.io/config.json",\n  "model": "kimi-cli/kimi-k3",\n  "small_model": "aliyun-qwen/qwen3.7-flash",\n  "provider": {"aliyun-qwen": {"name": "Qwen"}}\n}\n' \
    > "$h/.config/gizzi-code/gizzi.json"
  # agy: json config with only an unrelated server → cli add path.
  printf '{"mcpServers": {"other": {"command": "python", "args": ["/x.js"]}}}\n' \
    > "$h/.gemini/config/mcp_config.json"
  # opencode: strict-JSON jsonc with unrelated mcp entry → cli add path.
  printf '{\n  "$schema": "https://opencode.ai/config.json",\n  "mcp": {"other": {"type": "local", "command": ["python", "/x.js"]}}\n}\n' \
    > "$h/.config/opencode/opencode.jsonc"
  # qwen: settings.json with an OK harness entry → cli kind short-circuits
  # (status ok → unchanged, NEVER execs the fake bin).
  printf '{\n  "modelProviders": {"openai": [{"id": "local"}]},\n  "mcpServers": {"allternit-ops": {"command": "node", "args": ["%s"]}}\n}\n' \
    "$TDIR/ops/index.js" > "$h/.qwen/settings.json"
  # qoder: stale entry missing args → fix.
  printf '{"window": {"width": 100}, "mcpServers": {"allternit-ops": {"command": "node", "type": "stdio"}}}\n' \
    > "$h/.qoder/settings.json"
  # openclaw: rules file with markers + surrounding content.
  printf 'Header stays.\n\n<!-- allternit-harness:start -->\nOLD\n<!-- allternit-harness:end -->\n\nFooter stays.\n' \
    > "$h/.openclaw/workspace/AGENTS.md"
}

# ---------------------------------------------------------------------------
# Runner helpers
# ---------------------------------------------------------------------------

# run_pair <scenario-name> <seed-fn|-> <verb> [flags...]
run_pair() {
  local name="$1" seed="$2"; shift 2
  local scen_path="${SCEN_PATH:-$FAKEBIN_PATH}"
  local manifest="${SCEN_MANIFEST:-$TDIR/fixture-harness.json}"
  local template="$TDIR/template-$name"
  rm -rf "$template"
  mkdir -p "$template/home"
  if [ "$seed" != "-" ]; then
    "$seed" "$template"
  fi

  cp -RP "$template" "$TDIR/js-$name"
  cp -RP "$template" "$TDIR/rs-$name"

  env HOME="$TDIR/js-$name/home" PATH="$scen_path" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" "$@" \
    > "$TDIR/js-$name.stdout" 2> "$TDIR/js-$name.stderr"
  echo $? > "$TDIR/js-$name.exit"

  env HOME="$TDIR/rs-$name/home" PATH="$scen_path" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    AO_HARNESS_MANIFEST="$manifest" "$AO_BIN" harness "$@" \
    > "$TDIR/rs-$name.stdout" 2> "$TDIR/rs-$name.stderr"
  echo $? > "$TDIR/rs-$name.exit"

  compare "$name" "$@"
}

compare() {
  local name="$1"; shift
  local ok=1
  # The two sides necessarily run in different scratch trees, and action
  # details embed absolute paths (create-dir/append/create/would-run lines).
  # Normalize the tree path prefix on BOTH sides before diffing.
  local side
  for side in js rs; do
    sed -E "s|$TDIR/(js|rs)-[a-z0-9-]+|TREE|g" "$TDIR/$side-$name.stdout" \
      > "$TDIR/$side-$name.stdout.norm"
    sed -E "s|$TDIR/(js|rs)-[a-z0-9-]+|TREE|g" "$TDIR/$side-$name.stderr" \
      > "$TDIR/$side-$name.stderr.norm"
  done
  if ! diff -u "$TDIR/js-$name.stdout.norm" "$TDIR/rs-$name.stdout.norm" > "$TDIR/$name.stdout.diff" 2>&1; then
    ok=0; echo "  stdout diff (js left, rs right):"; head -40 "$TDIR/$name.stdout.diff" | sed 's/^/    /'
  fi
  if ! diff -u "$TDIR/js-$name.stderr.norm" "$TDIR/rs-$name.stderr.norm" > "$TDIR/$name.stderr.diff" 2>&1; then
    ok=0; echo "  stderr diff:"; head -20 "$TDIR/$name.stderr.diff" | sed 's/^/    /'
  fi
  if ! diff -q "$TDIR/js-$name.exit" "$TDIR/rs-$name.exit" > /dev/null; then
    ok=0; echo "  exit diff: js=$(cat "$TDIR/js-$name.exit") rs=$(cat "$TDIR/rs-$name.exit")"
  fi
  normalize_trees "$TDIR/js-$name" "$TDIR/rs-$name"
  if ! diff -r "$TDIR/js-$name.norm" "$TDIR/rs-$name.norm" > "$TDIR/$name.tree.diff" 2>&1; then
    ok=0; echo "  tree diff:"; head -60 "$TDIR/$name.tree.diff" | sed 's/^/    /'
  fi
  if [ "$ok" -eq 1 ]; then pass "$name ($*)"; else fail "$name ($*)"; fi
}

# Copy trees with syncedAt normalized out of .allternit-harness.json files.
normalize_trees() {
  local a="$1.norm" b="$2.norm"
  rm -rf "$a" "$b"
  cp -RP "$1" "$a"
  cp -RP "$2" "$b"
  local tree f
  for tree in "$a" "$b"; do
    find "$tree" -name .allternit-harness.json -type f | while read -r f; do
      sed -i '' 's/"syncedAt": "[^"]*"/"syncedAt": "NORMALIZED"/' "$f"
    done
  done
}

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

echo "scratch: $TDIR" >&2
mkdir -p "$TDIR/ops"
printf '// stub MCP server entry point (never executed by sync/status)\n' > "$TDIR/ops/index.js"

build_src_skills "$TDIR/src-skills"
build_rules_file "$TDIR/rules.md"
TDIR_REF="$TDIR" build_fixture_manifest "$BRAIN_OPS/harness.json" "$TDIR/fixture-harness.json"
build_fake_bins "$TDIR/fakebin"
FAKEBIN_PATH="$TDIR/fakebin:/usr/bin:/bin"
EMPTYBIN="$TDIR/emptybin"
mkdir -p "$EMPTYBIN"

# JS reference, copied UNMODIFIED next to the fixture manifest (the JS
# resolves harness.json relative to its own location).
mkdir -p "$TDIR/js-sync"
cp "$BRAIN_OPS/harness-sync.js" "$TDIR/js-sync/harness-sync.js"
cp -R "$BRAIN_OPS/harness-sync" "$TDIR/js-sync/harness-sync"
cp "$TDIR/fixture-harness.json" "$TDIR/js-sync/harness.json"

# ---------------------------------------------------------------------------
# Conformance gates
# ---------------------------------------------------------------------------

if cmp -s "$BRAIN_OPS/harness.json" "$AOE_ROOT/src/ao/harness/harness.json"; then
  pass "conformance: embedded harness.json is byte-identical to Ops/harness.json"
else
  fail "conformance: embedded harness.json differs from Ops/harness.json"
fi

# Extract key/label/probe sequence from every live JS driver file and diff
# against the checked-in drivers.tsv (catches the gizzi-drift class).
EXTRACT_OK=1
: > "$TDIR/drivers.actual.tsv"
for f in "$BRAIN_OPS"/harness-sync/drivers/*.js; do
  "$NODE_BIN" -e '
    const fs = require("fs");
    const src = fs.readFileSync(process.argv[1], "utf8");
    const key = src.match(/key: .([\w-]+)./)[1];
    const label = src.match(/label: .(.*).,/)[1];
    const probes = [];
    const body = src.match(/installed: \(\) => (.+?),?\n/)[1];
    for (const part of body.split("||")) {
      const m = part.trim().match(/^(exists|which)\((.)(.*)\2\)/);
      probes.push(`${m[1]}:${m[3]}`);
    }
    console.log([key, label, ...probes].join("\t"));
  ' "$f" >> "$TDIR/drivers.actual.tsv" || EXTRACT_OK=0
done
if [ "$EXTRACT_OK" -eq 1 ] && diff -u \
    <(grep -v '^#' "$PARITY_DIR/drivers.tsv" | grep -v '^$' | sort) \
    "$TDIR/drivers.actual.tsv" > "$TDIR/drivers.diff" 2>&1; then
  pass "conformance: driver table matches live JS drivers (16 tools)"
else
  fail "conformance: driver table drifted"; cat "$TDIR/drivers.diff"
fi

# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------

# S1: cold sync — all 16 tools detected, mixed initial states.
run_pair cold-sync seed_cold_state sync

# S2: warm status on the synced trees.
env HOME="$TDIR/js-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
  "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" status > "$TDIR/js-warm-status.stdout" 2> "$TDIR/js-warm-status.stderr"
echo $? > "$TDIR/js-warm-status.exit"
env HOME="$TDIR/rs-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
  AO_HARNESS_MANIFEST="$TDIR/fixture-harness.json" "$AO_BIN" harness status \
  > "$TDIR/rs-warm-status.stdout" 2> "$TDIR/rs-warm-status.stderr"
echo $? > "$TDIR/rs-warm-status.exit"
if diff -u "$TDIR/js-warm-status.stdout" "$TDIR/rs-warm-status.stdout" > "$TDIR/warm-status.diff" 2>&1 \
   && diff -u "$TDIR/js-warm-status.stderr" "$TDIR/rs-warm-status.stderr" >> "$TDIR/warm-status.diff" 2>&1 \
   && diff -q "$TDIR/js-warm-status.exit" "$TDIR/rs-warm-status.exit" > /dev/null; then
  pass "warm status (post-sync trees)"
else
  fail "warm status"; head -40 "$TDIR/warm-status.diff"
fi

# S3: idempotent re-sync on the synced trees; trees must stay identical.
run_pair_on_trees() {
  local name="$1" js_home="$2" rs_home="$3"; shift 3
  env HOME="$js_home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" "$@" > "$TDIR/js-$name.stdout" 2> "$TDIR/js-$name.stderr"
  echo $? > "$TDIR/js-$name.exit"
  env HOME="$rs_home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    AO_HARNESS_MANIFEST="$TDIR/fixture-harness.json" "$AO_BIN" harness "$@" > "$TDIR/rs-$name.stdout" 2> "$TDIR/rs-$name.stderr"
  echo $? > "$TDIR/rs-$name.exit"
  local ok=1
  local side
  for side in js rs; do
    sed -E "s|$TDIR/(js|rs)-[a-z0-9-]+|TREE|g" "$TDIR/$side-$name.stdout" \
      > "$TDIR/$side-$name.stdout.norm"
    sed -E "s|$TDIR/(js|rs)-[a-z0-9-]+|TREE|g" "$TDIR/$side-$name.stderr" \
      > "$TDIR/$side-$name.stderr.norm"
  done
  diff -u "$TDIR/js-$name.stdout.norm" "$TDIR/rs-$name.stdout.norm" > "$TDIR/$name.diff" 2>&1 || ok=0
  diff -u "$TDIR/js-$name.stderr.norm" "$TDIR/rs-$name.stderr.norm" >> "$TDIR/$name.diff" 2>&1 || ok=0
  diff -q "$TDIR/js-$name.exit" "$TDIR/rs-$name.exit" > /dev/null || ok=0
  if [ "$ok" -eq 1 ]; then pass "$name ($*)"; else fail "$name ($*)"; head -40 "$TDIR/$name.diff"; fi
}
run_pair_on_trees resync "$TDIR/js-cold-sync/home" "$TDIR/rs-cold-sync/home" sync
normalize_trees "$TDIR/js-cold-sync" "$TDIR/rs-cold-sync"
if diff -r "$TDIR/js-cold-sync.norm" "$TDIR/rs-cold-sync.norm" > "$TDIR/resync.tree.diff" 2>&1; then
  pass "trees still identical after re-sync"
else
  fail "trees diverged after re-sync"; head -40 "$TDIR/resync.tree.diff"
fi

# S4: dry-run sync on a fresh cold tree (trees must stay pristine).
run_pair dryrun-sync seed_cold_state sync --dry-run

# S5: uninstall on the synced trees; S6: dry-run uninstall on fresh trees.
run_pair_on_trees uninstall "$TDIR/js-cold-sync/home" "$TDIR/rs-cold-sync/home" uninstall
normalize_trees "$TDIR/js-cold-sync" "$TDIR/rs-cold-sync"
if diff -r "$TDIR/js-cold-sync.norm" "$TDIR/rs-cold-sync.norm" > "$TDIR/uninstall.tree.diff" 2>&1; then
  pass "trees identical after uninstall"
else
  fail "trees diverged after uninstall"; head -40 "$TDIR/uninstall.tree.diff"
fi
run_pair dryrun-uninstall seed_cold_state uninstall --dry-run

# S7: --tools filter on a fresh cold tree.
run_pair tools-filter seed_cold_state sync --tools=codex,kimi,cursor,grok

# S8: absent tools — only claude/codex/kimi detected (cursor stays "installed"
# via the real /Applications/Cursor.app, which both sides see identically).
seed_absent_tools_state() {
  mkdir -p "$1/home/.claude" "$1/home/.codex" "$1/home/.kimi-code"
}
SCEN_PATH="$EMPTYBIN:/usr/bin:/bin"
run_pair absent-tools seed_absent_tools_state status
run_pair absent-sync seed_absent_tools_state sync
SCEN_PATH=""

# S9: drift/broken-state recovery. Seed by JS-syncing a cold tree (setup only
# — both sides then receive identical tampered copies).
seed_drift_state() {
  seed_cold_state "$1"
  local home="$1/home"
  env HOME="$home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" sync > /dev/null 2>&1
  # Tamper: remove a source skill, drift a target skill, break entries,
  # orphan the cursor frontmatter, comment the opencode jsonc, flip grok's
  # detection to the which-probe with a pre-existing OK block.
  rm -rf "$TDIR/src-skills/quote"
  printf 'DRIFTED\n' >> "$home/.claude/skills/alpha/SKILL.md"
  printf '{"mcpServers": {"allternit-ops": {"command": "node", "args": ["/stale.js"]}}}\n' \
    > "$home/.claude/settings.json"
  python3 - "$home/.cursor/rules/allternit.mdc" <<'PY'
import sys
path = sys.argv[1]
text = open(path).read()
text = text.replace("alwaysApply: true\n---\n\n", "alwaysApply: true\n---\n\nUSER EDIT\n\n", 1)
open(path, "w").write(text)
PY
  printf '{\n  // user comment\n  "mcp": {}\n}\n' > "$home/.config/opencode/opencode.jsonc"
  rm -rf "$home/.grok"
  mkdir -p "$home/.grok"
  printf '[mcp_servers.allternit-ops]\ncommand = "node"\nargs = ["%s"]\nenabled = true\n' \
    "$TDIR/ops/index.js" > "$home/.grok/config.toml"
}
run_pair drift-state seed_drift_state status
run_pair drift-sync seed_drift_state sync

# Restore the source skill the drift scenario removed (later scenarios and
# the live gate do not depend on it, but keep the fixture tree canonical).
build_src_skills "$TDIR/src-skills"

# S10: flat skillsFormat engine path — unused by the real manifest but the JS
# supports it; both sides must agree if a manifest ever sets it. Swap in a
# flat-manifest for both sides, run, then restore.
seed_flat_format_state() {
  mkdir -p "$1/home/.dsh"
}
"$NODE_BIN" -e '
  const fs = require("fs");
  const m = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  m.tools.dsh.skillsFormat = "flat";
  fs.writeFileSync(process.argv[2], JSON.stringify(m, null, 2) + "\n");
' "$TDIR/fixture-harness.json" "$TDIR/fixture-harness-flat.json"
TDIR_REF="$TDIR" build_fixture_manifest "$BRAIN_OPS/harness.json" "$TDIR/fixture-harness.json"
"$NODE_BIN" -e '
  const fs = require("fs");
  const m = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  m.tools.dsh.skillsFormat = "flat";
  fs.writeFileSync(process.argv[2], JSON.stringify(m, null, 2) + "\n");
' "$TDIR/fixture-harness.json" "$TDIR/fixture-harness.json.tmp"
mv "$TDIR/fixture-harness.json.tmp" "$TDIR/fixture-harness.json"
cp "$TDIR/fixture-harness.json" "$TDIR/js-sync/harness.json"
SCEN_MANIFEST="$TDIR/fixture-harness.json"
run_pair flat-format seed_flat_format_state sync --tools=dsh
SCEN_MANIFEST=""

# S11: error exits — unknown command and missing source skills dir must both
# produce identical stderr + exit 1.
BADSRC="$TDIR/badsrc-harness.json"
"$NODE_BIN" -e '
  const fs = require("fs");
  const m = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  m.source.skillsDir = "/nonexistent-ao-harness-src";
  fs.writeFileSync(process.argv[2], JSON.stringify(m, null, 2) + "\n");
' "$TDIR/fixture-harness.json" "$BADSRC"
cp "$BADSRC" "$TDIR/js-sync/harness.json"
for cmd in "bogus" "sync"; do
  env HOME="$TDIR/js-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" $cmd > "$TDIR/js-err-$cmd.stdout" 2> "$TDIR/js-err-$cmd.stderr"
  echo $? > "$TDIR/js-err-$cmd.exit"
  env HOME="$TDIR/rs-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
    AO_HARNESS_MANIFEST="$BADSRC" "$AO_BIN" harness $cmd > "$TDIR/rs-err-$cmd.stdout" 2> "$TDIR/rs-err-$cmd.stderr"
  echo $? > "$TDIR/rs-err-$cmd.exit"
  if diff -u "$TDIR/js-err-$cmd.stderr" "$TDIR/rs-err-$cmd.stderr" > "$TDIR/err-$cmd.diff" 2>&1 \
     && diff -q "$TDIR/js-err-$cmd.exit" "$TDIR/rs-err-$cmd.exit" > /dev/null; then
    pass "error exit: '$cmd' with missing source dir (stderr + exit 1 identical)"
  else
    fail "error exit: '$cmd'"; cat "$TDIR/err-$cmd.diff"
  fi
done
# Unknown command with a VALID source dir.
cp "$TDIR/fixture-harness-flat.json" "$TDIR/js-sync/harness.json"
env HOME="$TDIR/js-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
  "$NODE_BIN" "$TDIR/js-sync/harness-sync.js" bogus > "$TDIR/js-err2.stdout" 2> "$TDIR/js-err2.stderr"
echo $? > "$TDIR/js-err2.exit"
env HOME="$TDIR/rs-cold-sync/home" PATH="$FAKEBIN_PATH" AO_PARITY_MCP_PATH="$TDIR/ops/index.js" \
  AO_HARNESS_MANIFEST="$TDIR/fixture-harness-flat.json" "$AO_BIN" harness bogus \
  > "$TDIR/rs-err2.stdout" 2> "$TDIR/rs-err2.stderr"
echo $? > "$TDIR/rs-err2.exit"
if diff -u "$TDIR/js-err2.stderr" "$TDIR/rs-err2.stderr" > "$TDIR/err2.diff" 2>&1 \
   && diff -q "$TDIR/js-err2.exit" "$TDIR/rs-err2.exit" > /dev/null; then
  pass "error exit: unknown command (stderr + exit 1 identical)"
else
  fail "error exit: unknown command"; cat "$TDIR/err2.diff"
fi

# Restore manifests.
TDIR_REF="$TDIR" build_fixture_manifest "$BRAIN_OPS/harness.json" "$TDIR/fixture-harness.json"
cp "$TDIR/fixture-harness.json" "$TDIR/js-sync/harness.json"

# ---------------------------------------------------------------------------
# Live smoke gate (real $HOME, read-only: status + dry-run sync)
# ---------------------------------------------------------------------------

if [ "$SKIP_LIVE" -eq 0 ]; then
  "$NODE_BIN" "$BRAIN_OPS/harness-sync.js" status > "$TDIR/js-live-status.stdout" 2> "$TDIR/js-live-status.stderr"
  echo $? > "$TDIR/js-live-status.exit"
  "$AO_BIN" harness status > "$TDIR/rs-live-status.stdout" 2> "$TDIR/rs-live-status.stderr"
  echo $? > "$TDIR/rs-live-status.exit"
  if diff -u "$TDIR/js-live-status.stdout" "$TDIR/rs-live-status.stdout" > "$TDIR/live-status.diff" 2>&1 \
     && diff -q "$TDIR/js-live-status.exit" "$TDIR/rs-live-status.exit" > /dev/null; then
    pass "live smoke: status on real HOME byte-identical (16 tools)"
  else
    fail "live smoke: status"; head -60 "$TDIR/live-status.diff"
  fi

  "$NODE_BIN" "$BRAIN_OPS/harness-sync.js" sync --dry-run > "$TDIR/js-live-dry.stdout" 2> "$TDIR/js-live-dry.stderr"
  echo $? > "$TDIR/js-live-dry.exit"
  "$AO_BIN" harness sync --dry-run > "$TDIR/rs-live-dry.stdout" 2> "$TDIR/rs-live-dry.stderr"
  echo $? > "$TDIR/rs-live-dry.exit"
  if diff -u "$TDIR/js-live-dry.stdout" "$TDIR/rs-live-dry.stdout" > "$TDIR/live-dry.diff" 2>&1 \
     && diff -q "$TDIR/js-live-dry.exit" "$TDIR/rs-live-dry.exit" > /dev/null; then
    pass "live smoke: sync --dry-run on real HOME byte-identical (16 tools)"
  else
    fail "live smoke: sync --dry-run"; head -60 "$TDIR/live-dry.diff"
  fi
else
  echo "SKIP: live smoke gate (--skip-live)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo
echo "== ao harness parity: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
