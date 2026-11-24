#!/bin/bash
# Install the Factory pane engine and the allternit-node daemon on a cloud
# computer, so the node serves its terminals from the engine (#2814).
#
# Runs INSIDE the guest, as root. The one shared copy of this setup; every
# image calls it:
#   - build-image.sh (the Incus "allternit-desktop" base; the cloud computer
#     and headless images are built on top of it) runs it with --init systemd;
#   - build-headless-computer-image.sh runs it again (no binaries passed:
#     it keeps the base image's, or installs FACTORY_BIN / NODE_BIN);
#   - orgo/setup.sh runs it with --init supervisord (Orgo has no systemd).
# cloud-init.yaml writes the same units inline; keep the two in step.
#
# Who and where (the same on every image):
#   - Both services run as root with HOME=/root. The Allternit app runs as root
#     on every cloud image and writes the pairing identity the node adopts to
#     /root/.config/allternit/runtime-identity.json. The engine makes its
#     socket owner-only (0600), so the node must be the engine's user to
#     connect to it.
#   - The socket is the engine's default for HOME=/root and the Factory session
#     `ao` (/root/.config/ao/sessions/ao/herdr.sock), the path the app's own
#     allternit-api finds; where the engine binds is unchanged. Both units carry
#     ALLTERNIT_FACTORY_PANE_SOCKET with that path, so the node (and anything run
#     in an engine terminal) is handed it instead of guessing.
#     validate-image.sh checks that the engine's real socket matches it.
#   - The node starts only once the computer is paired: before that the
#     identity file does not exist and it would exit. Under systemd a path unit
#     starts it when the file appears; under supervisord a wrapper waits for it.
#
# usage: install-pane-engine.sh [--init systemd|supervisord] [--factory BIN]
#                               [--node BIN] [--supervisor-dir DIR] [--log-dir DIR]
#   --init            service manager (default: systemd when it is PID 1,
#                     else supervisord)
#   --factory BIN     allternit-factory binary to install (default: keep the
#                     one already at /opt/allternit-factory; one must exist)
#   --node BIN        allternit-node binary to install (default: keep the one
#                     already at /opt/allternit-node; without one the node
#                     units are still written but stay inert under systemd,
#                     and its supervisord program is left out)
#   --supervisor-dir  where the supervisord program file goes
#                     (default /etc/allternit/supervisor); it is written as
#                     allternit-engine.conf
#   --log-dir         supervisord log dir (default /var/log/allternit)
#
# Safe to run again: binaries are replaced only when they differ, units are
# rewritten with the same content, enabling is idempotent.

set -euo pipefail

FACTORY_PATH=/opt/allternit-factory/allternit-factory
NODE_PATH=/opt/allternit-node/allternit-node
SERVICE_USER=root
SERVICE_HOME=/root
PANE_SOCKET="${SERVICE_HOME}/.config/ao/sessions/ao/herdr.sock"
NODE_IDENTITY="${SERVICE_HOME}/.config/allternit/runtime-identity.json"

INIT=""
FACTORY_SRC=""
NODE_SRC=""
SUPERVISOR_DIR=/etc/allternit/supervisor
LOG_DIR=/var/log/allternit

usage() {
    sed -n '/^# usage:/,/^#   --log-dir/p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --init) INIT="${2:?--init needs a value}"; shift 2 ;;
        --factory) FACTORY_SRC="${2:?--factory needs a path}"; shift 2 ;;
        --node) NODE_SRC="${2:?--node needs a path}"; shift 2 ;;
        --supervisor-dir) SUPERVISOR_DIR="${2:?--supervisor-dir needs a path}"; shift 2 ;;
        --log-dir) LOG_DIR="${2:?--log-dir needs a path}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install-pane-engine: unknown argument $1" >&2; usage >&2; exit 2 ;;
    esac
done

log() { echo "[install-pane-engine] $*"; }

[ "$(id -u)" = 0 ] || { echo "install-pane-engine: run as root" >&2; exit 1; }

if [ -z "${INIT}" ]; then
    if [ -d /run/systemd/system ] || [ "$(cat /proc/1/comm 2>/dev/null || true)" = systemd ]; then
        INIT=systemd
    else
        INIT=supervisord
    fi
fi
case "${INIT}" in
    systemd|supervisord) ;;
    *) echo "install-pane-engine: --init must be systemd or supervisord" >&2; exit 2 ;;
esac

# ---------------------------------------------------------------------------
# 1. Binaries.
# ---------------------------------------------------------------------------
install_bin() { # <source> <dest>
    local src="$1" dest="$2"
    [ -f "${src}" ] || { echo "install-pane-engine: ${src} not found" >&2; exit 1; }
    if [ -x "${dest}" ] && cmp -s "${src}" "${dest}"; then
        log "${dest} is current"
        return
    fi
    mkdir -p "$(dirname "${dest}")"
    install -m 0755 "${src}" "${dest}.new"
    mv -f "${dest}.new" "${dest}"
    log "installed ${dest}"
}

if [ -n "${FACTORY_SRC}" ]; then
    install_bin "${FACTORY_SRC}" "${FACTORY_PATH}"
fi
[ -x "${FACTORY_PATH}" ] || { echo "install-pane-engine: no allternit-factory at ${FACTORY_PATH} (pass --factory)" >&2; exit 1; }
if [ -n "${NODE_SRC}" ]; then
    install_bin "${NODE_SRC}" "${NODE_PATH}"
