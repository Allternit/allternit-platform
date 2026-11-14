#!/usr/bin/env bash
# Build the Brain runner Incus image (plan step 1.3, PLAN-brain-runner-2026-11-14.md).
#
# A runner is a tiny container that holds one person's vendor CLI sign-ins
# (claude, codex, grok, kimi) and runs chat turns with tools off. It is NOT a
# computer: no desktop, no SSH, no systemd as init. The runner program is the
# first process, started by lxc.init.cmd (set on the instance at create time by
# cloud-api, D4), through the small wrapper /usr/local/bin/allternit-runner-init.
#
# Run on an Incus host that may build images (the standby Linux builder, never
# the production mail host), as a user that can run `incus`:
#   bash build-brain-runner-image.sh --runner-bin ./allternit-brain-runner
#
# Output (in --out-dir, default .): allternit-brain-runner-<tag>.tar.gz (unified
# Incus image tarball) and a .sha256, plus the `incus image import` line to run
# on the serving host. Target size <= 1.5 GB; the size is printed.
set -euo pipefail

BASE_IMAGE=${BASE_IMAGE:-images:ubuntu/24.04}  # same glibc (2.39) as the runner binary built in alt-linux-build
IMAGE_ALIAS=${IMAGE_ALIAS:-allternit-brain-runner}
NODE_VERSION=${NODE_VERSION:-22.23.5}   # must be >= 22.19
NODE_SHA256=${NODE_SHA256:-}            # optional; else read from nodejs.org SHASUMS256.txt
CLAUDE_VERSION=${CLAUDE_VERSION:-2.1.325}
CODEX_VERSION=${CODEX_VERSION:-0.169.0}
GROK_VERSION=${GROK_VERSION:-1.0.77}
KIMI_VERSION=${KIMI_VERSION:-2.5.2}
SIZE_LIMIT_MB=1536

RUNNER_BIN=""
OUT_DIR="."
KEEP_BUILDER=${KEEP_BUILDER:-}
BUILDER="allternit-brain-runner-builder-$$"

usage() {
  cat <<USAGE
Usage: $0 --runner-bin <path> [--out-dir <dir>] [--alias <name>]

Builds the Brain runner image (Ubuntu 24.04 container) with Node ${NODE_VERSION} and the
pinned CLIs: claude ${CLAUDE_VERSION}, codex ${CODEX_VERSION}, grok ${GROK_VERSION}, kimi ${KIMI_VERSION}.

  --runner-bin <path>  allternit-brain-runner built for linux-x64 (step 1.2). Required.
  --out-dir <dir>      where the tarball goes (default: current directory)
  --alias <name>       image alias used in the printed import line (default: ${IMAGE_ALIAS})
  -h, --help           this text

Environment overrides: BASE_IMAGE NODE_VERSION NODE_SHA256 CLAUDE_VERSION CODEX_VERSION
GROK_VERSION KIMI_VERSION KEEP_BUILDER (set to keep the build container).
Needs: incus, curl, sha256sum (or shasum). Does not touch any other instance.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --runner-bin) RUNNER_BIN=${2:?--runner-bin needs a path}; shift 2 ;;
    --out-dir) OUT_DIR=${2:?--out-dir needs a path}; shift 2 ;;
    --alias) IMAGE_ALIAS=${2:?--alias needs a name}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { echo "[build-brain-runner-image] $*"; }
