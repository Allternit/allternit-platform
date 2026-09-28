#!/bin/bash
# scripts/build-state.sh — what is built, released, installed and running for
# gizzi-code and Allternit Desktop, across every worktree of this repo.
#
# Run this BEFORE building, releasing, or installing either product
# (AGENTS.md "one current build" commandment). It answers:
#   - is someone else building right now?            (ACTIVE BUILDS)
#   - what did the last release/build come from?     (sha + date)
#   - is the installed/running copy the newest one?  (RUNNING)
#   - which old binaries are still on disk?          (STALE)
#
# Usage:
#   scripts/build-state.sh            report (+ refresh the guard stamp)
#   scripts/build-state.sh --prune    also delete STALE artifacts this session
#                                     may touch (shared checkout, current
#                                     worktree, workspace-level backups) and
#                                     `brew cleanup gizzi-code`. Refuses while
#                                     a build is active. Never touches the
#                                     installed app, the current Homebrew
#                                     version, or another session's worktree.
#   scripts/build-state.sh --offline  skip network checks (npm, brew tap)
#
# The stamp ~/.allternit/state/build-state.stamp is what
# .steering/bin/guard-build.sh checks before letting a build command run.
set -u
exec python3 - "$@" <<'PY'
import datetime as dt, json, os, re, shutil, subprocess, sys, time
from pathlib import Path

ARGS = set(sys.argv[1:])
PRUNE = "--prune" in ARGS
OFFLINE = "--offline" in ARGS
HOME = Path.home()
CWD = Path.cwd()

def sh(*cmd, cwd=None, timeout=30):
    try:
        return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout).stdout.strip()
    except Exception:
        return ""

def mtime(p): return dt.datetime.fromtimestamp(p.stat().st_mtime)
def fmt(t): return t.strftime("%Y-%m-%d %H:%M") if t else "?"
def short(s): return (s or "?")[:9] + ("-dirty" if s and s.endswith("-dirty") else "")

TOP = Path(sh("git", "rev-parse", "--show-toplevel") or ".")
common = sh("git", "rev-parse", "--path-format=absolute", "--git-common-dir")
SHARED = Path(common).parent if common else TOP  # the shared main checkout
WORKTREES = []
for line in sh("git", "-C", str(SHARED), "worktree", "list", "--porcelain").splitlines():
    if line.startswith("worktree "):
        WORKTREES.append(Path(line[9:]))
WORKSPACE = SHARED.parent
MINE = {SHARED.resolve(), TOP.resolve()}

def owned(p: Path) -> bool:
    """May this session delete p? Shared checkout, own worktree, workspace backups."""
    rp = p.resolve()
    if any(str(rp).startswith(str(m) + "/") for m in MINE):
        # but not something inside a *different* linked worktree nested under the shared checkout
        for wt in WORKTREES:
            w = wt.resolve()
            if w not in MINE and str(rp).startswith(str(w) + "/"):
                return False
        return True
    return rp.parent.name.startswith(".backup-sidecars") or ".backup-sidecars" in str(rp)

stale, problems, active = [], [], []

sh("git", "-C", str(SHARED), "fetch", "-q", "origin", "--tags", timeout=60)
main_sha = sh("git", "-C", str(SHARED), "rev-parse", "origin/main")
main_date = sh("git", "-C", str(SHARED), "log", "-1", "--format=%cd", "--date=format:%Y-%m-%d %H:%M", "origin/main")
print(f"== origin/main {short(main_sha)} ({main_date})")

# ---------------------------------------------------------------- active builds
BUILD_PAT = re.compile(r"build-production\.js|script/build\.ts|electron-builder|build-desktop\.sh|bun build .*--compile|notarize")
ps = sh("ps", "-axo", "pid=,lstart=,args=")
procs = []
for line in ps.splitlines():
    m = re.match(r"\s*(\d+)\s+(\w{3} \w{3}\s+\d+ [\d:]+ \d{4})\s+(.*)", line)
    if m:
        procs.append((int(m.group(1)), m.group(2), m.group(3)))
