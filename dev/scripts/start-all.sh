#!/bin/bash

# Allternit - Unified Startup Script
# Runs all required services in the background with a single command

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOG_DIR="$PROJECT_ROOT/.logs"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Create log directory
mkdir -p "$LOG_DIR"

print_header() {
    echo ""
    echo -e "${CYAN}╔═══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║${NC}          ${GREEN}Allternit - Spatial Agent Shell${NC}              ${CYAN}║${NC}"
    echo -e "${CYAN}║${NC}              ${BLUE}Unified Service Launcher${NC}                   ${CYAN}║${NC}"
    echo -e "${CYAN}╚═══════════════════════════════════════════════════════════╝${NC}"
    echo ""
}

print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

print_error() {
    echo -e "${RED}[✗]${NC} $1"
}

# Configuration
TERMINAL_SERVER_PORT=4096
TERMINAL_SERVER_HOST=127.0.0.1
VOICE_SERVICE_PORT=8001
SHELL_UI_PORT=5177
VITE_PREVIEW_PORT=4173
GATEWAY_PORT=8013
API_PORT=3000
FACTORY_PORT=3011
OPENCLAW_PORT=18789
LOCAL_HOST="127.0.0.1"

# Python dependencies installed for the voice service
VOICE_DEPENDENCIES=(fastapi uvicorn pydantic torch torchaudio chatterbox-tts)

install_voice_dependencies() {
    local wheelhouse_dir="$1"
    local pip_args=("${VOICE_DEPENDENCIES[@]}")

    set +e
    pip install "${pip_args[@]}"
    local status=$?
    set -e
    if [ $status -eq 0 ]; then
        return 0
    fi

    if [ -d "$wheelhouse_dir" ]; then
        set +e
        pip install --no-index --find-links="$wheelhouse_dir" "${pip_args[@]}"
        status=$?
        set -e
        if [ $status -eq 0 ]; then
            return 0
        fi
    fi

    return 1
}

# Kill existing services
cleanup() {
    print_status "Stopping existing services..."

    # Kill by port
    lsof -ti :${TERMINAL_SERVER_PORT} | xargs kill -9 2>/dev/null || true  # Terminal server
    lsof -ti :${VOICE_SERVICE_PORT} | xargs kill -9 2>/dev/null || true  # Voice service
    lsof -ti :${SHELL_UI_PORT} | xargs kill -9 2>/dev/null || true  # Vite dev server
    lsof -ti :${VITE_PREVIEW_PORT} | xargs kill -9 2>/dev/null || true  # Vite preview
    lsof -ti :${GATEWAY_PORT} | xargs kill -9 2>/dev/null || true  # Gateway service
    lsof -ti :${API_PORT} | xargs kill -9 2>/dev/null || true  # API service
    lsof -ti :${FACTORY_PORT} | xargs kill -9 2>/dev/null || true  # Factory engine
    lsof -ti :${OPENCLAW_PORT} | xargs kill -9 2>/dev/null || true  # OpenClaw service

    sleep 1
    print_success "Cleaned up existing processes"
}

# Start Terminal Server (AI Model Service)
start_terminal_server() {
    print_status "Starting Terminal Server (AI Model Service)..."

    TERMINAL_DIR="$PROJECT_ROOT/7-apps/shell/terminal"

    # Check if directory exists
    if [ ! -d "$TERMINAL_DIR" ]; then
        print_error "Terminal directory not found: $TERMINAL_DIR"
        return 1
    fi

    # Check if bun is installed
    if ! command -v bun &> /dev/null; then
        print_error "bun is not installed. Install with: curl -fsSL https://bun.sh/install | bash"
        return 1
    fi

    # Check if node_modules exists
    if [ ! -d "$TERMINAL_DIR/node_modules" ]; then
        print_warning "Installing Terminal Server dependencies..."
        (cd "$TERMINAL_DIR" && bun install)
    fi

    # Start the terminal server
    (
        cd "$TERMINAL_DIR"
        bun run src/index.ts serve --port $TERMINAL_SERVER_PORT --hostname $TERMINAL_SERVER_HOST > "$LOG_DIR/terminal-server.log" 2>&1
    ) &

    TERMINAL_PID=$!
    echo $TERMINAL_PID > "$LOG_DIR/terminal-server.pid"

    # Wait for service to be ready
    print_status "Waiting for Terminal Server to start..."
    local max_attempts=30
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -s "http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}/doc" > /dev/null 2>&1; then
            print_success "Terminal Server started on http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}"
            # Save terminal server URL for other services
            echo "TERMINAL_SERVER_URL=http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}" > "$PROJECT_ROOT/.terminal-server.env"
            return 0
        fi
        sleep 1
        attempt=$((attempt + 1))
        if [ $((attempt % 5)) -eq 0 ]; then
            print_status "Still waiting for Terminal Server... (${attempt}s)"
        fi
    done
    
    print_error "Terminal Server failed to start within ${max_attempts}s"
    print_error "Check logs: $LOG_DIR/terminal-server.log"
    return 1
}

