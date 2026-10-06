# =============================================================================
# Allternit PLATFORM - SERVICE CONFIGURATION
# =============================================================================
# Source this file to load all service port configurations
# Usage: source ./dev/scripts/service-config.sh
# =============================================================================

# -----------------------------------------------------------------------------
# Core Platform Services (3000-3099)
# -----------------------------------------------------------------------------
export Allternit_API_PORT=3000
export Allternit_POLICY_PORT=3003
export Allternit_KERNEL_PORT=3004
export Allternit_OPERATOR_PORT=3010
export ALLTERNIT_FACTORY_PORT=3011   # Allternit Factory engine
export Allternit_LINK_CARD_PORT=3090

# -----------------------------------------------------------------------------
# AI/ML Services (8000-8009)
# -----------------------------------------------------------------------------
export Allternit_VOICE_PORT=8001
export Allternit_WEBVM_PORT=8002
export Allternit_BROWSER_RUNTIME_PORT=8003

# -----------------------------------------------------------------------------
# Gateway Services (8010-8019)
# -----------------------------------------------------------------------------
# Allternit_AGUI_PORT (8010) / Allternit_A2A_PORT (8012) retired — agui/a2a gateway packages deleted
export Allternit_GATEWAY_PORT=8013

# -----------------------------------------------------------------------------
# Infrastructure Services (3200-3299)
# -----------------------------------------------------------------------------
export Allternit_MEMORY_PORT=3200
export Allternit_REGISTRY_PORT=8080

# -----------------------------------------------------------------------------
# Terminal & Shell (4096-5199)
# -----------------------------------------------------------------------------
export Allternit_TERMINAL_PORT=4096
export Allternit_SHELL_UI_PORT=5177

# -----------------------------------------------------------------------------
# Third-Party Integration
# -----------------------------------------------------------------------------
export OPENCLAW_PORT=18789

# -----------------------------------------------------------------------------
# Service URLs (constructed from ports)
# -----------------------------------------------------------------------------
export Allternit_API_URL="http://127.0.0.1:${Allternit_API_PORT}"
export Allternit_KERNEL_URL="http://127.0.0.1:${Allternit_KERNEL_PORT}"
export Allternit_POLICY_URL="http://127.0.0.1:${Allternit_POLICY_PORT}"
export Allternit_MEMORY_URL="http://127.0.0.1:${Allternit_MEMORY_PORT}"
export Allternit_REGISTRY_URL="http://127.0.0.1:${Allternit_REGISTRY_PORT}"
export Allternit_VOICE_URL="http://127.0.0.1:${Allternit_VOICE_PORT}"
export Allternit_WEBVM_URL="http://127.0.0.1:${Allternit_WEBVM_PORT}"
export Allternit_OPERATOR_URL="http://127.0.0.1:${Allternit_OPERATOR_PORT}"
export ALLTERNIT_FACTORY_URL="http://127.0.0.1:${ALLTERNIT_FACTORY_PORT}"
export Allternit_GATEWAY_URL="http://127.0.0.1:${Allternit_GATEWAY_PORT}"
export Allternit_TERMINAL_URL="http://127.0.0.1:${Allternit_TERMINAL_PORT}"

# -----------------------------------------------------------------------------
# All Ports Array (for cleanup)
# -----------------------------------------------------------------------------
export Allternit_ALL_PORTS="${Allternit_API_PORT} ${Allternit_POLICY_PORT} ${Allternit_KERNEL_PORT} ${Allternit_OPERATOR_PORT} ${ALLTERNIT_FACTORY_PORT} ${Allternit_VOICE_PORT} ${Allternit_WEBVM_PORT} ${Allternit_GATEWAY_PORT} ${Allternit_MEMORY_PORT} ${Allternit_REGISTRY_PORT} ${Allternit_TERMINAL_PORT} ${Allternit_SHELL_UI_PORT} ${OPENCLAW_PORT}"
