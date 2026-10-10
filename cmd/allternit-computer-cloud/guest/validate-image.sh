#!/bin/bash
# Validate a freshly built Allternit desktop image by launching a test
# container and checking that key services and binaries are present, and that
# the Allternit Driver answers hello/read_ui through the guest-agent forwarder
# (phase D1b). Do NOT run this against the production fleet; it launches a
# local throw-away container from a locally built image.

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
incus exec "${TEST_CONTAINER}" -- test -x /usr/local/bin/allternit-driver-rpc
incus exec "${TEST_CONTAINER}" -- test -f /opt/allternit-driver/allternit_driver/__main__.py
incus exec "${TEST_CONTAINER}" -- which google-chrome
incus exec "${TEST_CONTAINER}" -- which tailscale
incus exec "${TEST_CONTAINER}" -- which x11vnc
incus exec "${TEST_CONTAINER}" -- which xfce4-session
incus exec "${TEST_CONTAINER}" -- which xdotool
incus exec "${TEST_CONTAINER}" -- which scrot
incus exec "${TEST_CONTAINER}" -- which python3

echo "[validate-image] checking services"
incus exec "${TEST_CONTAINER}" -- systemctl is-enabled allternit-desktop.service
incus exec "${TEST_CONTAINER}" -- systemctl is-enabled allternit-factory-pane.service
incus exec "${TEST_CONTAINER}" -- systemctl is-enabled allternit-driver.service
# The driver must actually be running (it starts at boot; the desktop session
# may still be coming up, so give it a moment).
driver_up=""
for i in $(seq 1 60); do
    if incus exec "${TEST_CONTAINER}" -- systemctl is-active --quiet allternit-driver.service 2>/dev/null; then
        driver_up=1
        break
    fi
    sleep 2
done
if [ -z "${driver_up}" ]; then
    incus exec "${TEST_CONTAINER}" -- journalctl -u allternit-driver --no-pager -n 40 >&2 || true
    echo "ERROR: allternit-driver.service is not active" >&2
    exit 1
fi

echo "[validate-image] checking the driver's capability report"
# hello through the same forwarder path allternit-api uses.
incus exec "${TEST_CONTAINER}" -- python3 - <<'PY'
import base64, json, subprocess, sys

req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {}})
out = subprocess.run(
    ["/usr/local/bin/allternit-driver-rpc", base64.b64encode(req.encode()).decode()],
    capture_output=True, text=True, timeout=20)
if out.returncode != 0:
    sys.exit(f"driver forwarder failed: {out.stderr.strip()[:300]}")
reply = json.loads(out.stdout.strip().splitlines()[-1])
if reply.get("error"):
    sys.exit(f"driver refused hello: {reply['error']}")
caps = reply["result"]["hello"]
for member in ("read_ui", "act", "run_batch", "verify"):
    if not caps.get(member):
        sys.exit(f"driver capability {member} is not set: {json.dumps(caps)[:300]}")
if not caps.get("pixel"):
    sys.exit("driver reports no pixel tools")
print(f"[validate-image] driver hello ok: engines={list(reply['result']['engines'].keys())}")
PY

echo "[validate-image] checking read_ui on a real window"
# Chrome exposes an AT-SPI tree once it detects an assistive client; the
# driver is one. xdotool keeps this honest about the screen actually working.
incus exec "${TEST_CONTAINER}" -- sh -c '
    export DISPLAY=:0
    google-chrome --new-window --force-renderer-accessibility --no-first-run --no-default-browser-check about:blank >/tmp/allternit-validate-chrome.log 2>&1 &
    for i in $(seq 1 30); do
        if xdotool search --name "Chrome" 2>/dev/null | head -1 | grep -q .; then
            break
        fi
        sleep 1
    done
'
incus exec "${TEST_CONTAINER}" -- python3 - <<'PY'
import base64, json, subprocess, sys

def rpc(method, params):
    req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    out = subprocess.run(
        ["/usr/local/bin/allternit-driver-rpc", base64.b64encode(req.encode()).decode()],
        capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        sys.exit(f"driver forwarder failed: {out.stderr.strip()[:300]}")
    return json.loads(out.stdout.strip().splitlines()[-1])

last = None
for _ in range(30):
    reply = rpc("read_ui", {"app": "google chrome", "max_elements": 60, "vision": "off"})
    if reply.get("error"):
        last = reply["error"]
        import time; time.sleep(2)
        continue
    result = reply["result"]
    elements = result.get("elements") or []
    if len(elements) >= 2:
        print(f"[validate-image] read_ui ok: {len(elements)} elements from engine={result.get('engine')}")
        break
    last = result
    import time; time.sleep(2)
else:
    sys.exit(f"read_ui never returned a populated tree: {json.dumps(last)[:400]}")
PY

echo "[validate-image] validation passed"
