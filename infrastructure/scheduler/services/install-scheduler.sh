#!/usr/bin/env bash
#
# Allternit Cloud Scheduler Setup Wizard
#
# Installs the cloud scheduler as an always-on background service.
# - Explicit opt-in prompt
# - Interactive prompts for required settings
# - Input validation
# - Creates config, installs binary, installs service
# - Supports unattended mode via environment variables
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../../" && pwd)"
BINARY_SRC="$REPO_ROOT/target/release/allternit-scheduler"
BINARY_DST="/usr/local/bin/allternit-scheduler"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_header() {
    echo -e "${BLUE}"
    echo "╔════════════════════════════════════════════════════════════╗"
    echo "║      Allternit Cloud Scheduler Setup Wizard                ║"
    echo "╚════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
}

print_success() { echo -e "${GREEN}✓${NC} $1"; }
print_info()    { echo -e "${BLUE}ℹ${NC} $1"; }
print_warning() { echo -e "${YELLOW}⚠${NC} $1"; }
print_error()   { echo -e "${RED}✗${NC} $1"; }

ask_yes_no() {
    local prompt="$1"
    local default="${2:-N}"
    while true; do
        if [[ "$default" == "Y" ]]; then
            read -rp "$prompt [Y/n]: " answer
            answer="${answer:-Y}"
        else
            read -rp "$prompt [y/N]: " answer
            answer="${answer:-N}"
        fi
        case "$answer" in
            [Yy]* ) return 0;;
            [Nn]* ) return 1;;
            * ) echo "Please answer yes or no.";;
        esac
    done
}

prompt_with_default() {
    local prompt="$1"
    local default="$2"
    local result
    read -rp "$prompt [$default]: " result
    echo "${result:-$default}"
}

prompt_required() {
    local prompt="$1"
    local result
    while true; do
        read -rp "$prompt: " result
        if [[ -n "${result// /}" ]]; then
            echo "$result"
            return 0
        fi
        print_error "This field is required."
    done
}

prompt_secret() {
    local prompt="$1"
    local result
    while true; do
        read -rsp "$prompt: " result
        echo >&2
        if [[ -n "$result" ]]; then
            echo "$result"
            return 0
        fi
        print_error "This field is required." >&2
    done
}

print_header

echo "The Cloud Scheduler runs scheduled tasks even when your desktop app is closed."
echo "It is installed as a background service on this machine or a server."
echo ""

if [[ "${ALLTERNIT_SCHEDULER_UNATTENDED:-false}" == "true" ]]; then
    echo "Running in unattended mode using environment variables."
    INSTALL_SCHEDULER="${INSTALL_SCHEDULER:-yes}"
else
    if ! ask_yes_no "Do you want to enable Allternit Cloud Scheduling?" "N"; then
        echo ""
        print_info "Skipped. Cloud scheduling will not be available."
        print_info "You can re-run this wizard anytime with:"
        print_info "  ./infrastructure/scheduler/services/install-scheduler.sh"
        exit 0
    fi
    INSTALL_SCHEDULER="yes"
fi

# Gather configuration
if [[ "$INSTALL_SCHEDULER" == "yes" ]]; then
    echo ""
    echo "Scheduler Configuration"
    echo "-----------------------"

    if [[ -n "${ALLTERNIT_SCHEDULER_DATABASE_URL:-}" ]]; then
        DATABASE_URL="$ALLTERNIT_SCHEDULER_DATABASE_URL"
        print_info "Using database URL from environment."
    else
        DEFAULT_DB="postgres://localhost/allternit"
        DATABASE_URL="$(prompt_with_default "Postgres database URL (control-plane DB with the schedules table)" "$DEFAULT_DB")"
    fi

    if [[ -n "${ALLTERNIT_SCHEDULER_API_URL:-}" ]]; then
        API_URL="$ALLTERNIT_SCHEDULER_API_URL"
        print_info "Using API URL from environment."
    else
        API_URL="$(prompt_with_default "Control plane API URL" "http://127.0.0.1:3001")"
    fi

    if [[ -n "${ALLTERNIT_SCHEDULER_API_KEY:-}" ]]; then
        API_KEY="$ALLTERNIT_SCHEDULER_API_KEY"
        print_info "Using API key from environment."
    else
        echo ""
        print_info "The API key authenticates the scheduler with the Allternit control plane."
        API_KEY="$(prompt_secret "Operator API key")"
    fi

    if [[ -n "${ALLTERNIT_SCHEDULER_EXECUTION_MODE:-}" ]]; then
        EXECUTION_MODE="$ALLTERNIT_SCHEDULER_EXECUTION_MODE"
    else
        echo ""
        echo "Execution mode:"
        echo "  api   — trigger runs via the control plane API (recommended for cloud)"
        echo "  local — run commands directly on this machine"
        EXECUTION_MODE="$(prompt_with_default "Execution mode" "api")"
    fi

    if [[ -n "${ALLTERNIT_SCHEDULER_POLL_INTERVAL_SECS:-}" ]]; then
        POLL_INTERVAL="$ALLTERNIT_SCHEDULER_POLL_INTERVAL_SECS"
    else
        POLL_INTERVAL="$(prompt_with_default "Poll interval (seconds)" "60")"
    fi

    # Validate execution mode
    if [[ "$EXECUTION_MODE" != "api" && "$EXECUTION_MODE" != "local" ]]; then
        print_error "Execution mode must be 'api' or 'local'."
        exit 1
    fi

    # Validate poll interval is a number
    if ! [[ "$POLL_INTERVAL" =~ ^[0-9]+$ ]]; then
        print_error "Poll interval must be a positive integer."
        exit 1
    fi

    echo ""
    print_info "Configuration summary:"
    echo "  Database URL:     $DATABASE_URL"
    echo "  API URL:          $API_URL"
    echo "  Execution mode:   $EXECUTION_MODE"
    echo "  Poll interval:    ${POLL_INTERVAL}s"
    echo "  API key:          (hidden)"

    if [[ "${ALLTERNIT_SCHEDULER_UNATTENDED:-false}" != "true" ]]; then
        if ! ask_yes_no "Install with these settings?" "Y"; then
            print_info "Installation cancelled."
            exit 0
        fi
    fi