# Start Voice Service (Rust + whisper.cpp)
start_voice_service() {
    print_status "Starting Voice Service (whisper.cpp)..."

    (
        cd "$PROJECT_ROOT"
        cargo run -p voice-service > "$LOG_DIR/voice-service.log" 2>&1
    ) &

    VOICE_PID=$!
    echo $VOICE_PID > "$LOG_DIR/voice-service.pid"

    # Wait for service to be ready
    sleep 2
    local_proto="http"
    if curl -s "${local_proto}://${LOCAL_HOST}:${VOICE_SERVICE_PORT}/health" > /dev/null 2>&1; then
        print_success "Voice Service started on ${local_proto}://${LOCAL_HOST}:${VOICE_SERVICE_PORT}"
    else
        print_warning "Voice Service starting (model loads on first request)"
    fi
}

# Start Shell UI (Vite)
start_shell_ui() {
    print_status "Starting Shell UI..."

    SHELL_DIR="$PROJECT_ROOT/7-apps/shell/web"

    # Check if node_modules exists
    if [ ! -d "$SHELL_DIR/node_modules" ]; then
        print_warning "Installing Shell UI dependencies..."
        (cd "$SHELL_DIR" && npm install)
    fi

    # Create .env.development.local to point to Terminal Server
    cat > "$SHELL_DIR/.env.development.local" << EOF
# Auto-generated by start-all.sh
# Points to Terminal Server for AI model APIs
VITE_TERMINAL_SERVER_URL=http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}
EOF

    print_status "Shell UI configured to use Terminal Server at http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}"

    # Start dev server
    (
        cd "$SHELL_DIR"
        npm run dev > "$LOG_DIR/shell-ui.log" 2>&1
    ) &

    SHELL_PID=$!
    echo $SHELL_PID > "$LOG_DIR/shell-ui.pid"

    # Wait for service to be ready
    sleep 3
    local_proto="http"
    if curl -s "${local_proto}://${LOCAL_HOST}:${SHELL_UI_PORT}" > /dev/null 2>&1; then
        print_success "Shell UI started on ${local_proto}://${LOCAL_HOST}:${SHELL_UI_PORT}"
    else
        print_warning "Shell UI starting..."
    fi
}

# Start API Service (Rust)
start_api_service() {
    print_status "Starting API Service (Rust)..."

    API_DIR="$PROJECT_ROOT/7-apps/api"

    # Start the Rust API service
    (
        cd "$API_DIR"
        cargo run --release > "$LOG_DIR/api-service.log" 2>&1
    ) &

    API_PID=$!
    echo $API_PID > "$LOG_DIR/api-service.pid"

    # Wait for service to be ready (may take longer for first compile)
    print_status "Waiting for API service to compile and start..."
    for i in {1..60}; do
        if curl -s "http://${LOCAL_HOST}:${API_PORT}/health" > /dev/null 2>&1; then
            print_success "API Service started on http://${LOCAL_HOST}:${API_PORT}"
            return 0
        fi
        sleep 2
        if [ $((i % 10)) -eq 0 ]; then
            print_status "Still compiling... (${i}s)"
        fi
    done
    
    print_warning "API Service still starting in background..."
}

# Start the Allternit Factory engine (agents, orchestration, workflows, workspace)
start_factory_engine() {
    print_status "Starting the Factory engine..."

    # Start the engine
    (
        cd "$PROJECT_ROOT"
        cargo run -p allternit-factory --release -- --root "$PROJECT_ROOT" serve --port "$FACTORY_PORT" > "$LOG_DIR/factory.log" 2>&1
    ) &

    FACTORY_PID=$!
    echo $FACTORY_PID > "$LOG_DIR/factory.pid"

    # Wait for service to be ready
    sleep 2
    local_proto="http"
    if curl -s "${local_proto}://${LOCAL_HOST}:${FACTORY_PORT}/api/factory/health" > /dev/null 2>&1; then
        print_success "Factory engine started on ${local_proto}://${LOCAL_HOST}:${FACTORY_PORT}"
    else
        print_warning "Factory engine starting..."
    fi
}