fi
HAVE_NODE=0
[ -x "${NODE_PATH}" ] && HAVE_NODE=1
[ "${HAVE_NODE}" = 1 ] || log "no allternit-node binary; its units stay inert"

# ---------------------------------------------------------------------------
# 2a. systemd: allternit-factory-pane.service, allternit-node.service and
#     allternit-node.path (starts the node once the identity file appears).
# ---------------------------------------------------------------------------
write_if_changed() { # <dest>  (content on stdin)
    local dest="$1" tmp
    tmp="$(mktemp)"
    cat > "${tmp}"
    if [ -f "${dest}" ] && cmp -s "${tmp}" "${dest}"; then
        rm -f "${tmp}"
        return
    fi
    mkdir -p "$(dirname "${dest}")"
    install -m 0644 "${tmp}" "${dest}"
    rm -f "${tmp}"
    log "wrote ${dest}"
}

if [ "${INIT}" = systemd ]; then
    write_if_changed /etc/systemd/system/allternit-factory-pane.service <<EOF
[Unit]
Description=Allternit Factory pane engine (guest terminals)
After=network.target systemd-user-sessions.service
ConditionPathExists=${FACTORY_PATH}

[Service]
Type=simple
User=${SERVICE_USER}
Environment="HOME=${SERVICE_HOME}"
# The Factory's agent session: the one \`allternit-factory pane tty\` reaches.
Environment="HERDR_SESSION=ao"
# Where the engine binds for HOME and HERDR_SESSION above; allternit-node
# gets the same path.
Environment="ALLTERNIT_FACTORY_PANE_SOCKET=${PANE_SOCKET}"
ExecStart=${FACTORY_PATH} pane server
ExecStop=/bin/kill -TERM \$MAINPID
KillMode=mixed
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

    write_if_changed /etc/systemd/system/allternit-node.service <<EOF
[Unit]
Description=Allternit Node Daemon (always-on node.core presence; terminals on the pane engine)
After=network-online.target allternit-factory-pane.service
Wants=network-online.target
ConditionPathExists=${NODE_PATH}
# Paired: the app wrote the identity. allternit-node.path starts it then.
ConditionPathExists=${NODE_IDENTITY}
StartLimitIntervalSec=0

[Service]
Type=simple
User=${SERVICE_USER}
Environment="HOME=${SERVICE_HOME}"
Environment="ALLTERNIT_RUNTIME_IDENTITY_PATH=${NODE_IDENTITY}"
Environment="ALLTERNIT_FACTORY_PANE_SOCKET=${PANE_SOCKET}"
ExecStart=${NODE_PATH} run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

    write_if_changed /etc/systemd/system/allternit-node.path <<EOF
[Unit]
Description=Start allternit-node once this computer is paired

[Path]
PathExists=${NODE_IDENTITY}
Unit=allternit-node.service

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable allternit-factory-pane.service allternit-node.service allternit-node.path
    log "systemd units enabled (socket ${PANE_SOCKET}, user ${SERVICE_USER})"
    exit 0
fi

# ---------------------------------------------------------------------------
# 2b. supervisord: one program file with the engine and (when installed) the
#     node. The caller includes ${SUPERVISOR_DIR}/allternit-engine.conf.
#     No user= line, like the app's own program: both run as supervisord's
#     user, which is root on Orgo (the same user as the app).
# ---------------------------------------------------------------------------
mkdir -p "${LOG_DIR}"
write_if_changed /usr/local/sbin/allternit-node-run <<EOF
#!/bin/sh
# Waits until this computer is paired (the app writes the identity), then runs
# allternit-node. supervisord keeps it running.
while [ ! -s "${NODE_IDENTITY}" ]; do sleep 5; done
exec ${NODE_PATH} run
EOF
chmod 0755 /usr/local/sbin/allternit-node-run

# XDG_CONFIG_HOME is pinned here because supervisord passes on whatever
# environment it was started with; the engine's socket lives under it.
ENV_COMMON="HOME=\"${SERVICE_HOME}\",XDG_CONFIG_HOME=\"${SERVICE_HOME}/.config\",ALLTERNIT_FACTORY_PANE_SOCKET=\"${PANE_SOCKET}\""
{
    cat <<EOF
; Written by install-pane-engine.sh. The Factory pane engine and allternit-node.
[program:allternit-factory-pane]
command=${FACTORY_PATH} pane server
environment=${ENV_COMMON},HERDR_SESSION="ao"
autostart=true
autorestart=true
startsecs=3
startretries=100
stopasgroup=true
killasgroup=true
stdout_logfile=${LOG_DIR}/allternit-factory-pane.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=2
redirect_stderr=true
EOF
    if [ "${HAVE_NODE}" = 1 ]; then
        cat <<EOF

[program:allternit-node]
command=/usr/local/sbin/allternit-node-run
environment=${ENV_COMMON},ALLTERNIT_RUNTIME_IDENTITY_PATH="${NODE_IDENTITY}"
autostart=true
autorestart=true
startsecs=3
startretries=100
stopasgroup=true
killasgroup=true
stdout_logfile=${LOG_DIR}/allternit-node.log
stdout_logfile_maxbytes=10MB
stdout_logfile_backups=2
redirect_stderr=true
EOF
    fi
} | write_if_changed "${SUPERVISOR_DIR}/allternit-engine.conf"
log "supervisord programs in ${SUPERVISOR_DIR}/allternit-engine.conf (socket ${PANE_SOCKET})"
