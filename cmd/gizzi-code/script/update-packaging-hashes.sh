#!/usr/bin/env bash
# update-packaging-hashes.sh — download the current gizzi-code release assets
# and rewrite the sha256 placeholder fields in packaging/{homebrew,scoop,
# arch}. Run this at release time, after the GitHub release assets
# for the version in packaging (and cmd/gizzi-code/package.json) are published.
#
# Usage:
#   bash script/update-packaging-hashes.sh              # uses latest release
#   bash script/update-packaging-hashes.sh 1.0.2        # explicit version
#   TAG_PREFIX=gizzi-code bash script/update-packaging-hashes.sh 1.0.2
#
# The npm/GitHub Release workflow tags releases "gizzi-code/v<version>"
# and names assets gizzi-code-v<version>-<target>.<ext>. Older releases
# used "gizzi-code/<version>" (no v); set TAG_V= to match those.
set -euo pipefail

REPO="Allternit/allternit-platform"
TAG_PREFIX="${TAG_PREFIX:-gizzi-code}"
TAG_V="${TAG_V-v}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ $# -ge 1 ]; then
  VERSION="$1"
else
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
    | sed -e "s|^${TAG_PREFIX}/||" -e 's/^v//')"
fi

if [ -z "$VERSION" ]; then
  echo "Error: could not resolve version" >&2
  exit 1
fi

TAG="${TAG_PREFIX}/${TAG_V}${VERSION}"
BASE="https://github.com/${REPO}/releases/download/${TAG}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

hash_file="$WORK/hashes.txt"
: > "$hash_file"
ok=true
for target in darwin-arm64 darwin-x64 linux-arm64 linux-x64 windows-x64; do
  ext="tar.gz"
  case "$target" in
    windows-*) ext="zip" ;;
  esac
  asset="gizzi-code-v${VERSION}-${target}.${ext}"
  url="${BASE}/${asset}"
  echo "Downloading ${asset}..."
  if ! curl -fL --retry 3 -o "$WORK/$asset" "$url"; then
    echo "Warning: ${url} not downloadable yet — leaving __SHA256_$(echo "$target" | tr 'a-z-' 'A-Z_')__ placeholder" >&2
    ok=false
    continue
  fi
  digest="$(shasum -a 256 "$WORK/$asset" | awk '{print $1}')"
  echo "${target} ${digest}" >> "$hash_file"
  echo "  sha256: ${digest}"
done

replace() {
  local file="$1" key="$2" target="$3"
  local digest
  digest="$(awk -v t="$target" '$1 == t {print $2}' "$hash_file")"
  if [ -n "$digest" ]; then
    perl -pi -e "s/__SHA256_${key}__/${digest}/g" "$file"
  fi
}

# Placeholders are only present on a fresh template. After the first fill,
# rewrite the live hash fields so a version bump cannot ship stale  SHA256s.
python3 - "$hash_file" "$ROOT" "$VERSION" <<'PY'
import pathlib, re, sys
version = sys.argv[3]
hashes = {}
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    if not line.strip():
        continue
    target, digest = line.split()
    hashes[target] = digest
root = pathlib.Path(sys.argv[2])

def sub_sha256_after_url(path: pathlib.Path, needle: str, digest: str) -> None:
    if not path.exists():
        return
    text = path.read_text()
    # Replace the sha256 on the line immediately after a url containing needle.
    pattern = re.compile(
        rf'(url\s+[^\n]*{re.escape(needle)}[^\n]*\n\s*sha256\s+")[0-9a-fA-F]{{64}}(")',
        re.M,
    )
    path.write_text(pattern.sub(rf'\g<1>{digest}\2', text, count=1))

for path in (
    root / "packaging/homebrew/gizzi-code.rb",
    root / "cli-package/install/gizzi.rb",
):
    for target, digest in hashes.items():
        if target.startswith("windows"):
            continue
        sub_sha256_after_url(path, target, digest)

scoop = root / "packaging/scoop/gizzi-code.json"
if scoop.exists() and "windows-x64" in hashes:
    scoop.write_text(re.sub(r'("hash"\s*:\s*")[0-9a-fA-F]{64}(")', rf'\g<1>{hashes["windows-x64"]}\2', scoop.read_text(), count=1))

winget = root / "cli-package/install/winget/Allternit.GizziCode.yaml"
if winget.exists() and "windows-x64" in hashes:
    winget.write_text(re.sub(r'(InstallerSha256:\s*)[0-9a-fA-F]{64}', rf'\g<1>{hashes["windows-x64"].upper()}', winget.read_text(), count=1))

