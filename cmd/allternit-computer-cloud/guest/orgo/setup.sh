#!/bin/bash
# First boot of an Allternit cloud computer on Orgo (orgo.ai).
#
# Orgo computers boot Orgo's own Linux desktop image. A custom template needs
# Orgo's top plan, so instead of an image cloud-api runs this script once over
# Orgo's bash API (embedded in cloud-api: services/provision_orgo.rs). It makes
# the computer behave like our `allternit-cloud-computer` Incus image:
#
#   - installs the Allternit Desktop linux-x64 .deb (the same package
#     build-cloud-computer-image.sh installs);
#   - runs the app the way that image's desktop autostart does
#     (ALLTERNIT_PROVISIONED=1 /usr/bin/allternit --no-sandbox, as root), on the
#     X display Orgo's desktop already runs;
#   - keeps it running with supervisord. Orgo computers have no systemd (PID 1
#     is Orgo's init.sh), so there is no allternit-desktop.service. Orgo's image
#     already runs supervisord (/etc/supervisor/supervisord.conf, programs in
#     conf.d/, checked live 2026-11-13): the program is added there. On an
#     image without one, we start our own supervisord instance instead;
#   - works around Desktop .debs whose bundled sidecars need a newer glibc
#     than Ubuntu 24.04 has (1.1.14 was built on 26.04): those binaries are
#     replaced from the signed runtime feed. A no-op once a 24.04-built .deb
#     ships.
#
# cloud-api writes /etc/allternit/bootstrap.json BEFORE it starts this script,
# so a run that finishes after cloud-api restarted still pairs the computer.
# After every later start of the computer cloud-api runs
# /usr/local/sbin/allternit-orgo-ensure, which brings supervisord and the app
# back (a stopped Orgo computer loses its processes, not its disk).
#
# Safe to run again: each step checks what is already there.
#
# Environment:
#   ALLTERNIT_ORGO_DESKTOP_DEB_URL     - https URL of the .deb (required)
#   ALLTERNIT_ORGO_DESKTOP_DEB_SHA256  - expected sha256 of the .deb (optional,
#                                        checked when set)
#   ALLTERNIT_ORGO_DISPLAY             - force the X display (e.g. ":99"); by
#                                        default it is detected at each app start
#   ALLTERNIT_ORGO_RUNTIME_FEED        - runtime feed for the glibc workaround
#                                        (default
#                                        https://runtime.allternit.com/stable/linux-x64)
#
# Result: /var/lib/allternit-orgo/state is "running", "done" or
# "failed: <reason>". cloud-api polls it; the log is
# /var/log/allternit-orgo/setup.log (cloud-api redirects output there).

set -euo pipefail

DEB_URL="${ALLTERNIT_ORGO_DESKTOP_DEB_URL:-}"
DEB_SHA="${ALLTERNIT_ORGO_DESKTOP_DEB_SHA256:-}"
RUNTIME_FEED="${ALLTERNIT_ORGO_RUNTIME_FEED:-https://runtime.allternit.com/stable/linux-x64}"
STATE_DIR=/var/lib/allternit-orgo
LOG_DIR=/var/log/allternit-orgo
SUPERVISOR_DIR=/etc/allternit/orgo

# Root is needed (apt, /etc). Orgo runs commands as root today; fall back to
# passwordless sudo if that ever changes.
if [ "$(id -u)" != 0 ]; then
    if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        exec sudo -n env \
            ALLTERNIT_ORGO_DESKTOP_DEB_URL="${DEB_URL}" \
            ALLTERNIT_ORGO_DESKTOP_DEB_SHA256="${DEB_SHA}" \
            ALLTERNIT_ORGO_DISPLAY="${ALLTERNIT_ORGO_DISPLAY:-}" \
            ALLTERNIT_ORGO_RUNTIME_FEED="${RUNTIME_FEED}" \
            bash "$0" "$@"
    fi
    echo "allternit setup: needs root or passwordless sudo" >&2
    exit 1
fi

mkdir -p "${STATE_DIR}" "${LOG_DIR}"

# One run at a time. A second run while the first is working leaves the
# state file to the first.
exec 9>"${STATE_DIR}/setup.lock"
if command -v flock >/dev/null 2>&1; then
    flock -n 9 || { echo "allternit setup: another run is in progress"; exit 0; }
fi

fail() {
    echo "failed: $*" > "${STATE_DIR}/state"
    echo "allternit setup failed: $*" >&2
    exit 1
}
trap 'fail "line ${LINENO}: ${BASH_COMMAND}"' ERR

echo running > "${STATE_DIR}/state"
echo "allternit setup: started $(date -u +%Y-%m-%dT%H:%M:%SZ)"

