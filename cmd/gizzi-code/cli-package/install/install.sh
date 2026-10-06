#!/bin/bash
#
# Gizzi Code Installer
# Usage: curl -fsSL https://install.gizziio.com/install | bash
#

set -e

# =============================================================================
# GIZZI BRAND COLORS (from mascot)
# =============================================================================
RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'

# Primary brand colors
ORANGE='\033[38;5;173m'        # #d97757 - Main brand color
BEIGE='\033[38;5;180m'         # #d4b08c - Secondary
BROWN='\033[38;5;95m'          # #8f6f56 - Tertiary
DARK='\033[38;5;233m'          # #111318 - Dark

# Functional colors
GREEN='\033[38;5;114m'         # Success
YELLOW='\033[38;5;220m'        # Warning  
RED='\033[38;5;203m'           # Error
CYAN='\033[38;5;80m'           # Info/accent

# =============================================================================
# GIZZI MASCOT ASCII ART
# =============================================================================
print_mascot() {
    printf "${ORANGE}      ▄▄      ${RESET}\n"
    printf "${BEIGE}   ▄▄▄  ▄▄▄   ${RESET}\n"
    printf "${BEIGE} ▄██████████▄ ${RESET}\n"
    printf "${BEIGE} █  ${DARK}●    ●${BEIGE}  █ ${RESET}\n"
    printf "${BEIGE} █  A : / / █ ${RESET}\n"
    printf "${BEIGE}  ▀████████▀  ${RESET}\n"
    printf "${BROWN}   █ █  █ █   ${RESET}\n"
    printf "${BROWN}   ▀ ▀  ▀ ▀   ${RESET}\n"
}

# =============================================================================
# CONFIGURATION
# =============================================================================
REPO="Allternit/allternit-platform"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VERSION:-latest}"
GITHUB_API="https://api.github.com/repos/${REPO}"

# =============================================================================
# UTILITY FUNCTIONS
# =============================================================================

print_color() {
    local color="$1"
    local message="$2"
    printf "${color}${message}${RESET}\n"
}

print_step() {
    local step="$1"
    printf "${ORANGE}●${RESET} ${BOLD}%s${RESET}\n" "$step"
}

print_success() {
    printf "${GREEN}✓${RESET} %s\n" "$1"
}

print_error() {
    printf "${RED}✗${RESET} %s\n" "$1" >&2
}

print_warning() {
    printf "${YELLOW}⚠${RESET} %s\n" "$1"
}

print_info() {
    printf "${CYAN}ℹ${RESET} %s\n" "$1"
}

# Spinner for long operations
spinner() {
    local pid=$1
    local message="$2"
    local spin='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
    local i=0
    
    printf "${DIM}%s${RESET} " "$message"
    while kill -0 $pid 2>/dev/null; do
        i=$(( (i+1) % 10 ))
        printf "\r${ORANGE}%s${RESET} ${DIM}%s${RESET} " "${spin:$i:1}" "$message"
        sleep 0.1
    done
    printf "\r${GREEN}✓${RESET} %s\n" "$message"
}

# =============================================================================
# PLATFORM DETECTION
# =============================================================================

detect_platform() {
    local platform
    platform="$(uname -s | tr '[:upper:]' '[:lower:]')"
    
    case "$platform" in
        linux)
            echo "linux"
            ;;
        darwin)
            echo "macos"
            ;;
        mingw*|msys*|cygwin*)
            echo "windows"
            ;;
        *)
            print_error "Unsupported platform: $platform"
            exit 1
            ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    
    case "$arch" in
        x86_64|amd64)
            echo "x64"
            ;;
        arm64|aarch64)
            echo "arm64"
            ;;
        *)
            print_warning "Unknown architecture: $arch, defaulting to x64"
            echo "x64"
            ;;
    esac
}

# =============================================================================
# DEPENDENCY CHECKS
# =============================================================================

check_dependencies() {
    print_step "Checking dependencies..."
    
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER="curl"
        print_success "curl found"
    elif command -v wget >/dev/null 2>&1; then
        DOWNLOADER="wget"
        print_success "wget found"
    else
        print_error "Neither curl nor wget found. Please install one of them."
        exit 1
    fi
}

