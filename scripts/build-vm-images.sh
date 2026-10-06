#!/bin/bash
# Build and Release VM Images
#
# Usage:
#   ./scripts/build-vm-images.sh [version]
#
# Examples:
#   ./scripts/build-vm-images.sh 1.1.0
#   ./scripts/build-vm-images.sh 1.2.0-beta

set -e

VERSION=${1:-"1.1.0"}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "╔═══════════════════════════════════════════════════════════════╗"
echo "║           Allternit VM Image Build & Release Script                ║"
echo "╚═══════════════════════════════════════════════════════════════╝"
echo ""
echo "Version: $VERSION"
echo "Repository: Allternit/allternit-platform"
echo ""

# Check if we're in the right directory
if [ ! -f "$REPO_ROOT/Cargo.toml" ]; then
    echo "❌ Error: Not in the allternit repository root"
    exit 1
fi

cd "$REPO_ROOT"

# Check for GitHub CLI
if ! command -v gh &> /dev/null; then
    echo "❌ Error: GitHub CLI (gh) is not installed"
    echo "   Install it from: https://cli.github.com/"
    exit 1
fi

# Check if logged in to GitHub
if ! gh auth status &> /dev/null; then
    echo "❌ Error: Not logged in to GitHub CLI"
    echo "   Run: gh auth login"
    exit 1
fi

echo "Step 1: Triggering GitHub Actions workflow..."
echo "─────────────────────────────────────────────"

# Trigger the workflow
gh workflow run vm-images.yml \
    --field version="$VERSION" \
    --field upload_release=true

echo ""
echo "✅ Workflow triggered successfully!"
echo ""
echo "📊 Monitor progress at:"
echo "   https://github.com/Allternit/allternit-platform/actions"
echo ""
echo "📝 The workflow will:"
echo "   1. Build VM images for x86_64 and ARM64"
echo "   2. Compress images with zstd"
echo "   3. Create GitHub Release v$VERSION"
echo "   4. Upload images as release assets"
echo ""
echo "⏱️  This will take approximately 15-20 minutes"
echo ""
echo "💡 To check status locally:"
echo "   gh run list --workflow=vm-images.yml"
echo ""
echo "🎯 After completion, users can download with:"
echo "   allternit vm setup"
echo ""