[ -n "${DEB_URL}" ] || fail "ALLTERNIT_ORGO_DESKTOP_DEB_URL is not set"
command -v apt-get >/dev/null 2>&1 || fail "apt-get not found (a Debian or Ubuntu image is expected)"

export DEBIAN_FRONTEND=noninteractive
APT=(apt-get -o DPkg::Lock::Timeout=300 -y -q)
APT_UPDATED=0
apt_update_once() {
    if [ "${APT_UPDATED}" = 0 ]; then
        "${APT[@]}" update || fail "apt-get update failed"
        APT_UPDATED=1
    fi
}

# ---------------------------------------------------------------------------
# 1. Tools: curl, supervisor, sha256sum, ps, python3.
# ---------------------------------------------------------------------------
need=()
command -v curl >/dev/null 2>&1 || need+=(curl ca-certificates)
command -v supervisord >/dev/null 2>&1 || need+=(supervisor)
command -v sha256sum >/dev/null 2>&1 || need+=(coreutils)
command -v ps >/dev/null 2>&1 || need+=(procps)
command -v python3 >/dev/null 2>&1 || need+=(python3)
if [ "${#need[@]}" -gt 0 ]; then
    echo "allternit setup: installing ${need[*]}"
    # Keep the supervisor package from starting its default daemon (we start
    # what we need ourselves). Only when no policy is in place already.
    own_policy=0
    if [ ! -e /usr/sbin/policy-rc.d ]; then
        printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d
        chmod 0755 /usr/sbin/policy-rc.d
        own_policy=1
    fi
    apt_update_once
    "${APT[@]}" install --no-install-recommends "${need[@]}" || fail "could not install ${need[*]}"
    if [ "${own_policy}" = 1 ]; then
        rm -f /usr/sbin/policy-rc.d
    fi
fi
command -v supervisord >/dev/null 2>&1 || fail "supervisord missing after install"

# ---------------------------------------------------------------------------
# 2. The Allternit Desktop app.
# ---------------------------------------------------------------------------
want="${DEB_URL} ${DEB_SHA}"
if [ -x /usr/bin/allternit ] && [ "$(cat "${STATE_DIR}/deb.installed" 2>/dev/null || true)" = "${want}" ]; then
    echo "allternit setup: app already installed"
else
    deb=/var/tmp/allternit-desktop.deb
    echo "allternit setup: downloading the app"
    curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 30 -o "${deb}" "${DEB_URL}" \
        || fail "download of the Allternit Desktop .deb failed"
    if [ -n "${DEB_SHA}" ]; then
        got="$(sha256sum "${deb}" | awk '{print $1}')"
        expected="$(printf '%s' "${DEB_SHA}" | tr 'A-F' 'a-f')"
        if [ "${got}" != "${expected}" ]; then
            rm -f "${deb}"
            fail "sha256 mismatch for the .deb: got ${got}"
        fi
    fi
    apt_update_once
    "${APT[@]}" install "${deb}" || fail "apt could not install the Allternit Desktop .deb"
    rm -f "${deb}"
    [ -x /usr/bin/allternit ] || fail "/usr/bin/allternit missing after install"
    echo "${want}" > "${STATE_DIR}/deb.installed"
fi

# ---------------------------------------------------------------------------
# 2b. Sidecars built against a newer glibc than this host has: replace them
#     from the runtime feed (latest.json -> manifest -> objects/<sha256>.gz,
#     every hash checked). Skipped when the bundled ones load.
# ---------------------------------------------------------------------------
APP_DIR="$(dirname "$(readlink -f /usr/bin/allternit)")"
BIN_DIR="${APP_DIR}/resources/bin"
needs_newer_glibc() {
    local f out
    for f in "${BIN_DIR}/allternit-api" "${BIN_DIR}/allternit-tts" "${BIN_DIR}/allternit-voice-service"; do
        [ -x "${f}" ] || continue
        out="$(ldd "${f}" 2>&1 || true)"
        case "${out}" in
            *GLIBC_*"not found"*) return 0 ;;
        esac
    done
    return 1
}
if needs_newer_glibc; then
    echo "allternit setup: bundled sidecars need a newer glibc; replacing them from ${RUNTIME_FEED}"
    python3 -I - "${BIN_DIR}" "${RUNTIME_FEED}" <<'PY' || fail "could not replace the sidecars from the runtime feed"
import gzip, hashlib, json, os, sys, urllib.request

bin_dir, feed = sys.argv[1], sys.argv[2].rstrip("/")
base = feed.rsplit("/stable/", 1)[0]


def get(url):
    with urllib.request.urlopen(url, timeout=300) as response:
        return response.read()