# =============================================================================
# DOWNLOAD FUNCTIONS
# =============================================================================

download_file() {
    local url="$1"
    local output="$2"
    
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL -o "$output" "$url" 2>&1
    else
        wget -q -O "$output" "$url" 2>&1
    fi
}

# =============================================================================
# VERSION FUNCTIONS
# =============================================================================

get_latest_version() {
    print_step "Fetching latest version..."
    
    local version
    version=$(curl -s "$GITHUB_API/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
    
    if [ -z "$version" ]; then
        print_error "Failed to fetch latest version"
        exit 1
    fi
    
    print_success "Latest version: ${ORANGE}${version}${RESET}"
    echo "$version"
}

# =============================================================================
# INSTALLATION METHODS
# =============================================================================

install_via_npm() {
    print_step "Installing via npm..."
    
    if ! command -v npm >/dev/null 2>&1; then
        print_error "npm not found. Please install Node.js first:"
        print_info "https://nodejs.org/"
        return 1
    fi
    
    npm install -g @allternit/gizzi-code >/dev/null 2>&1 &
    local pid=$!
    spinner $pid "Installing via npm..."
    
    wait $pid
    if [ $? -eq 0 ]; then
        print_success "Successfully installed via npm"
        return 0
    else
        print_error "npm installation failed"
        return 1
    fi
}

install_binary() {
    local platform="$1"
    local arch="$2"
    local version="$3"

    print_step "Installing binary for ${BEIGE}${platform}-${arch}${RESET}..."

    # Delegate to the canonical installer (SOURCE OF TRUTH:
    # cmd/gizzi-code/install in Allternit/allternit-platform, served from
    # https://install.gizziio.com/install). It resolves the version-named
    # release assets (gizzi-code-v<version>-<target>.tar.gz|zip), verifies
    # checksums.txt when present, and installs the `gizzi` alias.
    local env_args=("INSTALL_DIR=$INSTALL_DIR")
    if [ "$version" != "latest" ]; then
        env_args+=("GIZZI_VERSION=$version")
    fi

    if ! curl -fsSL https://install.gizziio.com/install | env "${env_args[@]}" bash; then
        print_error "Binary installation failed"
        return 1
    fi

    print_success "Binary installed to ${BEIGE}${INSTALL_DIR}${RESET}"
}

# =============================================================================
# PATH SETUP
# =============================================================================

add_to_path() {
    print_step "Configuring PATH..."
    
    local shell_rc=""
    local path_export="export PATH=\"\$HOME/.local/bin:\$PATH\""
    
    case "$SHELL" in
        */zsh)
            shell_rc="$HOME/.zshrc"
            ;;
        */bash)
            shell_rc="$HOME/.bashrc"
            ;;
        */fish)
            shell_rc="$HOME/.config/fish/config.fish"
            path_export="set -x PATH \$HOME/.local/bin \$PATH"
            ;;
    esac
    
    if [ -n "$shell_rc" ] && [ -f "$shell_rc" ]; then
        if ! grep -q "$INSTALL_DIR" "$shell_rc" 2>/dev/null; then
            printf "\n# Added by Gizzi Code installer\n%s\n" "$path_export" >> "$shell_rc"
            print_success "Added ${BEIGE}${INSTALL_DIR}${RESET} to PATH in ${BEIGE}${shell_rc}${RESET}"
            print_warning "Please restart your terminal or run: ${BOLD}source ${shell_rc}${RESET}"
        else
            print_info "${INSTALL_DIR} already in PATH"
        fi
    fi
}

# =============================================================================
# VERIFICATION
# =============================================================================

verify_installation() {
    print_step "Verifying installation..."
    
    if command -v gizzi-code >/dev/null 2>&1; then
        print_success "gizzi-code is installed!"
        local version
        version=$(gizzi-code --version 2>/dev/null || echo "unknown")
        print_info "Version: ${ORANGE}${version}${RESET}"
        return 0
    else
        print_warning "gizzi-code installed but not in PATH"
        print_info "Add this to your shell profile:"
        print_color "$CYAN" "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        return 1
    fi
}

