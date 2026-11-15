#!/usr/bin/env bash
# Build, sign and package the MLX engine pack (mlx-metal-macos-arm64) from scripts/engine-packs/mlx-server.
# Usage: build-mlx-server-macos.sh [outdir]      (NOTARIZE=1 to also notarize the zip)
# xcodebuild, not `swift build`: only xcodebuild compiles the Metal shaders into
# mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib. Needs full Xcode with the
# Metal toolchain (xcodebuild -downloadComponent MetalToolchain).
# Layout: bin/allternit-mlx-server with every SwiftPM resource bundle beside it (MLX finds
# the metallib through SWIFTPM_BUNDLE next to the executable).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${ENGINE_PACK_WORK:-$HOME/w6-build-tmp}"
OUTDIR="${1:-$WORK/out}"
DD="$WORK/mlx-derived"
PACK_ID=mlx-metal-macos-arm64
PACK="$OUTDIR/pack/$PACK_ID"
cd "$HERE/mlx-server"
xcodebuild -scheme allternit-mlx-server -configuration Release -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath "$DD" -skipPackagePluginValidation -skipMacroValidation build | tail -3
PRODUCTS="$DD/Build/Products/Release"
rm -rf "$PACK" && mkdir -p "$PACK/bin"
cp "$PRODUCTS/allternit-mlx-server" "$PACK/bin/"
for b in "$PRODUCTS"/*.bundle; do ditto "$b" "$PACK/bin/$(basename "$b")"; done
[ -f "$PACK/bin/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib" ] || { echo "default.metallib missing: Metal toolchain?" >&2; exit 1; }
cp "$HERE/mlx-server/LICENSES.md" "$PACK/" 2>/dev/null || true

IDENT="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
if [ -n "$IDENT" ]; then
  # Bundles first (resource-only, signed as bundles), then the executable. No entitlements:
  # MLX compiles its kernels at runtime without the JIT entitlement (S3).
  for b in "$PACK"/bin/*.bundle; do
    codesign --force --timestamp --options runtime -s "$IDENT" "$b"
  done
  codesign --force --timestamp --options runtime -s "$IDENT" -i com.allternit.engine.allternit-mlx-server "$PACK/bin/allternit-mlx-server"
  codesign --verify --deep --strict --verbose=2 "$PACK/bin/allternit-mlx-server"
else
  echo "WARNING: no Developer ID identity; pack is unsigned (dev only)" >&2
fi
VERSION="${MLX_PACK_VERSION:-mlx0.31.3-lm2.31.3}"
python3 "$HERE/make-manifest.py" "$PACK" --engine mlx --backend metal --version "$VERSION" --os macos --arch arm64 \
  --min-os 14.0 --source-commit "$(git -C "$HERE" rev-parse HEAD)" --entrypoint bin/allternit-mlx-server
if [ "${NOTARIZE:-0}" = 1 ]; then
  ZIP="$OUTDIR/$PACK_ID.zip"; rm -f "$ZIP"
  (cd "$OUTDIR/pack" && ditto -c -k --keepParent "$PACK_ID" "$ZIP")
  xcrun notarytool submit "$ZIP" --keychain-profile "${NOTARY_PROFILE:-allternit}" --wait 2>&1 | grep -vi password
fi
du -sh "$PACK"; ls -la "$PACK/bin"
