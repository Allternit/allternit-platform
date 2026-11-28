#!/bin/sh
# Fetch the pinned linux-firmware tag and stage the blobs listed in MANIFEST (plan step 32).
#   fetch.sh OUTDIR [CACHEDIR]
# Writes in OUTDIR:
#   lib-firmware/<path>.xz   each blob xz-compressed (--check=crc32, 1 MiB dict: the kernel's
#                            FW_LOADER_COMPRESS_XZ format), same relative path as upstream
#   licenses/<GROUP>.<name>  the licence text WHENCE names for each group
#   firmware.list            "<sha256 of the upstream blob>  <path>  <group>", sorted by path
#   groups.tsv               group, licence file, licence sha256, files, compressed bytes
# The tarball is checked against the pinned sha256 in LINUX_FIRMWARE before anything is read.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?fetch.sh OUTDIR}
CACHE=${2:-$OUT/../cache}
die() { echo "firmware: $*" >&2; exit 1; }
say() { echo "firmware: $*"; }
# shellcheck disable=SC1091
. "$HERE/LINUX_FIRMWARE"
mkdir -p "$OUT" "$CACHE"
OUT=$(cd "$OUT" && pwd); CACHE=$(cd "$CACHE" && pwd)
sha() { sha256sum "$1" | cut -d' ' -f1; }

TARBALL="$CACHE/linux-firmware-$FW_TAG.tar.xz"
if [ ! -f "$TARBALL" ]; then
  say "downloading $FW_URL"
  curl -fsSL -o "$TARBALL.part" "$FW_URL"
  mv "$TARBALL.part" "$TARBALL"
fi
[ "$(sha "$TARBALL")" = "$FW_SHA256" ] || { rm -f "$TARBALL"; die "tarball sha256 mismatch for $FW_TAG (want $FW_SHA256)"; }
SRC="$CACHE/linux-firmware-$FW_TAG"
if [ ! -f "$SRC/.extracted" ]; then
  say "extracting $FW_TAG"
  rm -rf "$SRC"; mkdir -p "$SRC"
  tar -C "$SRC" --strip-components=1 -xf "$TARBALL"
  : > "$SRC/.extracted"
fi

rm -rf "$OUT/lib-firmware" "$OUT/licenses" "$OUT/firmware.list" "$OUT/groups.tsv"
mkdir -p "$OUT/lib-firmware" "$OUT/licenses"
: > "$OUT/files.tmp"
export LC_ALL=C
grep -v '^[[:space:]]*#' "$HERE/MANIFEST" | grep -v '^[[:space:]]*$' > "$OUT/manifest.tmp"
# Newer linux-firmware tags keep licence texts under LICENSES/ (20261019 does), older ones at
# the top level. Accept either.
lic_path() {
  if [ -f "$SRC/$1" ]; then echo "$SRC/$1"; elif [ -f "$SRC/LICENSES/$1" ]; then echo "$SRC/LICENSES/$1"; fi
}
while IFS='|' read -r group lic glob; do
  [ -n "$(lic_path "$lic")" ] || die "licence file $lic for group $group is not in the $FW_TAG tarball (top level or LICENSES/)"
  n=0
  # shellcheck disable=SC2086
  for f in $(cd "$SRC" && ls -1d $glob 2>/dev/null); do
    [ -f "$SRC/$f" ] || continue
    printf '%s  %s  %s\n' "$(sha "$SRC/$f")" "$f" "$group" >> "$OUT/files.tmp"
    n=$((n + 1))
  done
  [ "$n" -gt 0 ] || die "group $group: glob '$glob' matches nothing in $FW_TAG"
done < "$OUT/manifest.tmp"
sort -u -k2,2 "$OUT/files.tmp" > "$OUT/firmware.list"
rm -f "$OUT/files.tmp"

while read -r _ path _; do
  mkdir -p "$OUT/lib-firmware/$(dirname "$path")"
  xz -9 --check=crc32 --lzma2=dict=1MiB -T1 -c < "$SRC/$path" > "$OUT/lib-firmware/$path.xz"
done < "$OUT/firmware.list"

: > "$OUT/groups.tsv"
for group in $(cut -d'|' -f1 "$OUT/manifest.tmp" | sort -u); do
  lic=$(grep "^$group|" "$OUT/manifest.tmp" | head -1 | cut -d'|' -f2)
  cp "$(lic_path "$lic")" "$OUT/licenses/$group.$lic"
  count=$(awk -v g="$group" '$3 == g' "$OUT/firmware.list" | wc -l | tr -d ' ')
  bytes=0
  for p in $(awk -v g="$group" '$3 == g {print $2}' "$OUT/firmware.list"); do
    bytes=$((bytes + $(wc -c < "$OUT/lib-firmware/$p.xz")))
  done
  printf '%s\t%s\t%s\t%s\t%s\n' "$group" "$lic" "$(sha "$SRC/$lic")" "$count" "$bytes" >> "$OUT/groups.tsv"
done
rm -f "$OUT/manifest.tmp"

TOTAL=$(awk -F'\t' '{s += $5} END {print s + 0}' "$OUT/groups.tsv")
say "$(wc -l < "$OUT/firmware.list" | tr -d ' ') blobs from $FW_TAG, $((TOTAL / 1048576)) MB compressed"
cat "$OUT/groups.tsv"
[ "$TOTAL" -le $((FW_MAX_MB * 1048576)) ] || die "compressed firmware is over FW_MAX_MB=$FW_MAX_MB: trim MANIFEST"