# =============================================================================
# POST-INSTALL
# =============================================================================

print_post_install() {
    printf "\n"
    printf "${BEIGE}${BOLD}╔══════════════════════════════════════════════════════════════╗${RESET}\n"
    printf "${BEIGE}${BOLD}║                                                              ║${RESET}\n"
    printf "${BEIGE}${BOLD}║${RESET}   ${ORANGE}${BOLD}Installation Complete!${RESET}                                    ${BEIGE}${BOLD}║${RESET}\n"
    printf "${BEIGE}${BOLD}║                                                              ║${RESET}\n"
    printf "${BEIGE}${BOLD}╚══════════════════════════════════════════════════════════════╝${RESET}\n"
    printf "\n"
    
    printf "${BOLD}Get started:${RESET}\n"
    printf "  ${ORANGE}gizzi-code${RESET}              Start the TUI\n"
    printf "  ${ORANGE}gizzi-code --help${RESET}       Show all commands\n"
    printf "  ${ORANGE}gizzi-code --version${RESET}    Check version\n"
    printf "\n"
    
    printf "${BOLD}Documentation:${RESET}\n"
    printf "  ${CYAN}https://docs.gizziio.com${RESET}\n"
    printf "  ${CYAN}https://github.com/Allternit/allternit-platform${RESET}\n"
    printf "\n"
    
    printf "${DIM}Need help? Run: gizzi-code --help${RESET}\n"
    printf "\n"
}

# =============================================================================
# MAIN
# =============================================================================

main() {
    printf "\n"
    print_mascot
    printf "\n"
    printf "${BEIGE}${BOLD}              GIZZI CODE - AI Terminal Interface${RESET}\n"
    printf "${DIM}                    for the Allternit Ecosystem${RESET}\n"
    printf "\n"
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --version)
                VERSION="$2"
                shift 2
                ;;
            --install-dir)
                INSTALL_DIR="$2"
                shift 2
                ;;
            --help|-h)
                printf "Gizzi Code Installer\n\n"
                printf "Usage: %s [options]\n\n" "$0"
                printf "Options:\n"
                printf "  --version <version>   Install specific version\n"
                printf "  --install-dir <dir>   Custom install directory\n"
                printf "  --help, -h            Show this help\n"
                exit 0
                ;;
            *)
                shift
                ;;
        esac
    done
    
    # Detect platform
    local platform
    local arch
    
    platform="$(detect_platform)"
    arch="$(detect_arch)"
    
    print_info "Platform: ${BEIGE}${platform}${RESET}"
    print_info "Architecture: ${BEIGE}${arch}${RESET}"
    print_info "Install directory: ${BEIGE}${INSTALL_DIR}${RESET}"
    printf "\n"
    
    # Check dependencies
    check_dependencies
    printf "\n"
    
    # Get version
    local target_version
    if [ "$VERSION" = "latest" ]; then
        target_version="$(get_latest_version)"
    else
        target_version="$VERSION"
        print_info "Installing version: ${ORANGE}${target_version}${RESET}"
    fi
    printf "\n"
    
    # Check if already installed
    if command -v gizzi-code >/dev/null 2>&1; then
        local current_version
        current_version=$(gizzi-code --version 2>/dev/null | head -1 || echo "unknown")
        printf "${YELLOW}⚠ Gizzi Code is already installed: ${ORANGE}%s${RESET}\n" "$current_version"
        print_info "Location: $(command -v gizzi-code)"
        printf "\n"
        printf "Reinstall/Update? [y/N] "
        read -r REPLY
        printf "\n"
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            print_info "Installation cancelled."
            print_post_install
            exit 0
        fi
    fi
    
    # Install
    local install_success=false
    
    if command -v npm >/dev/null 2>&1; then
        if install_via_npm; then
            install_success=true
        fi
    fi
    
    if [ "$install_success" = false ]; then
        install_binary "$platform" "$arch" "$target_version"
        add_to_path
    fi
    
    printf "\n"
    verify_installation
    printf "\n"
    
    print_post_install
}

# Run main
main "$@"