# Show service URLs
show_urls() {
    local_proto="http"
    echo ""
    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}Services Running:${NC}"
    echo ""
    echo -e "  ${BLUE}🤖 Terminal Server:${NC} http://${LOCAL_HOST}:${TERMINAL_SERVER_PORT}"
    echo -e "     └─ AI Model API for Web/Desktop apps"
    echo ""
    echo -e "  ${BLUE}🌐 Shell UI:${NC}        ${local_proto}://${LOCAL_HOST}:${SHELL_UI_PORT}"
    echo -e "  ${BLUE}🔊 Voice Service:${NC}   ${local_proto}://${LOCAL_HOST}:${VOICE_SERVICE_PORT}"
    echo -e "  ${BLUE}⚙️  API Service:${NC}     ${local_proto}://${LOCAL_HOST}:${API_PORT}"
    echo -e "  ${BLUE}🐾 OpenClaw:${NC}        ${local_proto}://${LOCAL_HOST}:${OPENCLAW_PORT}"
    echo ""
    echo -e "${CYAN}═══════════════════════════════════════════════════════════${NC}"
    echo ""
}

# Start OpenClaw Service
start_openclaw_service() {
    print_status "Starting OpenClaw Service..."

    # Check if openclaw is installed
    if ! command -v openclaw &> /dev/null; then
        print_warning "OpenClaw not installed. Install with: npm install -g openclaw@latest"
        return 1
    fi

    # Kill any existing OpenClaw processes on the port first
    lsof -ti :${OPENCLAW_PORT} | xargs kill -9 2>/dev/null || true
    sleep 1

    # Generate a secure random token for this session
    OPENCLAW_TOKEN=$(openssl rand -hex 32 2>/dev/null || head -c 64 /dev/urandom | xxd -p | head -c 64)
    
    # Save token to env file for Shell UI to read
    echo "OPENCLAW_GATEWAY_TOKEN=${OPENCLAW_TOKEN}" > "$PROJECT_ROOT/.openclaw.env"
    echo "OPENCLAW_PORT=${OPENCLAW_PORT}" >> "$PROJECT_ROOT/.openclaw.env"
    
    # Create OpenClaw config with insecure auth allowed for local dev
    OPENCLAW_CONFIG_DIR="${HOME}/.openclaw"
    mkdir -p "$OPENCLAW_CONFIG_DIR"
    
    # Backup existing config
    if [ -f "$OPENCLAW_CONFIG_DIR/openclaw.json" ]; then
        cp "$OPENCLAW_CONFIG_DIR/openclaw.json" "$OPENCLAW_CONFIG_DIR/openclaw.json.backup.$(date +%s)" 2>/dev/null || true
    fi
    
    # Set required config values using openclaw CLI (preserves other settings)
    print_status "Configuring OpenClaw gateway..."
    export OPENCLAW_GATEWAY_TOKEN="${OPENCLAW_TOKEN}"
    openclaw config set gateway.port ${OPENCLAW_PORT}
    openclaw config set gateway.mode local
    openclaw config set gateway.auth.token "${OPENCLAW_TOKEN}"
    openclaw config set gateway.controlUi.allowInsecureAuth true

    print_status "OpenClaw gateway token configured"

    # Start OpenClaw gateway in background (NOT in subshell)
    nohup openclaw gateway --port ${OPENCLAW_PORT} > "$LOG_DIR/openclaw-service.log" 2>&1 &

    OPENCLAW_PID=$!
    echo $OPENCLAW_PID > "$LOG_DIR/openclaw-service.pid"

    # Wait for service to be ready
    sleep 2
    for i in {1..15}; do
        if curl -s "http://${LOCAL_HOST}:${OPENCLAW_PORT}/health" > /dev/null 2>&1; then
            print_success "OpenClaw Service started on http://${LOCAL_HOST}:${OPENCLAW_PORT}"
            print_status "Gateway token saved to .openclaw.env"
            return 0
        fi
        sleep 1
    done
    
    # Check if process is still running
    if ! kill -0 $OPENCLAW_PID 2>/dev/null; then
        print_error "OpenClaw process died. Check logs: $LOG_DIR/openclaw-service.log"
        cat "$LOG_DIR/openclaw-service.log" | tail -20
        return 1
    fi
    
    print_warning "OpenClaw Service starting (PID: $OPENCLAW_PID)..."
}