for pid, started, args in procs:
    if BUILD_PAT.search(args) and "build-state" not in args and "grep" not in args:
        cwd = sh("lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn").split("\nn")[-1].lstrip("n")
        active.append(f"pid {pid} since {started}: {args[:110]}  (cwd {cwd})")
print("\n== ACTIVE BUILDS")
print("\n".join("  " + a for a in active) if active else "  none")

# ---------------------------------------------------------------- gizzi-code
print("\n== gizzi-code")
SHA_RE = re.compile(rb'GIZZI_BUILD_SHA = "([0-9a-f]{7,40}(?:-dirty)?|unknown)"')
def binary_sha(p: Path):
    try:
        with open(p, "rb") as f:
            m = SHA_RE.search(f.read())
        return m.group(1).decode() if m else None
    except Exception:
        return None

def gizzi_version(p: Path):
    out = sh(str(p), "--version", timeout=15)
    m = re.search(r"\d+\.\d+\.\d+", out)
    return m.group(0) if m else "?"

tags = sh("git", "-C", str(SHARED), "tag", "--list", "gizzi-code/v*", "--sort=-v:refname").splitlines()
rel_tag = tags[0] if tags else None
if rel_tag:
    rel_sha = sh("git", "-C", str(SHARED), "rev-list", "-n1", rel_tag)
    rel_date = sh("git", "-C", str(SHARED), "log", "-1", "--format=%cd", "--date=format:%Y-%m-%d %H:%M", rel_tag)
    # Only changes that alter the shipped binary count (not packaging/docs).
    ahead = sh("git", "-C", str(SHARED), "rev-list", "--count", "--no-merges", f"{rel_tag}..origin/main", "--",
               "cmd/gizzi-code/src", "cmd/gizzi-code/package.json", "cmd/gizzi-code/packages", "cmd/gizzi-code/script/build-production.js",
               "cmd/gizzi-code/script/features.mjs")
    print(f"  latest release   {rel_tag} @ {short(rel_sha)} ({rel_date})")
    if ahead and ahead != "0":
        print(f"  unreleased       {ahead} commit(s) changing the gizzi-code binary since {rel_tag} — the official channels are behind main")
rel_ver = rel_tag.split("/v")[-1] if rel_tag else None

brew_json = sh("brew", "info", "--json=v2", "gizzi-code", timeout=60) if shutil.which("brew") else ""
brew_installed, brew_formula = [], None
if brew_json:
    try:
        f = json.loads(brew_json)["formulae"][0]
        brew_formula = f["versions"]["stable"]
        brew_installed = [i["version"] for i in f.get("installed", [])]
    except Exception:
        pass
cellar = Path("/opt/homebrew/Cellar/gizzi-code")
cellar_versions = sorted(p.name for p in cellar.iterdir()) if cellar.exists() else []
print(f"  homebrew         formula {brew_formula or '?'} · installed {', '.join(cellar_versions) or 'none'}")
if rel_ver and brew_formula and brew_formula != rel_ver:
    problems.append(f"Homebrew tap formula is {brew_formula}, latest release tag is {rel_ver} — tap not updated")
if rel_ver and cellar_versions and rel_ver not in cellar_versions:
    problems.append(f"installed Homebrew gizzi is {cellar_versions[-1]}, latest release is {rel_ver} — run `brew upgrade gizzi-code`")
old_cellar = [v for v in cellar_versions if v != (brew_formula or cellar_versions[-1])]
for v in old_cellar:
    stale.append(("gizzi", cellar / v, f"old Homebrew keg {v}", True))
if not OFFLINE:
    npm_latest = sh("npm", "view", "@allternit/gizzi-code", "version", timeout=30)
    if npm_latest:
        print(f"  npm              @allternit/gizzi-code latest {npm_latest}")
        if rel_ver and npm_latest != rel_ver:
            problems.append(f"npm latest is {npm_latest}, latest release tag is {rel_ver}")

