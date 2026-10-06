#!/bin/bash
# =============================================================================
# Allternit PLATFORM - COMPLETE SERVICE STARTUP SCRIPT
# =============================================================================
# This script starts all Allternit platform services in the correct dependency order
# 
# Usage:
#   ./start-all-services.sh [mode] [options]
#
# Modes:
#   core      - Start only core services (API, Factory engine, Workspace)
#   standard  - Start core + AI services (Voice, WebVM) [default]
#   full      - Start all services including infrastructure
#   dev       - Start with hot-reload development mode
#
# Options:
#   --build   - Force rebuild services before starting
#   --detach  - Run services in background (tmux)
#   --logs    - Show logs after starting
#
# Examples:
#   ./start-all-services.sh core
#   ./start-all-services.sh standard --build
#   ./start-all-services.sh full --detach
# =============================================================================

set -euo pipefail

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Source service configuration
source "${SCRIPT_DIR}/service-config.sh"

# =============================================================================
# CONFIGURATION
# =============================================================================

# Mode selection
MODE="${1:-standard}"
shift || true

# Parse options
BUILD=false
DETACH=false
SHOW_LOGS=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --build) BUILD=true ;;
        --detach) DETACH=true ;;
        --logs) SHOW_LOGS=true ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
    shift
done

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# =============================================================================
# LOGGING FUNCTIONS
# =============================================================================

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_service() { echo -e "${CYAN}[SERVICE]${NC} $1"; }

# =============================================================================
# UTILITY FUNCTIONS
# =============================================================================

# Check if a port is in use
port_in_use() {
    local port=$1
    if lsof -Pi :"$port" -sTCP:LISTEN -t >/dev/null 2>&1; then
        return 0
    else
        return 1
    fi
}

# Check if a service is running on a specific port
check_service() {
    local name=$1
    local port=$2
    local health_path="${3:-/health}"
    
    if curl -sf "http://127.0.0.1:${port}${health_path}" >/dev/null 2>&1; then
        return 0
    else
        return 1
    fi
}

# Wait for a service to be ready
wait_for_service() {
    local name=$1
    local port=$2
    local max_attempts=${3:-30}
    local health_path="${4:-/health}"
    
    log_info "Waiting for $name on port $port..."
    
    for i in $(seq 1 $max_attempts); do
        if check_service "$name" "$port" "$health_path"; then
            log_success "$name is ready!"
            return 0
        fi
        sleep 1
    done
    
    log_error "$name failed to start after ${max_attempts}s"
    return 1
}

# Kill process on a port
kill_port() {
    local port=$1
    if port_in_use "$port"; then
        log_warn "Killing process on port $port"
        lsof -ti :"$port" | xargs kill -9 2>/dev/null || true
        sleep 1
    fi
}

# =============================================================================
# BUILD FUNCTIONS
# =============================================================================

build_rust_service() {
    local service_name=$1
    local service_path=$2
    local binary_name=$3
    
    log_info "Building $service_name..."
    
    cd "$PROJECT_ROOT"
    
    if cargo build --release -p "$service_name" 2>&1 | tee /tmp/build-${service_name}.log; then
        log_success "$service_name built successfully"
        return 0
    else
        log_error "Failed to build $service_name"
        cat /tmp/build-${service_name}.log
        return 1
    fi
}

build_all_rust_services() {
    log_info "Building all Rust services..."
    cd "$PROJECT_ROOT"
    
    # Core services
    build_rust_service "allternit-api" "7-apps/api" "allternit-api"
    build_rust_service "workspace-service" "4-services/orchestration/workspace-service" "workspace-service"
    build_rust_service "allternit-factory" "cmd/allternit-factory" "allternit-factory"
    build_rust_service "kernel" "4-services/orchestration/kernel-service" "kernel"
    
    # AI/ML services
    if [[ "$MODE" == "full" ]] || [[ "$MODE" == "standard" ]]; then
        build_rust_service "voice-service" "services/voice" "voice-service"
        build_rust_service "webvm-service" "3-adapters/bridge-systems/allternit-webvm" "webvm-service"
    fi
    
    # Additional full-mode services
    if [[ "$MODE" == "full" ]]; then
        build_rust_service "policy-service" "4-services/orchestration/policy-service" "policy-service"
        build_rust_service "pattern-service" "4-services/ml-ai-services/pattern-service" "pattern-service"
        build_rust_service "io-service" "4-services/io-service" "io-service"
    fi
}

# =============================================================================
# SERVICE START FUNCTIONS
# =============================================================================

