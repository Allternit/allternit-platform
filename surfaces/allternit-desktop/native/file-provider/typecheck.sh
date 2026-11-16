#!/bin/bash
# Seconds-long check (no code generation): AllternitFilesKit, the shared
# File Provider extension sources and this folder's Swift, typechecked for
# macOS. Same inputs as build.sh.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IOS="$(cd "$HERE/../../../allternit-mobile/ios" && pwd)"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
TARGET=(-sdk "$SDK" -target arm64-apple-macos13.0 -swift-version 5)
KIT_FILES=()
while IFS= read -r f; do KIT_FILES+=("$f"); done < <(find -L "$IOS/Packages/AllternitFilesKit/Sources/AllternitFilesKit" -name '*.swift' | sort)
xcrun swiftc "${TARGET[@]}" -parse-as-library -application-extension -module-name AllternitFilesKit \
  -emit-module -emit-module-path "$TMP/AllternitFilesKit.swiftmodule" "${KIT_FILES[@]}"
xcrun swiftc "${TARGET[@]}" -typecheck -parse-as-library -application-extension -module-name AllternitFileProvider \
  -I "$TMP" "$IOS"/AllternitFileProvider/*.swift
xcrun swiftc "${TARGET[@]}" -typecheck -parse-as-library -module-name FilesDomainsHelper -I "$TMP" "$HERE/Helper/main.swift"
xcrun swiftc "${TARGET[@]}" -typecheck "$HERE/Badges/make-badges.swift"
echo "typecheck ok (kit, extension, helper, badges; macOS 13 arm64)"