fi

# Build binary if missing
if [[ ! -f "$BINARY_SRC" ]]; then
    echo ""
    print_info "Release binary not found. Building allternit-scheduler..."
    cd "$REPO_ROOT"
    cargo build --release -p allternit-scheduler
    print_success "Build complete."
fi

# Install binary
echo ""
print_info "Installing scheduler binary to $BINARY_DST..."
sudo cp "$BINARY_SRC" "$BINARY_DST"
sudo chmod +x "$BINARY_DST"
print_success "Binary installed."

OS="$(uname -s)"
case "$OS" in
    Linux)
        ENV_FILE="/etc/allternit-scheduler/env"
        sudo mkdir -p /etc/allternit-scheduler
        sudo mkdir -p /var/lib/allternit

        print_info "Writing configuration to $ENV_FILE..."
        sudo tee "$ENV_FILE" >/dev/null <<EOF
ALLTERNIT_SCHEDULER_DATABASE_URL=$DATABASE_URL
ALLTERNIT_SCHEDULER_API_URL=$API_URL
ALLTERNIT_SCHEDULER_API_KEY=$API_KEY
ALLTERNIT_SCHEDULER_POLL_INTERVAL_SECS=$POLL_INTERVAL
ALLTERNIT_SCHEDULER_EXECUTION_MODE=$EXECUTION_MODE
EOF
        sudo chmod 600 "$ENV_FILE"

        # Create allternit user if it does not exist
        if ! id -u allternit >/dev/null 2>&1; then
            print_info "Creating allternit system user..."
            sudo useradd --system --no-create-home --home-dir /var/lib/allternit allternit
        fi
        sudo chown allternit:allternit /var/lib/allternit

        print_info "Installing systemd service..."
        sudo cp "$REPO_ROOT/infrastructure/scheduler/services/allternit-scheduler.service" /etc/systemd/system/
        sudo systemctl daemon-reload
        sudo systemctl enable allternit-scheduler

        echo ""
        print_success "Cloud scheduler installed."
        echo ""
        print_info "Start it now with:"
        echo "  sudo systemctl start allternit-scheduler"
        echo ""
        print_info "Check status with:"
        echo "  sudo systemctl status allternit-scheduler"
        echo "  sudo journalctl -u allternit-scheduler -f"
        ;;

    Darwin)
        PLIST_SRC="$REPO_ROOT/infrastructure/scheduler/services/com.allternit.scheduler.plist"
        PLIST_DST="$HOME/Library/LaunchAgents/com.allternit.scheduler.plist"
        LOG_DIR="$HOME/Library/Logs"
        mkdir -p "$LOG_DIR"

        # Expand the plist template
        print_info "Installing launchd user agent..."
        sed \
            -e "s|REPLACE_ME_DATABASE_URL|$DATABASE_URL|g" \
            -e "s|REPLACE_ME_API_URL|$API_URL|g" \
            -e "s|REPLACE_ME_EXECUTION_MODE|$EXECUTION_MODE|g" \
            -e "s|REPLACE_ME_POLL_INTERVAL|$POLL_INTERVAL|g" \
            -e "s|REPLACE_ME_API_KEY|$API_KEY|g" \
            -e "s|REPLACE_ME_LOG_DIR|$LOG_DIR|g" \
            "$PLIST_SRC" > "$PLIST_DST"
        chmod 600 "$PLIST_DST"

        # Unload first in case it is already loaded
        launchctl unload "$PLIST_DST" 2>/dev/null || true
        launchctl load "$PLIST_DST"
        launchctl start com.allternit.scheduler

        echo ""
        print_success "Cloud scheduler installed and started."
        echo ""
        print_info "Check status with:"
        echo "  launchctl list com.allternit.scheduler"
        echo "  tail -f $LOG_DIR/allternit-scheduler.log"
        ;;

    *)
        print_error "Unsupported OS: $OS"
        print_info "Binary installed at $BINARY_DST. You will need to configure your own service manager."
        exit 1
        ;;
esac

echo ""
print_success "Setup complete."
print_info "Full documentation: docs/cloud-scheduler-setup.md"
