#!/bin/bash
# =============================================================================
# Allternit PLATFORM - SERVICE STATUS CHECKER
# =============================================================================
# Usage: ./check-services.sh [options]
#
# Options:
#   --watch   - Continuously monitor services
#   --json    - Output in JSON format
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Source service configuration
source "${SCRIPT_DIR}/service-config.sh"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
GRAY='\033[0;37m'
NC='\033[0m'

# Parse options
WATCH=false
JSON=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --watch) WATCH=true ;;
        --json) JSON=true ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
    shift
done

# Check if a port is in use
port_in_use() {
    local port=$1
    lsof -Pi :"$port" -sTCP:LISTEN -t >/dev/null 2>&1
}

# Check service health
check_health() {
    local port=$1
    local path="${2:-/health}"
    curl -sf "http://127.0.0.1:${port}${path}" >/dev/null 2>&1
}

# Get process info for a port
get_process_info() {
    local port=$1
    if port_in_use "$port"; then
        lsof -Pi :"$port" -sTCP:LISTEN | tail -1 | awk '{print $1, $2}'
    else
        echo "- -"
    fi
}

# Print status in table format
print_status_table() {
    printf "\n${CYAN}%-25s %8s %10s %15s %s${NC}\n" "SERVICE" "PORT" "STATUS" "HEALTH" "PROCESS"
    printf "%s\n" "--------------------------------------------------------------------------------"
    
    declare -a services=(
        "API:$Allternit_API_PORT:/health"
        "Workspace:3021:/health"
        "Factory:${ALLTERNIT_FACTORY_PORT:-3011}:/api/factory/health"
        "Kernel:$Allternit_KERNEL_PORT:/health"
        "Policy:$Allternit_POLICY_PORT:/health"
        "Voice:$Allternit_VOICE_PORT:/health"
        "WebVM:$Allternit_WEBVM_PORT:/health"
        "Terminal:$Allternit_TERMINAL_PORT:/doc"
        "Gateway:$Allternit_GATEWAY_PORT:/health"
        "Memory:$Allternit_MEMORY_PORT:/health"
        "Registry:$Allternit_REGISTRY_PORT:/health"
        "Shell UI:$Allternit_SHELL_UI_PORT:/"
        "Redis:6379:"
        "Postgres:5432:"
    )
    
    for service_def in "${services[@]}"; do
        IFS=':' read -r name port health_path <<< "$service_def"
        
        if port_in_use "$port"; then
            status="${GREEN}UP${NC}"
            
            if [[ -n "$health_path" ]]; then
                if check_health "$port" "$health_path" 2>/dev/null; then
                    health="${GREEN}OK${NC}"
                else
                    health="${YELLOW}WARN${NC}"
                fi
            else
                health="${GRAY}N/A${NC}"
            fi
            
            read -r process pid <<< "$(get_process_info $port)"
        else
            status="${RED}DOWN${NC}"
            health="${GRAY}-"${NC}""
            process="-"
            pid=""
        fi
        
        printf "%-25s %8s %b %15b %s %s\n" "$name" "$port" "$status" "$health" "$process" "$pid"
    done
    
    printf "\n"
}

# Print status in JSON format
print_status_json() {
    echo "{"
    echo '  "services": ['
    
    declare -a services=(
        "API:$Allternit_API_PORT:/health"
        "Workspace:3021:/health"
        "Factory:${ALLTERNIT_FACTORY_PORT:-3011}:/api/factory/health"
        "Kernel:$Allternit_KERNEL_PORT:/health"
        "Policy:$Allternit_POLICY_PORT:/health"
        "Voice:$Allternit_VOICE_PORT:/health"
        "WebVM:$Allternit_WEBVM_PORT:/health"
        "Terminal:$Allternit_TERMINAL_PORT:/doc"
        "Gateway:$Allternit_GATEWAY_PORT:/health"
        "Memory:$Allternit_MEMORY_PORT:/health"
        "Shell UI:$Allternit_SHELL_UI_PORT:/"
    )
    
    local first=true
    for service_def in "${services[@]}"; do
        IFS=':' read -r name port health_path <<< "$service_def"
        
        [[ "$first" == true ]] || echo ","
        first=false
        
        local status="down"
        local health="unknown"
        local process="-"
        local pid=""
        
        if port_in_use "$port"; then
            status="up"
            
            if [[ -n "$health_path" ]]; then
                if check_health "$port" "$health_path" 2>/dev/null; then
                    health="healthy"
                else
                    health="unhealthy"
                fi
            else
                health="no_check"
            fi
            
            read -r process pid <<< "$(get_process_info $port)"
        fi
        
        printf '    {"name": "%s", "port": %s, "status": "%s", "health": "%s", "process": "%s", "pid": "%s"}' \
            "$name" "$port" "$status" "$health" "$process" "$pid"
    done
    
    echo ""
    echo '  ]'
    echo "}"
}

# Main check function
check_all() {
    if [[ "$JSON" == true ]]; then
        print_status_json
    else
        print_status_table
    fi
}

# Main execution
if [[ "$WATCH" == true ]]; then
    while true; do
        clear
        echo "Allternit Service Monitor (Ctrl+C to exit)"
        echo "Last update: $(date)"
        check_all
        sleep 2
    done
else
    check_all
fi