app_sidecar = Path("/Applications/Allternit Desktop.app/Contents/Resources/bin/gizzi-code")
if app_sidecar.exists():
    print(f"  desktop sidecar  {fmt(mtime(app_sidecar))} sha {short(binary_sha(app_sidecar))}  {app_sidecar}")

# local builds, judged per checkout: each checkout keeps its newest build set
# (dist/ + the desktop resources/bin copy); anything older in that checkout is
# stale. A session worktree builds from its branch, so its newer build does
# not make the shared checkout's main build stale.
def gizzi_bins(root: Path):
    out = []
    for rel in ["cmd/gizzi-code/dist", "cmd/gizzi-code/cli-package/dist", "surfaces/allternit-desktop/resources/bin"]:
        d = root / rel
        if d.is_dir():
            out += [p for p in d.iterdir() if p.is_file() and p.name.startswith("gizzi") and p.stat().st_size > 20_000_000]
    return out
print("  local builds (per checkout, newest first):")
any_local = False
for wt in WORKTREES:
    bins = sorted(gizzi_bins(wt), key=lambda p: p.stat().st_mtime, reverse=True)
    if not bins:
        continue
    any_local = True
    branch = sh("git", "-C", str(wt), "branch", "--show-current") or "detached"
    label = "shared checkout" if wt.resolve() == SHARED.resolve() else "worktree"
    print(f"    [{label} · {branch}] {wt}")
    top_t = mtime(bins[0])
    for p in bins:
        t = mtime(p)
        cur = (top_t - t).total_seconds() < 600
        sha = binary_sha(p)
        print(f"      {'CURRENT' if cur else 'STALE':7} {fmt(t)} sha {short(sha) if sha else 'pre-marker':12} {p.relative_to(wt)}")
        if not cur:
            stale.append(("gizzi", p, "older than this checkout's newest build", owned(p)))
# Only gizzi binaries here: these dirs can also hold user-data backups
# (sqlite, ledgers), which this script never touches.
backups = [p for d in WORKSPACE.glob(".backup-sidecars*") for p in d.rglob("gizzi*")
           if p.is_file() and p.stat().st_size > 20_000_000]
if backups:
    any_local = True
    print("    [workspace backups]")
    for p in backups:
        print(f"      STALE   {fmt(mtime(p))} {p}")
        stale.append(("gizzi", p, "sidecar backup", True))
if not any_local:
    print("    none")

# running copies
print("  running:")
cur_brew = brew_formula or (cellar_versions[-1] if cellar_versions else None)
running_any = False
def exe_path(pid):
    for line in sh("lsof", "-a", "-p", str(pid), "-d", "txt", "-Fn").splitlines():
        if line.startswith("n/"):
            return line[1:]
    return ""
# comm= is the full executable path, spaces included (app bundles like
# "Allternit Desktop Preview.app"), which splitting args on spaces breaks.
COMM = {}
for line in sh("ps", "-axo", "pid=,comm=").splitlines():
    pid_s, _, comm = line.strip().partition(" ")
    if pid_s.isdigit():
        COMM[int(pid_s)] = comm.strip()
for pid, started, args in procs:
    first = COMM.get(pid) or args.split(" ")[0]
    is_bin = bool(re.search(r"gizzi(-code)?$", first))
    is_dev = first.rsplit("/", 1)[-1] in ("bun", "node") and "gizzi-code/src/cli/main.ts" in args
    if not (is_bin or is_dev):
        continue
    exe = exe_path(pid) if is_bin else first
    running_any = True
    note = ""
    m = re.search(r"Cellar/gizzi-code/([\d.]+)/", exe)
    if m and cur_brew and m.group(1) != cur_brew:
        note = f"OLD — Homebrew is now {cur_brew}; restart this session to pick it up"
        problems.append(f"gizzi pid {pid} runs old Homebrew {m.group(1)} (current {cur_brew})")
    elif ".app/Contents/Resources/bin/" in exe:
        app_name = re.search(r"([^/]+\.app)/Contents", exe).group(1)
        sub = args[len(first):].strip().split(" ")[0] if args.startswith(first) else ""
        note = f"sidecar of {app_name} ({sub or 'main'})"
    elif "main.ts" in args:
        cwd = sh("lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn").split("\nn")[-1].lstrip("n")
        note = f"dev source in {cwd}"
    print(f"    pid {pid} since {started}: {note or exe[-70:]}")