latest = json.loads(get(feed + "/latest.json"))
manifest_raw = get(feed + "/" + latest["manifest"])
want = latest.get("manifestSha256")
if want and hashlib.sha256(manifest_raw).hexdigest() != want:
    sys.exit("runtime manifest sha256 mismatch")
files = json.loads(manifest_raw)["files"]
replaced = 0
for name in ("allternit-api", "allternit-tts", "allternit-voice-service"):
    target = os.path.join(bin_dir, name)
    entry = files.get("bin/" + name)
    if entry is None or not os.path.exists(target):
        continue
    sha = entry["sha256"]
    data = gzip.decompress(get(f"{base}/objects/{sha}.gz"))
    if hashlib.sha256(data).hexdigest() != sha:
        sys.exit(f"{name}: sha256 mismatch")
    tmp = target + ".allternit-new"
    with open(tmp, "wb") as out:
        out.write(data)
    os.chmod(tmp, 0o755)
    os.replace(tmp, target)
    print(f"replaced {name} with runtime {latest.get('version')}")
    replaced += 1
if replaced == 0:
    sys.exit("the runtime feed has none of the bundled sidecars")
PY
    if needs_newer_glibc; then
        fail "sidecars still need a newer glibc after the runtime feed swap"
    fi
fi

# The app links gizzi into ~/.local/bin; put that on root's PATH (same as the
# cloud computer image).
printf '%s\n' 'case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) PATH="$HOME/.local/bin:$PATH" ;; esac' \
    > /etc/profile.d/allternit-local-bin.sh

# Provisioned mode (same file the cloud computer image writes). The
# subscription gateway is not part of the Orgo setup.
mkdir -p /etc/allternit
chmod 0700 /etc/allternit
cat > /etc/allternit/provisioned.env <<'EOF'
ALLTERNIT_PROVISIONED=1
ALLTERNIT_LOCAL_SUBS_GATEWAY=0
EOF

# ---------------------------------------------------------------------------
# 3. Helpers: display detection, the app launcher, ensure.
# ---------------------------------------------------------------------------
cat > /usr/local/sbin/allternit-orgo-display <<'EOF'
#!/bin/bash
# Prints the X display of Orgo's desktop as shell exports (DISPLAY, and
# XAUTHORITY when the X server uses an auth file). Exit 1 when none is found.
if [ -n "${ALLTERNIT_ORGO_DISPLAY:-}" ]; then
    echo "export DISPLAY=${ALLTERNIT_ORGO_DISPLAY}"
    exit 0
fi
display=""
auth=""
# 1. A running X server: its arguments carry :N and maybe -auth FILE.
line="$(ps -eo args= 2>/dev/null | grep -E '^(/[^ ]*/)?(Xvfb|Xvnc|Xtigervnc|Xtightvnc|Xorg|Xwayland|X)( |$)' | head -n 1 || true)"
if [ -n "${line}" ]; then
    display="$(printf '%s\n' "${line}" | grep -oE '(^| ):[0-9]+' | head -n 1 | tr -d ' ' || true)"
    auth="$(printf '%s\n' "${line}" | sed -nE 's/.* -auth ([^ ]+).*/\1/p')"
fi
# 2. Otherwise the lowest-numbered X socket.
if [ -z "${display}" ]; then
    n="$(ls /tmp/.X11-unix/ 2>/dev/null | grep -E '^X[0-9]+$' | sed 's/^X//' | sort -n | head -n 1 || true)"
    [ -n "${n}" ] && display=":${n}"
fi
[ -n "${display}" ] || exit 1
echo "export DISPLAY=${display}"
if [ -n "${auth}" ] && [ -r "${auth}" ]; then
    echo "export XAUTHORITY=${auth}"
fi
exit 0
EOF
chmod 0755 /usr/local/sbin/allternit-orgo-display

cat > /usr/local/sbin/allternit-orgo-run <<'EOF'
#!/bin/bash
# The allternit-desktop supervisord program. Mirrors the cloud computer
# image's autostart entry: provisioned mode, root, --no-sandbox.
set -a
[ -f /etc/allternit/provisioned.env ] && . /etc/allternit/provisioned.env
set +a
export HOME=/root
export ALLTERNIT_PROVISIONED=1
export ELECTRON_ENABLE_LOGGING=1
export GTK_MODULES=atk-bridge
export ACCESSIBILITY_ENABLED=1
exports="$(/usr/local/sbin/allternit-orgo-display)" || { echo "allternit: no X display found" >&2; exit 1; }
eval "${exports}"
exec /usr/bin/allternit --no-sandbox
EOF
chmod 0755 /usr/local/sbin/allternit-orgo-run

