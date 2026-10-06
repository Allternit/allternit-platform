#!/bin/bash
# Seed the VPS Desktop Cloud deployment with the e2e bot and template.
# Usage: SELF_HOSTED_TOKEN=<token> ./seed-e2e.sh [API_BASE_URL]
set -euo pipefail

API_BASE_URL="${1:-https://mail.news.allternit.com}"
TOKEN="${SELF_HOSTED_TOKEN:-}"

if [ -z "${TOKEN}" ]; then
  echo "SELF_HOSTED_TOKEN is required" >&2
  exit 1
fi

AUTH_HEADER="X-Allternit-Self-Hosted-Token: ${TOKEN}"

echo "Seeding bot and template on ${API_BASE_URL}..."

curl -s -X POST "${API_BASE_URL}/api/v1/agents" \
  -H "${AUTH_HEADER}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "desktop-cloud-e2e-bot",
    "description": "Test bot for Desktop Cloud end-to-end test",
    "model": "gpt-4o",
    "provider": "openai",
    "type": "assistant",
    "harness_config": { "mode": "local" },
    "trust_tier": "medium",
    "enabled_modes": ["chat"]
  }' | python3 -m json.tool 2>/dev/null || true

curl -s -X POST "${API_BASE_URL}/api/v1/desktop-templates" \
  -H "${AUTH_HEADER}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Allternit Desktop",
    "description": "Ubuntu 24.04 desktop for Linux agents",
    "os": "linux",
    "image": "allternit-desktop",
    "cpu_millis": 2000,
    "memory_mib": 4096,
    "disk_mib": 20480,
    "network_enabled": true,
    "env": {},
    "packages": ["allternit-factory"],
    "tags": ["e2e"],
    "public": true
  }' | python3 -m json.tool 2>/dev/null || true

curl -s -X POST "${API_BASE_URL}/api/v1/desktop-templates" \
  -H "${AUTH_HEADER}" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Allternit Desktop (Tart)",
    "description": "Ubuntu 24.04 desktop on Apple Silicon Tart with XFCE, VNC, and automation tools",
    "os": "macos",
    "image": "allternit-desktop-tart",
    "cpu_millis": 2000,
    "memory_mib": 4096,
    "disk_mib": 20480,
    "network_enabled": true,
    "env": {},
    "packages": [],
    "tags": ["tart", "e2e"],
    "public": true
  }' | python3 -m json.tool 2>/dev/null || true

echo "Seed complete."
