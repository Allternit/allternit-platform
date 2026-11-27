#!/bin/bash
#
# Gizzi CLI Publish Script
# Usage: ./scripts/publish.sh [version]
#

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
VERSION="${1:-patch}"
NPM_REGISTRY="https://registry.npmjs.org"
GITHUB_REPO="allternit/gizzi"

# Functions
log_info() {
    echo "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo "${GREEN}[SUCCESS]${NC} $1"
}

log_error() {
    echo "${RED}[ERROR]${NC} $1"
}

log_warn() {
    echo "${YELLOW}[WARN]${NC} $1"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."
    
    if ! command -v node &> /dev/null; then
        log_error "Node.js is required"
        exit 1
    fi
    
    if ! command -v npm &> /dev/null; then
        log_error "npm is required"
        exit 1
    fi
    
    if ! command -v git &> /dev/null; then
        log_error "Git is required"
        exit 1
    fi
    
    if ! command -v gh &> /dev/null; then
        log_warn "GitHub CLI not found. GitHub release will be skipped."
    fi
    
    log_success "Prerequisites check passed"
}

# Clean build artifacts
clean() {
    log_info "Cleaning build artifacts..."
    rm -rf dist/
    rm -rf node_modules/
    log_success "Clean complete"
}

# Install dependencies
install_deps() {
    log_info "Installing dependencies..."
    npm ci
    log_success "Dependencies installed"
}

# Run tests
test() {
    log_info "Running tests..."
    npm test
    log_success "Tests passed"
}

# Build binaries
build() {
    log_info "Building binaries..."
    
    # Build Node.js version
    npm run build
    
    # Build platform-specific binaries
    npm run build:all
    
    log_success "Build complete"
}

# Bump version
bump_version() {
    log_info "Bumping version: $VERSION"
    
    if [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        # Explicit version
        npm version "$VERSION" --no-git-tag-version
    else
        # npm version bump (patch/minor/major)
        npm version "$VERSION" --no-git-tag-version
    fi
    
    NEW_VERSION=$(node -p "require('./package.json').version")
    log_success "Version bumped to $NEW_VERSION"
}

# Publish to npm
publish_npm() {
    log_info "Publishing to npm..."
    
    # Check if logged in
    if ! npm whoami &> /dev/null; then
        log_error "Not logged in to npm. Run: npm login"
        exit 1
    fi
    
    # Publish
    npm publish --access public
    
    log_success "Published to npm"
}

# Create GitHub release
create_github_release() {
    if ! command -v gh &> /dev/null; then
        log_warn "Skipping GitHub release (gh not installed)"
        return
    fi
    
    log_info "Creating GitHub release..."
    
    NEW_VERSION=$(node -p "require('./package.json').version")
    TAG="v$NEW_VERSION"
    
    # Create git tag
    git add package.json package-lock.json
    git commit -m "Release $TAG"
    git tag -a "$TAG" -m "Release $TAG"
    git push origin "$TAG"
    
    # Create release with binaries
    gh release create "$TAG" \
        --title "Gizzi $TAG" \
        --notes "Release $TAG" \
        dist/gizzi-macos \
        dist/gizzi-linux \
        dist/gizzi-win.exe
    
    log_success "GitHub release created: $TAG"
}

# Homebrew formula + Scoop manifest
# Automated since step 20 of the Gizzi overhaul: the release workflow
# (.github/workflows/publish-gizzi-code-npm.yml, jobs release-feed, homebrew-tap,
# scoop-bucket) renders cmd/gizzi-code/packaging/homebrew/gizzi-code.rb and
# packaging/scoop/gizzi-code.json from the release checksums.txt and pushes them
# to Allternit/homebrew-tap and Allternit/scoop-bucket, along with the signed
# update feed at runtime.allternit.com/gizzi/<channel>/.
# Manual fallback (only when that job skipped for a missing token): take
# packaging/gizzi-code.rb / gizzi-code.json from the run's gizzi-release-feed
# artifact, or render them yourself:
#   bun script/make-release-feed.ts render --template packaging/homebrew/gizzi-code.rb \
#     --version <v> --checksums checksums.txt --out gizzi-code.rb
# then commit to Formula/gizzi-code.rb in Allternit/homebrew-tap.
update_homebrew() {
    log_info "Homebrew formula and Scoop manifest are rendered and pushed by the release workflow (publish-gizzi-code-npm.yml)."
    log_info "Manual fallback: see the comment above update_homebrew in this script."
}

# Main
main() {
    echo "${BLUE}"
    echo "╔═══════════════════════════════════════╗"
    echo "║     Gizzi CLI Publish Script          ║"
    echo "╚═══════════════════════════════════════╝"
    echo "${NC}"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --skip-tests)
                SKIP_TESTS=1
                shift
                ;;
            --skip-npm)
                SKIP_NPM=1
                shift
                ;;
            --skip-github)
                SKIP_GITHUB=1
                shift
                ;;
            --dry-run)
                DRY_RUN=1
                shift
                ;;
            -h|--help)
                echo "Usage: $0 [version] [options]"
                echo ""
                echo "Version: patch | minor | major | x.y.z"
                echo ""
                echo "Options:"
                echo "  --skip-tests     Skip running tests"
                echo "  --skip-npm       Skip npm publish"
                echo "  --skip-github    Skip GitHub release"
                echo "  --dry-run        Show what would be done"
                echo "  -h, --help       Show this help"
                exit 0
                ;;
            *)
                VERSION="$1"
                shift
                ;;
        esac
    done
    
    if [[ "$DRY_RUN" == "1" ]]; then
        log_warn "DRY RUN MODE - No changes will be made"
    fi
    
    # Run steps
    check_prerequisites
    clean
    install_deps
    
    if [[ "$SKIP_TESTS" != "1" ]]; then
        test
    fi
    
    bump_version
    
    if [[ "$DRY_RUN" != "1" ]]; then
        build
        
        if [[ "$SKIP_NPM" != "1" ]]; then
            publish_npm
        fi
        
        if [[ "$SKIP_GITHUB" != "1" ]]; then
            create_github_release
        fi
        
        update_homebrew
        
        log_success "Publish complete!"
    else
        log_info "Dry run - skipping build and publish"
    fi
}

main "$@"