if not running_any:
    print("    none")

# ---------------------------------------------------------------- Allternit Desktop
print("\n== Allternit Desktop")
dtags = sh("git", "-C", str(SHARED), "tag", "--list", "desktop-v*", "--sort=-v:refname").splitlines()
if dtags:
    print(f"  latest release   {dtags[0]}")
# Every installed copy: /Applications and ~/Applications (e.g. "Preview" apps
# that sessions sync platform/ into directly).
apps = sorted([*Path("/Applications").glob("Allternit Desktop*.app"), *(HOME / "Applications").glob("Allternit Desktop*.app")])
installed_build = None
for app in apps:
    b = sh("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleVersion", str(app / "Contents/Info.plist"))
    v = sh("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", str(app / "Contents/Info.plist"))
    newest_inside = max((f.stat().st_mtime for f in (app / "Contents/Resources").rglob("*") if f.is_file()), default=app.stat().st_mtime)
    print(f"  installed        {v} build {b} · last changed {fmt(dt.datetime.fromtimestamp(newest_inside))}  {app}")
    if app.name == "Allternit Desktop.app":
        installed_build, installed_ver = b, v
        # Builds made with an explicit ALLTERNIT_BUILD_SUFFIX before build-local.cjs
        # derived the number from it carry a timestamp CFBundleVersion; the
        # bundled build-info.json still records the real -b<N> suffix.
        # A UI swapped into platform/ after packaging never came from a build
        # (the one-current-build rule bans in-place patching): flag it.
        res = app / "Contents/Resources"
        try:
            ui_at = (res / "platform/index.html").stat().st_mtime
            if ui_at - (res / "app.asar").stat().st_mtime > 600:
                problems.append(f"installed Desktop UI was patched in place at {fmt(dt.datetime.fromtimestamp(ui_at))} (platform/ newer than the packaged app) — it matches no build; reinstall from a full Desktop build and find the session that rsync'd it")
        except OSError:
            pass
        # Which workspace UI commit this app carries (ui-source.json, stamped by
        # prepare-platform-static). Builds that packaged the stale shared
        # allternit-ai checkout silently "reverted" merged UI fixes.
        ai_repo = TOP.parent / "allternit-ai"
        try:
            src = json.loads((res / "platform/ui-source.json").read_text())
            commit = src.get("commit", "")
            behind = sh("git", "-C", str(ai_repo), "rev-list", "--count", f"{commit}..origin/main") if ai_repo.exists() and commit else ""
            print(f"  installed UI     allternit-ai {commit[:9]} ({src.get('branch')}) · {behind or '?'} commit(s) behind origin/main")
            if behind and behind.isdigit() and int(behind) > 0:
                problems.append(f"installed Desktop UI is {behind} commit(s) behind allternit-ai origin/main — rebuild Desktop from main")
        except (OSError, ValueError):
            print("  installed UI     unknown (no ui-source.json — built before the UI-source stamp; may be stale)")
        try:
            suffix = json.loads((app / "Contents/Resources/build-info.json").read_text()).get("buildSuffix") or ""
            if suffix:
                installed_build = f"{b} ({suffix})"
        except (OSError, ValueError):
            pass
        if dtags:
            rel = dtags[0].split("-v")[-1]
            vkey = lambda x: [int(n) for n in re.findall(r"\d+", x)[:3]]
            if v and vkey(v) < vkey(rel):
                problems.append(f"installed Desktop is {v}, latest release is {dtags[0]}")
    else:
        problems.append(f"extra installed copy {app.name} — a second app that sessions patch in place drifts from main; keep only one installed Desktop")