# Stop all services
stop_services() {
    print_status "Stopping all Allternit services..."

    # Kill by PID files
    for pidfile in "$LOG_DIR"/*.pid; do
        if [ -f "$pidfile" ]; then
            pid=$(cat "$pidfile")
            kill -9 "$pid" 2>/dev/null || true
            rm "$pidfile"
        fi
    done

    # Also kill by port (backup)
    lsof -ti :${TERMINAL_SERVER_PORT} | xargs kill -9 2>/dev/null || true
    lsof -ti :${VOICE_SERVICE_PORT} | xargs kill -9 2>/dev/null || true
    lsof -ti :${SHELL_UI_PORT} | xargs kill -9 2>/dev/null || true
    lsof -ti :${API_PORT} | xargs kill -9 2>/dev/null || true
    lsof -ti :${FACTORY_PORT} | xargs kill -9 2>/dev/null || true
    lsof -ti :${OPENCLAW_PORT} | xargs kill -9 2>/dev/null || true

    # Clean up env files
    rm -f "$PROJECT_ROOT/.terminal-server.env"

    print_success "All services stopped"
}

run_bootstrap_quickjs() {
    print_status "Bootstrapping Allternit Usage QuickJS env..."
    if [ -x "$PROJECT_ROOT/scripts/bootstrap-operator-quickjs-venv.sh" ]; then
        "$PROJECT_ROOT/scripts/bootstrap-operator-quickjs-venv.sh"
        print_success "Allternit Usage QuickJS runtime bootstrapped"
    else
        print_warning "QuickJS bootstrap missing; install manually before running the operator"
    fi
}

# Main execution
main() {
    case "${1:-start}" in
        start)
            print_header
            run_bootstrap_quickjs
            cleanup
            run_bootstrap_quickjs
            
            # Start Terminal Server FIRST (required by other services)
            start_terminal_server || exit 1
            
            start_voice_service
            start_api_service
            start_factory_engine
            start_openclaw_service
            start_shell_ui
            show_urls

            echo -e "${GREEN}All services started! Press Ctrl+C to stop.${NC}"
            echo ""

            # Keep script running and forward signals
            trap 'stop_services; exit 0' INT TERM

            # Wait for background processes
            wait
            ;;
        stop)
            stop_services
            ;;
        restart)
            stop_services
            sleep 2
            exec "$0" start
            ;;
        status)
            echo ""
            echo -e "${CYAN}Service Status:${NC}"
            echo ""
            if curl -s "http://${TERMINAL_SERVER_HOST}:${TERMINAL_SERVER_PORT}/doc" > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} Terminal Server (port ${TERMINAL_SERVER_PORT})"
            else
                echo -e "  ${RED}✗${NC} Terminal Server (port ${TERMINAL_SERVER_PORT})"
            fi
            if lsof -ti ${VOICE_SERVICE_PORT} > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} Voice Service (port ${VOICE_SERVICE_PORT})"
            else
                echo -e "  ${RED}✗${NC} Voice Service (port ${VOICE_SERVICE_PORT})"
            fi
            if lsof -ti ${SHELL_UI_PORT} > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} Shell UI (port ${SHELL_UI_PORT})"
            else
                echo -e "  ${RED}✗${NC} Shell UI (port ${SHELL_UI_PORT})"
            fi
            if lsof -ti ${API_PORT} > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} API Service (port ${API_PORT})"
            else
                echo -e "  ${RED}✗${NC} API Service (port ${API_PORT})"
            fi
            if lsof -ti ${FACTORY_PORT} > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} Factory engine (port ${FACTORY_PORT})"
            else
                echo -e "  ${RED}✗${NC} Factory engine (port ${FACTORY_PORT})"
            fi
            if lsof -ti ${OPENCLAW_PORT} > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} OpenClaw (port ${OPENCLAW_PORT})"
            else
                echo -e "  ${RED}✗${NC} OpenClaw (port ${OPENCLAW_PORT})"
            fi
            echo ""
            ;;
        logs)
            SERVICE="${2:-all}"
            case $SERVICE in
                terminal|term|t)
                    tail -f "$LOG_DIR/terminal-server.log"
                    ;;
                voice|v)
                    tail -f "$LOG_DIR/voice-service.log"
                    ;;
                shell|s)
                    tail -f "$LOG_DIR/shell-ui.log"
                    ;;
                api|a)
                    tail -f "$LOG_DIR/api-service.log"
                    ;;
                factory|f)
                    tail -f "$LOG_DIR/factory.log"
                    ;;
                openclaw|oc)
                    tail -f "$LOG_DIR/openclaw-service.log"
                    ;;
                all|*)
                    tail -f "$LOG_DIR"/*.log
                    ;;
            esac
            ;;
        *)
            echo "Usage: $0 {start|stop|restart|status|logs [terminal|voice|shell|api|factory|openclaw|all]}"
            exit 1
            ;;
    esac
}

main "$@"
