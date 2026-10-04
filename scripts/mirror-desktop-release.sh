#!/usr/bin/env bash
# Mirror a published Allternit Desktop release (Gizziio/desktop v<version>) to R2:
#   allternit-runtime/desktop/v<version>/<assets>   (served at runtime.allternit.com)
#   allternit-runtime/desktop/latest.json           {version, assets:{...}: url}
# and delete desktop/v* older than the previous version (keeps current + previous).
#
#   scripts/mirror-desktop-release.sh 1.1.3            # dry-run plan (default)
#   R2_ENDPOINT=https://<acct>.r2.cloudflarestorage.com R2_ACCESS_KEY=... R2_SECRET=... \
#     scripts/mirror-desktop-release.sh 1.1.3 --apply
#
# Needs: gh (authenticated), curl >= 7.75, shasum, python3.
set -euo pipefail

version="${1:-}"; mode="${2:-}"
if [[ -z "$version" || ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]]; then
  echo "usage: $0 <version, e.g. 1.1.3> [--apply]" >&2; exit 2
fi
apply=0; [[ "$mode" == "--apply" ]] && apply=1
REPO="${DESKTOP_REPO:-Gizziio/desktop}"
BUCKET="${R2_BUCKET:-allternit-runtime}"
PUBLIC="${RUNTIME_PUBLIC_URL:-https://runtime.allternit.com}"
tag="v$version"
work="$(mktemp -d "${TMPDIR:-/tmp}/desktop-mirror.XXXXXX")"
trap 'rm -rf "$work"' EXIT

if [[ $apply -eq 1 ]]; then
  : "${R2_ENDPOINT:?set R2_ENDPOINT}" "${R2_ACCESS_KEY:?set R2_ACCESS_KEY}" "${R2_SECRET:?set R2_SECRET}"
fi

r2() { # r2 <curl args...> ; signs the request
  curl -fsS --aws-sigv4 "aws:amz:auto:s3" --user "$R2_ACCESS_KEY:$R2_SECRET" "$@"
}

echo "== download $REPO $tag assets"
gh release download "$tag" -R "$REPO" -D "$work" \
  -p '*.dmg' -p '*.zip' -p '*.exe' -p '*.AppImage' -p '*.deb' -p '*.yml' -p '*.blockmap'
shopt -s nullglob
files=("$work"/*)
if [[ ${#files[@]} -eq 0 ]]; then echo "no assets found for $tag" >&2; exit 1; fi

echo "== upload plan -> $BUCKET/desktop/$tag/"
for f in "${files[@]}"; do
  n="$(basename "$f")"
  printf '  %s (%s bytes, sha256 %s)\n' "$n" "$(wc -c <"$f" | tr -d ' ')" "$(shasum -a 256 "$f" | cut -c1-12)"
  if [[ $apply -eq 1 ]]; then
    r2 -H "Content-Type: application/octet-stream" -T "$f" "$R2_ENDPOINT/$BUCKET/desktop/$tag/$n"
  fi
done

# latest.json: a key per platform, value is the public URL of the installer.
python3 - "$work" "$version" "$PUBLIC" >"$work/latest.json.out" <<'PY'
import json, os, sys
d, version, public = sys.argv[1:4]
slots = {}
for n in sorted(os.listdir(d)):
    url = f"{public}/desktop/v{version}/{n}"
    low = n.lower()
    if low.endswith(".dmg"):
        slots["mac_x64" if "x64" in low else "mac_arm64"] = url
    elif low.endswith(".exe"):
        slots["win_arm64" if "arm64" in low else "win_x64"] = url
    elif low.endswith(".appimage"):
        slots["linux_arm64" if ("arm64" in low or "aarch64" in low) else "linux_x64"] = url
    elif low.endswith(".deb"):
        slots["linux_deb_arm64" if ("arm64" in low or "aarch64" in low) else "linux_deb_x64"] = url
    elif low in ("latest.yml", "latest-mac.yml", "latest-linux.yml"):
        slots["manifest_" + low[:-4].replace("-", "_")] = url
print(json.dumps({"version": version, "assets": slots}, indent=2))
PY
echo "== latest.json"; cat "$work/latest.json.out"
if [[ $apply -eq 1 ]]; then
  r2 -H "Content-Type: application/json" -H "Cache-Control: public, max-age=300" \
    -T "$work/latest.json.out" "$R2_ENDPOINT/$BUCKET/desktop/latest.json"
fi

echo "== prune (keep $tag + previous version)"
if [[ $apply -eq 1 ]]; then
  listing="$(r2 "$R2_ENDPOINT/$BUCKET?list-type=2&prefix=desktop/&delimiter=/")"
else
  listing=""
  echo "  (dry run: remote listing skipped; --apply deletes desktop/v* older than the previous version)"
fi
all="$(printf '%s' "$listing" | grep -o '<Prefix>desktop/v[^<]*/</Prefix>' | sed -E 's#<Prefix>desktop/v([^<]*)/</Prefix>#\1#' || true)"
others="$(printf '%s\n' "$all" | grep -vxF "$version" | grep -v '^$' | sort -V || true)"
# previous = highest remaining version below the one being published
previous="$(printf '%s\n%s\n' "$others" "$version" | grep -v '^$' | sort -V | grep -xF -B1 "$version" | head -1 || true)"
[[ "$previous" == "$version" ]] && previous=""
for v in $others; do
  if [[ "$v" == "$previous" ]]; then echo "  keep   desktop/v$v/ (previous)"; continue; fi
  if [[ "$(printf '%s\n%s\n' "$v" "$version" | sort -V | tail -1)" == "$v" ]]; then
    echo "  keep   desktop/v$v/ (newer than $version)"; continue
  fi
  echo "  delete desktop/v$v/"
  keys="$(r2 "$R2_ENDPOINT/$BUCKET?list-type=2&prefix=desktop/v$v/" | grep -o '<Key>[^<]*</Key>' | sed -E 's#</?Key>##g')"
  for k in $keys; do r2 -X DELETE "$R2_ENDPOINT/$BUCKET/$k"; done
done

if [[ $apply -eq 0 ]]; then echo "dry run only; re-run with --apply to upload."; else echo "done."; fi