# Versions must move with the hashes: new hashes on old version URLs make
# every manifest fail its checksum (they sat at 2.0.5 with newer hashes).
def sub_version(path: pathlib.Path, pattern: str) -> None:
    if path.exists():
        path.write_text(re.sub(pattern, rf'\g<1>{version}\g<2>', path.read_text(), count=1, flags=re.M))

sub_version(root / "packaging/homebrew/gizzi-code.rb", r'^(\s*version ")[^"]+(")')
sub_version(root / "cli-package/install/gizzi.rb", r'^(\s*version ")[^"]+(")')
if scoop.exists():
    text = scoop.read_text()
    m = re.search(r'"version"\s*:\s*"([^"]+)"', text)
    old = m.group(1) if m else None
    # The live url pins a version too (only autoupdate uses $version).
    text = re.sub(r'(gizzi-code/v)\d+\.\d+\.\d+(/gizzi-code-v)\d+\.\d+\.\d+', rf'\g<1>{version}\g<2>{version}', text)
    if old and old != version:
        text = text.replace(f'"version": "{old}"', f'"version": "{version}"')
    scoop.write_text(text)
sub_version(root / "packaging/arch/PKGBUILD", r'^(pkgver=)[^\n]+()')
if winget.exists():
    text = winget.read_text()
    m = re.search(r'^PackageVersion:\s*(\S+)', text, flags=re.M)
    if m and m.group(1) != version:
        # Every reference (installer URL, release notes, description) follows.
        text = text.replace(m.group(1), version)
    winget.write_text(text)

import datetime
rpm = root / "packaging/rpm/gizzi-code.spec"
if rpm.exists():
    text = rpm.read_text()
    m = re.search(r'^Version:\s*(\S+)', text, flags=re.M)
    if m and m.group(1) != version:
        text = re.sub(r'^(Version:\s*)\S+', rf'\g<1>{version}', text, count=1, flags=re.M)
        text = re.sub(r'^(Release:\s*)\d+', r'\g<1>1', text, count=1, flags=re.M)
        stamp = datetime.date.today().strftime("%a %b %d %Y")
        entry = f"* {stamp} Allternit Technologies <team@allternit.io> - {version}-1\n- Update to {version}\n\n"
        text = text.replace("%changelog\n", "%changelog\n" + entry, 1)
    rpm.write_text(text)

deb = root / "packaging/debian/DEBIAN/control"
if deb.exists():
    deb.write_text(re.sub(r'^(Version:\s*)\S+', rf'\g<1>{version}', deb.read_text(), count=1, flags=re.M))

arch = root / "packaging/arch/PKGBUILD"
if arch.exists():
    text = arch.read_text()
    if "linux-x64" in hashes:
        text = re.sub(r"(sha256sums_x86_64=\(')[0-9a-fA-F]{64}('\))", rf'\g<1>{hashes["linux-x64"]}\2', text, count=1)
    if "linux-arm64" in hashes:
        text = re.sub(r"(sha256sums_aarch64=\(')[0-9a-fA-F]{64}('\))", rf'\g<1>{hashes["linux-arm64"]}\2', text, count=1)
    arch.write_text(text)
PY

replace packaging/homebrew/gizzi-code.rb DARWIN_ARM64 darwin-arm64
replace packaging/homebrew/gizzi-code.rb DARWIN_X64 darwin-x64
replace packaging/homebrew/gizzi-code.rb LINUX_ARM64 linux-arm64
replace packaging/homebrew/gizzi-code.rb LINUX_X64 linux-x64
replace packaging/scoop/gizzi-code.json WINDOWS_X64 windows-x64
replace packaging/arch/PKGBUILD LINUX_X64 linux-x64
replace packaging/arch/PKGBUILD LINUX_ARM64 linux-arm64
replace cli-package/install/winget/Allternit.GizziCode.yaml WINDOWS_X64 windows-x64
replace cli-package/install/gizzi.rb DARWIN_ARM64 darwin-arm64
replace cli-package/install/gizzi.rb DARWIN_X64 darwin-x64
replace cli-package/install/gizzi.rb LINUX_ARM64 linux-arm64
replace cli-package/install/gizzi.rb LINUX_X64 linux-x64

if $ok; then
  echo ""
  echo "All hashes updated for ${TAG}."
else
  echo ""
  echo "Some assets were missing; remaining placeholders kept." >&2
  exit 1
fi