die() { echo "ERROR: $*" >&2; exit 1; }
cleanup() {
  if [ -z "$KEEP_BUILDER" ]; then
    incus delete -f "$BUILDER" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

[ -n "$RUNNER_BIN" ] || { usage >&2; die "--runner-bin is required"; }
[ -f "$RUNNER_BIN" ] || die "$RUNNER_BIN not found"
[ -x "$RUNNER_BIN" ] || die "$RUNNER_BIN is not executable"
if command -v file >/dev/null 2>&1; then
  file -b "$RUNNER_BIN" | grep -q 'ELF 64-bit.*x86-64' || die "$RUNNER_BIN is not a linux-x64 ELF binary"
fi
command -v incus >/dev/null 2>&1 || die "incus not found (run this on an Incus host or builder)"
mkdir -p "$OUT_DIR"

# Node: pinned tarball from nodejs.org, checked against SHASUMS256.txt.
NODE_TAR="node-v${NODE_VERSION}-linux-x64.tar.xz"
NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/${NODE_TAR}"
if [ -z "$NODE_SHA256" ]; then
  NODE_SHA256=$(curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" | awk -v f="$NODE_TAR" '$2==f{print $1}')
fi
[[ "$NODE_SHA256" =~ ^[a-f0-9]{64}$ ]] || die "could not get a sha256 for $NODE_TAR"

log "launching ${BASE_IMAGE} as ${BUILDER}"
incus launch --quiet "$BASE_IMAGE" "$BUILDER" -c limits.cpu=2 -c limits.memory=2GiB
for _ in $(seq 1 60); do
  incus exec "$BUILDER" -- true >/dev/null 2>&1 && break
  sleep 1
done
# The builder has networking through its default bridge for downloads.
for _ in $(seq 1 60); do
  incus exec "$BUILDER" -- sh -c 'getent hosts nodejs.org >/dev/null' 2>/dev/null && break
  sleep 1
done

x() { incus exec "$BUILDER" -- "$@"; }

log "base packages"
x sh -c 'export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends ca-certificates curl xz-utils git util-linux procps tini >/dev/null'

log "Node ${NODE_VERSION}"
x sh -c "set -e
  curl -fsSL '${NODE_URL}' -o /tmp/node.tar.xz
  echo '${NODE_SHA256}  /tmp/node.tar.xz' | sha256sum -c -
  tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --exclude='*/CHANGELOG.md' --exclude='*/README.md' --exclude='*/share/doc' --exclude='*/share/man'
  rm -f /tmp/node.tar.xz
  node --version"

log "CLIs: claude ${CLAUDE_VERSION}, codex ${CODEX_VERSION}, grok ${GROK_VERSION}"
# npm packages ship the native binary through an optional per-platform package.
x sh -c "set -e
  npm install -g --no-audit --no-fund --no-update-notifier \
    @anthropic-ai/claude-code@${CLAUDE_VERSION} \
    @openai/codex@${CODEX_VERSION} \
    @xai-official/grok@${GROK_VERSION}
  npm cache clean --force >/dev/null 2>&1 || true"

log "CLI: kimi ${KIMI_VERSION} (official installer, checksum-verified by it)"
x sh -c "set -e
  curl -fsSL https://code.kimi.com/kimi-code/install.sh -o /tmp/kimi-install.sh
  KIMI_NO_MODIFY_PATH=1 KIMI_INSTALL_DIR=/usr/local KIMI_VERSION='${KIMI_VERSION}' bash /tmp/kimi-install.sh
  rm -f /tmp/kimi-install.sh"

log "runner user, binary, env, init wrapper"
x sh -c 'set -e
  useradd --create-home --home-dir /home/runner --shell /usr/sbin/nologin --uid 1000 runner
  mkdir -p /etc/allternit'
incus file push --mode 0755 "$RUNNER_BIN" "$BUILDER/usr/local/bin/allternit-brain-runner"

# /etc/environment: no vendor self-update, no telemetry, no browser (the
# CLIs print the login URL instead). Per-runner values (ALLTERNIT_RUNNER_*) are
# set on the instance by cloud-api, not baked here. The runner points each
# vendor at its own isolated home under ALLTERNIT_RUNNER_HOME (default
# /home/runner): HOME / CODEX_HOME / KIMI_CODE_HOME. Codex and grok take their
# update-off switches as flags/config the runner passes (see FINDINGS 0.2).
incus file push - "$BUILDER/etc/environment" --mode 0644 <<'ENVFILE'
DISABLE_AUTOUPDATER=1
DISABLE_UPDATES=1
DISABLE_TELEMETRY=1
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
KIMI_CODE_NO_AUTO_UPDATE=1
BROWSER=true
NO_COLOR=1
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ENVFILE

# lxc.init.cmd does not read /etc/environment (PAM does), so the wrapper loads
# it without overriding values the instance already set, makes the per-runner
# volume writable for the runner user, drops root and replaces itself with the
# runner. tini is PID 1's child so zombies from CLI children get reaped.
incus file push - "$BUILDER/usr/local/bin/allternit-runner-init" --mode 0755 <<'WRAPPER'
#!/bin/sh
# Runner entry point (lxc.init.cmd). Starts as root inside the container only
# to fix the volume owner, then runs the runner as user "runner".
set -eu
if [ -r /etc/environment ]; then
  while IFS='=' read -r k v; do
    case "$k" in ''|'#'*) continue ;; esac
    eval "[ -n \"\${$k:-}\" ]" || export "$k=$v"
  done < /etc/environment
fi
: "${ALLTERNIT_RUNNER_HOME:=/home/runner}"
export ALLTERNIT_RUNNER_HOME HOME=/home/runner
mkdir -p "$ALLTERNIT_RUNNER_HOME"
chown runner:runner "$ALLTERNIT_RUNNER_HOME"
chmod 0700 "$ALLTERNIT_RUNNER_HOME"
exec setpriv --reuid=runner --regid=runner --init-groups --inh-caps=-all --no-new-privs \
  /usr/bin/tini -s -- /usr/local/bin/allternit-brain-runner "$@"
WRAPPER

log "trimming (no SSH, no desktop, small)"
x sh -c 'export DEBIAN_FRONTEND=noninteractive
  apt-get purge -y -qq openssh-server openssh-client cloud-init 2>/dev/null || true
  apt-get autoremove -y -qq --purge >/dev/null
  apt-get clean
  rm -rf /var/lib/apt/lists/* /var/cache/apt/* /tmp/* /var/tmp/* /usr/share/doc/* /usr/share/man/* /usr/share/info/* /root/.npm /root/.cache
  find /usr/share/locale -mindepth 1 -maxdepth 1 ! -name "en*" -exec rm -rf {} + 2>/dev/null || true
  rm -f /etc/ssh/ssh_host_* 2>/dev/null || true'

log "checking: every CLI prints its version"
for c in claude codex grok kimi; do
  v=$(x su -s /bin/sh runner -c "HOME=/home/runner $c --version" 2>&1 | head -1) || die "$c failed to run"
  log "  $c -> $v"
done
x /usr/local/bin/allternit-brain-runner --help >/dev/null 2>&1 || log "  note: allternit-brain-runner --help exited non-zero (check the binary)"
x sh -c 'test ! -e /usr/sbin/sshd && echo "  no sshd: ok"'

log "publishing"
incus stop "$BUILDER"
STAMP=$(date -u +%Y%m%d)
FP_OUT=$(incus publish "$BUILDER" --alias "${IMAGE_ALIAS}-build-$$" \
  "description=Allternit Brain runner ${STAMP} (claude ${CLAUDE_VERSION}, codex ${CODEX_VERSION}, grok ${GROK_VERSION}, kimi ${KIMI_VERSION}, node ${NODE_VERSION})" \
  "properties.os=Ubuntu" "properties.release=noble" "properties.purpose=brain-runner" 2>&1)
echo "$FP_OUT"
TARBALL_BASE="${OUT_DIR}/${IMAGE_ALIAS}-${STAMP}"
incus image export "${IMAGE_ALIAS}-build-$$" "$TARBALL_BASE"
incus image delete "${IMAGE_ALIAS}-build-$$" >/dev/null 2>&1 || true
TARBALL=$(find "$OUT_DIR" -maxdepth 1 -name "$(basename "$TARBALL_BASE")*.tar.gz" | head -1)
[ -n "$TARBALL" ] || die "export produced no tarball at ${TARBALL_BASE}"
if command -v sha256sum >/dev/null 2>&1; then sha256sum "$TARBALL" > "${TARBALL}.sha256"; else shasum -a 256 "$TARBALL" > "${TARBALL}.sha256"; fi

SIZE_MB=$(( $(wc -c < "$TARBALL") / 1048576 ))
log "image tarball: ${TARBALL} (${SIZE_MB} MB; target <= ${SIZE_LIMIT_MB} MB)"
cat "${TARBALL}.sha256"
if [ "$SIZE_MB" -gt "$SIZE_LIMIT_MB" ]; then
  echo "WARNING: image is over the ${SIZE_LIMIT_MB} MB target" >&2
fi
echo
echo "Import on the serving host (after copying the tarball there):"
echo "  incus image import ${TARBALL} --alias ${IMAGE_ALIAS}"
echo "or let brain-runner-host.sh do it:  --image-tarball ${TARBALL}"
