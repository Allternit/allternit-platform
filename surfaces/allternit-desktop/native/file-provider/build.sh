#!/bin/bash
# Builds the macOS File Provider pieces of Allternit Desktop (plan step 23)
# into native/file-provider/build/:
#   AllternitFileProvider     the extension executable (universal arm64 + x86_64)
#   allternit-files-domains   the helper Electron main runs (universal)
#   badges/badge-*.icns       Finder badge images (design 6A)
#
# The Swift sources are the iOS ones, not a copy: the AllternitFilesKit
# package and the AllternitFileProvider extension sources from
# surfaces/allternit-mobile/ios (they hold no UIKit). Only Info.plist,
# entitlements, the helper and the badge drawing live here.
#
# Run by `npm run prepare:file-provider` (scripts/prepare-file-provider.cjs),
# which skips it off macOS. scripts/embed-file-provider.cjs (electron-builder
# afterPack) then places and signs the result inside the app bundle.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
IOS="$(cd "$HERE/../../../allternit-mobile/ios" && pwd)"
KIT_SRC="$IOS/Packages/AllternitFilesKit/Sources/AllternitFilesKit"
EXT_SRC="$IOS/AllternitFileProvider"
OUT="$HERE/build"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
MIN_MACOS=13.0
ARCHS="${ALLTERNIT_FILE_PROVIDER_ARCHS:-arm64 x86_64}"

# -L follows the Contract symlink to contracts/files-drive/gen.
KIT_FILES=()
while IFS= read -r f; do KIT_FILES+=("$f"); done < <(find -L "$KIT_SRC" -name '*.swift' | sort)
EXT_FILES=("$EXT_SRC"/*.swift)

rm -rf "$OUT"
mkdir -p "$OUT/badges"

COMMON=(-sdk "$SDK" -swift-version 5 -O -whole-module-optimization)

for arch in $ARCHS; do
  A="$OUT/$arch"
  mkdir -p "$A"
  TARGET=(-target "$arch-apple-macos$MIN_MACOS")
  echo "[file-provider] AllternitFilesKit ($arch)"
  xcrun swiftc "${COMMON[@]}" "${TARGET[@]}" -parse-as-library -application-extension \
    -module-name AllternitFilesKit -emit-library -static \
    -emit-module -emit-module-path "$A/AllternitFilesKit.swiftmodule" \
    -o "$A/libAllternitFilesKit.a" "${KIT_FILES[@]}"
  echo "[file-provider] AllternitFileProvider extension ($arch)"
  xcrun swiftc "${COMMON[@]}" "${TARGET[@]}" -parse-as-library -application-extension \
    -module-name AllternitFileProvider -I "$A" -L "$A" -lAllternitFilesKit \
    -framework FileProvider -framework UniformTypeIdentifiers \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$A/AllternitFileProvider" "${EXT_FILES[@]}"
  echo "[file-provider] allternit-files-domains helper ($arch)"
  xcrun swiftc "${COMMON[@]}" "${TARGET[@]}" -parse-as-library \
    -module-name FilesDomainsHelper -I "$A" -L "$A" -lAllternitFilesKit \
    -framework FileProvider \
    -o "$A/allternit-files-domains" "$HERE/Helper/main.swift"
done

for bin in AllternitFileProvider allternit-files-domains; do
  inputs=()
  for arch in $ARCHS; do inputs+=("$OUT/$arch/$bin"); done
  lipo -create "${inputs[@]}" -output "$OUT/$bin"
done

echo "[file-provider] badges"
xcrun swift "$HERE/Badges/make-badges.swift" "$OUT/badges"
for set in "$OUT"/badges/*.iconset; do
  iconutil -c icns "$set" -o "${set%.iconset}.icns"
  rm -rf "$set"
done

for arch in $ARCHS; do rm -rf "${OUT:?}/$arch"; done
echo "[file-provider] built into $OUT"