mkdir -p "${SUPERVISOR_DIR}"
# The program, shared by both supervisord modes.
cat > "${SUPERVISOR_DIR}/allternit-desktop.conf" <<EOF
[program:allternit-desktop]
command=/usr/local/sbin/allternit-orgo-run
autostart=true
autorestart=true
startsecs=10
startretries=100
stopasgroup=true
killasgroup=true
stdout_logfile=${LOG_DIR}/allternit-desktop.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=2
redirect_stderr=true
EOF
# Our own supervisord, used only when the image runs none.
cat > "${SUPERVISOR_DIR}/supervisord.conf" <<EOF
; Allternit's own supervisord on an Orgo computer without one.
[unix_http_server]
file=/run/allternit-supervisor.sock
chmod=0700

[supervisord]
logfile=${LOG_DIR}/supervisord.log
pidfile=/run/allternit-supervisord.pid
childlogdir=${LOG_DIR}

[rpcinterface:supervisor]
supervisor.rpcinterface_factory = supervisor.rpcinterface:make_main_rpcinterface

[supervisorctl]
serverurl=unix:///run/allternit-supervisor.sock

[include]
files = ${SUPERVISOR_DIR}/allternit-desktop.conf
EOF

cat > /usr/local/sbin/allternit-orgo-ensure <<'EOF'
#!/bin/bash
# Start Allternit on this Orgo computer (idempotent). cloud-api runs it after
# first boot and after every start of the computer. Exit 0 = app RUNNING.
#
# Uses the image's supervisord (/etc/supervisor/supervisord.conf) when it runs:
# the program goes in its conf.d and comes back on its own after a restart.
# Otherwise starts our own supervisord with the same program.
set -u
PROGRAM=/etc/allternit/orgo/allternit-desktop.conf
SYS_CONF=/etc/supervisor/supervisord.conf
OWN_CONF=/etc/allternit/orgo/supervisord.conf
OWN_SOCK=/run/allternit-supervisor.sock
mkdir -p /var/log/allternit-orgo
supervisord_up() {
    supervisorctl -c "$1" pid 2>/dev/null | grep -qE '^[0-9]+$'
}
if [ -f "${SYS_CONF}" ] && [ -d /etc/supervisor/conf.d ] && supervisord_up "${SYS_CONF}"; then
    CONF="${SYS_CONF}"
    # Never two copies of the app: stop our own instance if an earlier run used it.
    if [ -S "${OWN_SOCK}" ] && supervisord_up "${OWN_CONF}"; then
        supervisorctl -c "${OWN_CONF}" shutdown >/dev/null 2>&1 || true
    fi
    if ! cmp -s "${PROGRAM}" /etc/supervisor/conf.d/allternit-desktop.conf; then
        cp "${PROGRAM}" /etc/supervisor/conf.d/allternit-desktop.conf
    fi
    supervisorctl -c "${CONF}" reread >/dev/null 2>&1 || true
    supervisorctl -c "${CONF}" update >/dev/null 2>&1 || true
else
    CONF="${OWN_CONF}"
    if ! { [ -S "${OWN_SOCK}" ] && supervisord_up "${CONF}"; }; then
        rm -f "${OWN_SOCK}" /run/allternit-supervisord.pid
        supervisord -c "${CONF}" || { echo "allternit: supervisord did not start" >&2; exit 1; }
        for _ in $(seq 1 10); do supervisord_up "${CONF}" && break; sleep 1; done
    fi
fi
for _ in $(seq 1 30); do
    state="$(supervisorctl -c "${CONF}" status allternit-desktop 2>/dev/null | awk '{print $2}')"
    case "${state}" in
        RUNNING) echo "allternit-desktop RUNNING (${CONF})"; exit 0 ;;
        STARTING|BACKOFF) sleep 1 ;;
        *) supervisorctl -c "${CONF}" start allternit-desktop >/dev/null 2>&1 || true; sleep 1 ;;
    esac
done
supervisorctl -c "${CONF}" status allternit-desktop >&2 || true
tail -n 20 /var/log/allternit-orgo/allternit-desktop.log >&2 2>/dev/null || true
exit 1
EOF
chmod 0755 /usr/local/sbin/allternit-orgo-ensure

# ---------------------------------------------------------------------------
# 4. Start it.
# ---------------------------------------------------------------------------
/usr/local/sbin/allternit-orgo-display >/dev/null || fail "no X display found (is Orgo's desktop running?)"
/usr/local/sbin/allternit-orgo-ensure || fail "the app did not reach RUNNING under supervisord"

trap - ERR
echo done > "${STATE_DIR}/state"
echo "allternit setup: done $(date -u +%Y-%m-%dT%H:%M:%SZ)"
