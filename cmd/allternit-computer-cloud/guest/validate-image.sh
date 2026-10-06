#!/bin/bash
# Validate a freshly built Allternit desktop image by launching a test
# container and checking that key services and binaries are present.

set -euo pipefail

IMAGE_NAME="${1:-allternit-desktop}"
TEST_CONTAINER="allternit-desktop-validate-$$"

cleanup() {
    incus delete -f "${TEST_CONTAINER}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[validate-image] launching test container from ${IMAGE_NAME}"
incus launch "${IMAGE_NAME}" "${TEST_CONTAINER}"

# Wait for boot.
for i in $(seq 1 120); do
    if incus exec "${TEST_CONTAINER}" -- systemctl is-system-running >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "[validate-image] checking binaries"
incus exec "${TEST_CONTAINER}" -- test -x /opt/allternit-factory/allternit-factory
incus exec "${TEST_CONTAINER}" -- test -x /opt/allternit-desktop/run.sh
incus exec "${TEST_CONTAINER}" -- which google-chrome
incus exec "${TEST_CONTAINER}" -- which tailscale
incus exec "${TEST_CONTAINER}" -- which x11vnc
incus exec "${TEST_CONTAINER}" -- which xfce4-session

echo "[validate-image] checking services"
incus exec "${TEST_CONTAINER}" -- systemctl is-enabled allternit-desktop.service
incus exec "${TEST_CONTAINER}" -- systemctl is-enabled allternit-factory-pane.service

echo "[validate-image] validation passed"