# Start API Service (port 3000)
start_api() {
    local binary="${PROJECT_ROOT}/target/release/allternit-api"
    
    if port_in_use "$Allternit_API_PORT"; then
        log_warn "API service already running on port $Allternit_API_PORT"
        return 0
    fi
    
    log_service "Starting API Service on port $Allternit_API_PORT"
    
    export Allternit_API_BIND="0.0.0.0:${Allternit_API_PORT}"
    export Allternit_LEDGER_PATH="${PROJECT_ROOT}/.data/allternit.jsonl"
    export Allternit_DB_PATH="${PROJECT_ROOT}/.data/allternit.db"
    export Allternit_API_IDENTITY="api-service"
    export Allternit_API_TENANT="default"
    export Allternit_API_BOOTSTRAP_POLICY="true"
    export Allternit_API_POLICY_ENFORCE="true"
    
    mkdir -p "${PROJECT_ROOT}/.data"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/api.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/api.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/api.pid"
    fi
    
    wait_for_service "API" "$Allternit_API_PORT"
}

# Start Workspace Service (port 3021)
start_workspace() {
    local binary="${PROJECT_ROOT}/target/release/workspace-service"
    
    if port_in_use "3021"; then
        log_warn "Workspace service already running on port 3021"
        return 0
    fi
    
    log_service "Starting Workspace Service on port 3021"
    
    export WORKSPACE_SERVICE_PORT="3021"
    export WORKSPACE_SERVICE_BIND="0.0.0.0"
    export RUST_LOG="info"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/workspace.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/workspace.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/workspace.pid"
    fi
    
    wait_for_service "Workspace" "3021"
}

# Start the Allternit Factory engine (port 3011)
start_factory() {
    local binary="${PROJECT_ROOT}/target/release/allternit-factory"
    local port="${ALLTERNIT_FACTORY_PORT:-3011}"

    if port_in_use "$port"; then
        log_warn "Factory engine already running on port $port"
        return 0
    fi

    log_service "Starting the Factory engine on port $port"

    export RUST_LOG="info"

    if [[ "$DETACH" == true ]]; then
        nohup "$binary" --root "$PROJECT_ROOT" serve --port "$port" > "${PROJECT_ROOT}/.logs/factory.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/factory.pid"
    else
        "$binary" --root "$PROJECT_ROOT" serve --port "$port" &
        echo $! > "${PROJECT_ROOT}/.pids/factory.pid"
    fi

    sleep 2
    if curl -fsS "http://127.0.0.1:${port}/api/factory/health" > /dev/null 2>&1; then
        log_success "Factory engine is ready!"
        return 0
    else
        log_error "Factory engine failed to start (see .logs/factory.log)"
        return 1
    fi
}

# Start Kernel Service (port 3004)
start_kernel() {
    local binary="${PROJECT_ROOT}/target/release/kernel"
    
    if port_in_use "$Allternit_KERNEL_PORT"; then
        log_warn "Kernel service already running on port $Allternit_KERNEL_PORT"
        return 0
    fi
    
    log_service "Starting Kernel Service on port $Allternit_KERNEL_PORT"
    
    export PORT="$Allternit_KERNEL_PORT"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/kernel.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/kernel.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/kernel.pid"
    fi
    
    wait_for_service "Kernel" "$Allternit_KERNEL_PORT"
}

# Start Policy Service (port 3003)
start_policy() {
    local binary="${PROJECT_ROOT}/target/release/policy-service"
    
    if port_in_use "$Allternit_POLICY_PORT"; then
        log_warn "Policy service already running on port $Allternit_POLICY_PORT"
        return 0
    fi
    
    log_service "Starting Policy Service on port $Allternit_POLICY_PORT"
    
    export PORT="$Allternit_POLICY_PORT"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/policy.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/policy.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/policy.pid"
    fi
    
    wait_for_service "Policy" "$Allternit_POLICY_PORT"
}

# Start Voice Service (port 8001)
start_voice() {
    local binary="${PROJECT_ROOT}/target/release/voice-service"
    
    if port_in_use "$Allternit_VOICE_PORT"; then
        log_warn "Voice service already running on port $Allternit_VOICE_PORT"
        return 0
    fi
    
    log_service "Starting Voice Service on port $Allternit_VOICE_PORT"
    
    export PORT="$Allternit_VOICE_PORT"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/voice.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/voice.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/voice.pid"
    fi
    
    wait_for_service "Voice" "$Allternit_VOICE_PORT"
}

# Start WebVM Service (port 8002)
start_webvm() {
    local binary="${PROJECT_ROOT}/target/release/webvm-service"
    
    if port_in_use "$Allternit_WEBVM_PORT"; then
        log_warn "WebVM service already running on port $Allternit_WEBVM_PORT"
        return 0
    fi
    
    log_service "Starting WebVM Service on port $Allternit_WEBVM_PORT"
    
    export PORT="$Allternit_WEBVM_PORT"
    
    if [[ "$DETACH" == true ]]; then
        nohup "$binary" > "${PROJECT_ROOT}/.logs/webvm.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/webvm.pid"
    else
        "$binary" &
        echo $! > "${PROJECT_ROOT}/.pids/webvm.pid"
    fi
    
    wait_for_service "WebVM" "$Allternit_WEBVM_PORT"
}

