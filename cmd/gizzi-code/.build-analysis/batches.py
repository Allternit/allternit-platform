#!/usr/bin/env python3
"""Define batches and check for path-string references repo-wide."""
import json, os, subprocess, collections

ROOT = os.path.dirname(os.path.abspath(__file__)) + "/.."
ROOT = os.path.abspath(ROOT)
cands = json.load(open(os.path.join(ROOT, ".build-analysis/candidates.json")))
cand_set = set(cands)

PROTECTED = ("src/runtime/server/", "src/runtime/auth/", "src/vault/", "src/runtime/brain/")

def pick(pred):
    return sorted(f for f in cands if pred(f) and not f.startswith(PROTECTED))

batches = {
    "B1-legacy-root-tui": pick(lambda f: f.split("/")[1] in {
        "screens","commands","context","hooks","keybindings","vim","voice","utils",
        "coordinator","scheduler","jobs","state","migrations","watcher","workspace",
        "query","entrypoints","daemon","environment-runner","self-hosted-runner",
        "remote","others","native-ts","plugins","skills","moreright","upstreamproxy",
        "pet","config","constants","main.ts","index.ts","Tool.ts","ac.ts","acp.ts",
        "ink.ts","cost-tracker.ts","global.ts","env.ts","session.ts","snapshot.ts",
        "question.ts","ide.ts","patch.ts","commands.ts","hooks.ts","context.ts",
        "scheduler.ts","tools.ts","services","lib","memdir","bootstrap","agent",
        "bus.ts","codemap","continuity","file","hooks.ts","id","lsp","permission",
        "plugin","project","provider","schemas","server","session.ts","share",
        "storage","tool","types","ui","vendor","watcher","workspace"}),
    "B2-cli-ui-legacy": pick(lambda f: f.startswith("src/cli/ui/") and not f.startswith("src/cli/ui/ink-app/")),
    "B3-cli-legacy-files": pick(lambda f: f.startswith("src/cli/") and not f.startswith("src/cli/ui/") and not f.startswith("src/cli/commands/")),
    "B4-cli-commands-dirs": pick(lambda f: f.startswith("src/cli/commands/")),
    "B5-ink-app-dead": pick(lambda f: f.startswith("src/cli/ui/ink-app/")),
    "B6-shared-dead": pick(lambda f: f.startswith("src/shared/")),
    "B7-runtime-dead": pick(lambda f: f.startswith("src/runtime/")),
    "B8-misc": pick(lambda f: True),
}

assigned = set()
for name, fs in batches.items():
    fs = [f for f in fs if f not in assigned]
    assigned.update(fs)
    print(f"{name}: {len(fs)}")
leftover = [f for f in cands if f not in assigned and not f.startswith(PROTECTED)]
print("leftover (protected or unbucketed):", len(leftover))
for f in leftover[:20]:
    print("  ", f)

json.dump({k: [f for f in v if not f.startswith(PROTECTED)] for k, v in batches.items()},
          open(os.path.join(ROOT, ".build-analysis/batches.json"), "w"), indent=0)
print("\n== total to delete:", sum(len(v) for v in batches.values()))