dmgs, bundles = [], []
for wt in WORKTREES:
    rel = wt / "surfaces/allternit-desktop/release"
    if rel.is_dir():
        dmgs += [p for p in rel.glob("*.dmg")] + [p for p in rel.glob("*.blockmap")]
        bundles += [p for p in rel.glob("mac*/Allternit Desktop.app")]
def build_no(p):
    if p.suffix == ".app":  # unpacked bundle: same build as the DMG next to it
        rel = p.parent.parent
        near = [d for d in rel.glob("*.dmg") if abs(d.stat().st_mtime - p.stat().st_mtime) < 3600]
        p = near[0] if near else p
    m = re.search(r"-b(\d+)", p.name)
    return int(m.group(1)) if m else 0
arts = sorted(dmgs + bundles, key=lambda p: p.stat().st_mtime, reverse=True)
top_dmg = max((build_no(p) for p in dmgs if p.suffix == ".dmg"), default=0)
print("  local builds (newest first):")
for p in arts:
    b = build_no(p)
    t = mtime(p)
    is_newest = (p.suffix in (".dmg", ".blockmap") and b == top_dmg) or (p.suffix == ".app" and arts and (mtime(arts[0]) - t).total_seconds() < 3600)
    print(f"    {'NEWEST' if is_newest else 'STALE':6} {fmt(t)} b{b or '?':<6} {p}")
    if not is_newest:
        stale.append(("desktop", p, "older than the newest local build", owned(p)))
if not arts:
    print("    none")
if installed_build and top_dmg and str(top_dmg) not in installed_build:
    problems.append(f"installed Desktop is build {installed_build}, newest local build is b{top_dmg} — install it once verified, or delete it; never keep both")
running_apps = sorted({m.group(1) for pid, _, _ in procs
                       for m in [re.search(r"/([^/]*Allternit Desktop[^/]*\.app)/Contents/MacOS/", COMM.get(pid, ""))] if m and "Helper" not in m.group(1)})
for name in running_apps:
    print(f"  running          {name}")
if not running_apps:
    print("  running          none")

# ---------------------------------------------------------------- verdict
print("\n== VERDICT")
for p in problems:
    print(f"  ! {p}")
mine_stale = [s for s in stale if s[3]]
others_stale = [s for s in stale if not s[3]]
if stale:
    print(f"  {len(stale)} stale artifact(s): {len(mine_stale)} prunable here, {len(others_stale)} in other sessions' worktrees (tell that session, don't delete)")
if active:
    print("  another build is ACTIVE — coordinate with that session before building the same product")
if not (stale or problems or active):
    print("  clean: one current build per product, nothing stale, nothing building")

if PRUNE:
    print("\n== PRUNE")
    if active:
        print("  refused: a build is active; its outputs may be what looks stale")
    else:
        for prod, p, why, ok in mine_stale:
            try:
                if p.is_dir() and not p.is_symlink():
                    if "Cellar/gizzi-code" in str(p):
                        continue  # brew cleanup handles kegs
                    shutil.rmtree(p)
                else:
                    p.unlink()
                print(f"  deleted {p}")
            except Exception as e:
                print(f"  could not delete {p}: {e}")
        for d in WORKSPACE.glob(".backup-sidecars*"):
            if d.is_dir() and not any(d.rglob("*")):
                d.rmdir()
        if old_cellar and shutil.which("brew"):
            print("  " + (sh("brew", "cleanup", "gizzi-code", timeout=120) or "brew cleanup gizzi-code: done"))

state = HOME / ".allternit" / "state"
state.mkdir(parents=True, exist_ok=True)
(state / "build-state.stamp").write_text(json.dumps({
    "at": int(time.time()), "cwd": str(CWD), "main": main_sha,
    "active": active, "stale": len(stale), "problems": problems}) + "\n")
PY