# Start UI Development Server (port 5177)
start_ui() {
    if port_in_use "$Allternit_SHELL_UI_PORT"; then
        log_warn "UI already running on port $Allternit_SHELL_UI_PORT"
        return 0
    fi
    
    log_service "Starting Shell UI on port $Allternit_SHELL_UI_PORT"
    
    cd "${PROJECT_ROOT}/6-ui/allternit-platform"
    
    # Install dependencies if needed
    if [[ ! -d "node_modules" ]]; then
        log_info "Installing npm dependencies..."
        npm install
    fi
    
    if [[ "$DETACH" == true ]]; then
        nohup npm run dev -- --port "$Allternit_SHELL_UI_PORT" > "${PROJECT_ROOT}/.logs/ui.log" 2>&1 &
        echo $! > "${PROJECT_ROOT}/.pids/ui.pid"
    else
        npm run dev -- --port "$Allternit_SHELL_UI_PORT" &
        echo $! > "${PROJECT_ROOT}/.pids/ui.pid"
    fi
    
    wait_for_service "UI" "$Allternit_SHELL_UI_PORT" "/" || log_warn "UI might need more time to build"
}

# =============================================================================
# MAIN EXECUTION
# =============================================================================

main() {
    log_info "Allternit Platform Service Manager"
    log_info "Mode: $MODE"
    log_info "Project Root: $PROJECT_ROOT"
    
    # Create necessary directories
    mkdir -p "${PROJECT_ROOT}/.logs"
    mkdir -p "${PROJECT_ROOT}/.pids"
    mkdir -p "${PROJECT_ROOT}/.data"
    
    # Build if requested or if binaries don't exist
    if [[ "$BUILD" == true ]]; then
        build_all_rust_services
    fi
    
    # Start services based on mode
    case "$MODE" in
        core)
            log_info "Starting CORE services..."
            start_api
            start_workspace
            start_factory
            ;;
            
        standard)
            log_info "Starting STANDARD services (core + AI)..."
            start_api
            start_workspace
            start_factory
            start_kernel
            start_voice
            start_webvm
            start_ui
            ;;
            
        full)
            log_info "Starting FULL services (all)..."
            start_api
            start_workspace
            start_factory
            start_kernel
            start_policy
            start_voice
            start_webvm
            start_ui
            ;;
            
        dev)
            log_info "Starting DEVELOPMENT mode..."
            start_api
            start_workspace
            start_factory
            start_ui
            log_info "Development services started with hot-reload"
            ;;
            
        *)
            log_error "Unknown mode: $MODE"
            echo "Valid modes: core, standard, full, dev"
            exit 1
            ;;
    esac
    
    # Show status
    log_info "=============================================="
    log_info "SERVICE STATUS"
    log_info "=============================================="
    
    check_service "API" "$Allternit_API_PORT" && log_success "API: http://127.0.0.1:${Allternit_API_PORT}" || log_error "API: Not running"
    port_in_use "3021" && log_success "Workspace: http://127.0.0.1:3021" || log_error "Workspace: Not running"
    port_in_use "${ALLTERNIT_FACTORY_PORT:-3011}" && log_success "Factory engine: http://127.0.0.1:${ALLTERNIT_FACTORY_PORT:-3011}" || log_error "Factory engine: Not running"
    check_service "Kernel" "$Allternit_KERNEL_PORT" && log_success "Kernel: http://127.0.0.1:${Allternit_KERNEL_PORT}" || log_warn "Kernel: Not running"
    check_service "Voice" "$Allternit_VOICE_PORT" && log_success "Voice: http://127.0.0.1:${Allternit_VOICE_PORT}" || log_warn "Voice: Not running"
    check_service "WebVM" "$Allternit_WEBVM_PORT" && log_success "WebVM: http://127.0.0.1:${Allternit_WEBVM_PORT}" || log_warn "WebVM: Not running"
    port_in_use "$Allternit_SHELL_UI_PORT" && log_success "UI: http://127.0.0.1:${Allternit_SHELL_UI_PORT}" || log_warn "UI: Not running"
    
    log_info "=============================================="
    log_info "Console Drawer Terminal should now work!"
    log_info "Open: http://127.0.0.1:${Allternit_SHELL_UI_PORT}"
    log_info "=============================================="
    
    # Show logs if requested
    if [[ "$SHOW_LOGS" == true ]] && [[ "$DETACH" == true ]]; then
        tail -f "${PROJECT_ROOT}/.logs/"*.log
    fi
}

# Handle cleanup on exit
cleanup() {
    log_info "Cleaning up..."
    # Kill all services started by this script
    if [[ -d "${PROJECT_ROOT}/.pids" ]]; then
        for pid_file in "${PROJECT_ROOT}/.pids/"*.pid; do
            if [[ -f "$pid_file" ]]; then
                local pid
                pid=$(cat "$pid_file")
                if kill -0 "$pid" 2>/dev/null; then
                    kill "$pid" 2>/dev/null || true
                fi
                rm -f "$pid_file"
            fi
        done
    fi
}

trap cleanup EXIT INT TERM

# Run main function
main "$@"
