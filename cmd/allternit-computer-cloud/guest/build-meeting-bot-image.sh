#!/bin/bash
# Build the Allternit Meetings pod image (plan step 2.1). One container from
# this image = one meeting session: services/meeting-bot's runner drives a
# headed Google Chrome on Xvfb :0, joins the call, publishes the call audio to
# LiveKit and reports to cloud-api with its pod token.
#
# Derived from the headless computer image (build-headless-computer-image.sh),
# which already has Google Chrome, Xvfb :0 (allternit-xvfb.service), the
# Allternit runtime (allternit-api + gizzi-code, for the in-pod agent session)
# and systemd. This adds:
#   - PulseAudio with a null sink `meet_out` as the default sink. Chrome needs
#     an output device for call audio; nothing records it (the runner captures
#     the call from the page's peer connections), so it is a sink, not a source.
#   - Node 20+ and the built @allternit/meeting-bot package (pnpm deploy --prod)
#     in /opt/allternit/meeting-bot.
#   - An unprivileged user `meetbot` that runs PulseAudio, Chrome and the runner
#     (Chrome keeps its sandbox; no --no-sandbox as root).
#   - allternit-meeting-bot.service, started by the launcher after it writes
#     /etc/allternit/meeting-pod.json (PodConfig, CONTRACT section 6) and
#     /etc/allternit/meeting-pod.env. The unit is installed disabled: an image
#     never starts a bot on its own.
#   - meeting-pod.env is loaded by BOTH the runner and the Allternit app
#     (allternit-headless.service drop-in), because the in-pod allternit-api
#     serves the meeting turn routes (CONTRACT "meeting turns"):
#       ALLTERNIT_POD_TOKEN, ALLTERNIT_LIVEKIT_TOKEN      runner
#       ALLTERNIT_MEETING_CLOUD_API_URL,
#       ALLTERNIT_MEETING_POD_TOKEN                       allternit-api (= cloudApiUrl / podToken)
#       ALLTERNIT_INTERNAL_SERVICE_TOKEN                  both (runner -> allternit-api turn routes)
#       ALLTERNIT_API_PORT=8013, ALLTERNIT_MEETING_TURN_URL=http://127.0.0.1:8013
#     The launcher writes all of them; the token values are per session.
#   - /var/lib/allternit/bot-profile, where the bot Google account's Chrome
#     profile lives during a session (docs/meetings/BOT-ACCOUNTS.md), and the
#     two helpers around it:
#       allternit-meeting-profile.service (oneshot, Before= the runner,
#         ConditionPathExists= the archive): unpacks the archive the launcher
#         pushes to /var/lib/allternit-profile-seed/profile.tar.zst into
#         /var/lib/allternit/bot-profile (meetbot, 0700), deletes the archive
#         and leaves the marker /var/lib/allternit/bot-profile.delivered.
#       ExecStopPost of the runner (as root): when a profile was delivered and
#         the runner did not log bot_profile_signed_out, tars it (caches and
#         Singleton* locks excluded) to /var/lib/allternit-profile-out/
#         profile.tar.zst for the reaper's once-a-day write-back.
#     zstd is installed for both.
#   - allternit-meeting-dns.service: when a pod boots with no /etc/resolv.conf
#     (guest images expect provisioning to write it; the launcher does not),
#     points it at the default gateway so Chrome can resolve the meeting.
#
# Built like the other guest images: launch the base, change it, publish.
# Run on the Incus host that will serve the image. Not run by CI.
#
# Environment variables:
#   BASE_IMAGE   - base alias (default: allternit-computer-headless). It may be
#                  an alias copied from another host (e.g.
#                  `incus image copy <remote>:allternit-computer-headless local: --alias ...`);
#                  only the local alias name matters here.
#   IMAGE_NAME   - published alias (default: allternit-meeting-bot)
#   REPO_DIR     - platform repo checkout to build the bot from (default: this
#                  script's repo); needs pnpm and Node 20+ on the build host
#   BOT_DIR      - optional pre-built deploy dir (the output of
#                  `pnpm --filter @allternit/meeting-bot deploy --prod --legacy <dir>`
#                  after `pnpm --filter @allternit/meeting-bot run build`);
#                  skips the local build
#   DROP_RUNTIME - if set, the Allternit app service (allternit-headless) is
#                  disabled (~400 MB less per pod). Gizzi then joins, transcribes
#                  and posts the notice but cannot answer turns or write a summary.
#   API_BINARY   - optional Linux allternit-api binary installed over every
#                  bundled copy (same as build-headless-computer-image.sh).
#                  Needed when the base image's allternit-api predates the
#                  meeting turn routes (the 2026-10-10 base does), e.g. the
#                  host's deployed /opt/allternit-api/bin/allternit-api from main
#   ZOOM_SIDECAR_TGZ - optional built Zoom sidecar tree (a .tgz that unpacks to
#                  `zoom/`; services/meeting-bot/src/adapters/zoom/sidecar,
#                  e.g. /root/meetings-build/zoom-sidecar-7.1.5.4432.tgz on
#                  allternit-standby). Installed to /opt/allternit/zoom with the
#                  sidecar's runtime packages (its Dockerfile's "runtime" list)
#                  and checked with ldd. Unset: no sidecar, and Zoom sessions
#                  in these pods end join_failed.
#   BUILD_NETWORK - Incus bridge the builder is on (default: incusbr0). When
#                  the builder cannot resolve names (guest images come up with
#                  no /etc/resolv.conf; provisioning normally writes it), the
#                  bridge gateway is written to the builder's /etc/resolv.conf
#                  for the build and removed again before publishing.
#   SKIP_PUBLISH - if set, stop after the smoke check and cleaning: the builder
#                  is left stopped (not deleted) and the exact `incus publish`
#                  command is printed instead of run
#   KEEP_BUILDER - if set, do not delete the build container

set -euo pipefail

BASE_IMAGE="${BASE_IMAGE:-allternit-computer-headless}"
IMAGE_NAME="${IMAGE_NAME:-allternit-meeting-bot}"
BUILD_CONTAINER="allternit-meeting-bot-builder-$$"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="${REPO_DIR:-$(cd "${HERE}/../../.." && pwd)}"
WORK="$(mktemp -d /tmp/allternit-meeting-bot.XXXXXX)"

log() { echo "[build-meeting-bot-image] $*"; }
cleanup() {
    rm -rf "${WORK}"
    if [ -z "${KEEP_BUILDER:-}" ]; then
        incus delete -f "${BUILD_CONTAINER}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

if [ -n "${API_BINARY:-}" ]; then
    [ -x "${API_BINARY}" ] || { echo "ERROR: ${API_BINARY} is not an executable" >&2; exit 1; }
fi
if [ -n "${ZOOM_SIDECAR_TGZ:-}" ]; then
    [ -f "${ZOOM_SIDECAR_TGZ}" ] || { echo "ERROR: ${ZOOM_SIDECAR_TGZ} not found" >&2; exit 1; }
else
    log "ZOOM_SIDECAR_TGZ unset: no Zoom sidecar in this image; Zoom sessions in these pods will end join_failed"
fi
BUILD_NETWORK="${BUILD_NETWORK:-incusbr0}"
# The Zoom sidecar's runtime packages (sidecar/Dockerfile, "runtime" list).
ZOOM_RUNTIME_PACKAGES="libglib2.0-0 libdbus-1-3 libgbm1 libgl1 libgl1-mesa-dri libxfixes3 libx11-xcb1 libxcb-xfixes0 libxcb-shape0 libxcb-shm0 libxcb-randr0 libxcb-image0 libxcb-keysyms1 libxcb-xtest0"

# Profile helpers (pushed in step 3). The cache excludes must match
# scripts/meetings/bot-profile-signin.sh.
cat > "${WORK}/allternit-meeting-profile-unpack" <<'SCRIPT'
#!/bin/bash
# allternit-meeting-profile.service: unpack the bot account's Chrome profile
# the launcher pushed, then delete the archive (docs/meetings/BOT-ACCOUNTS.md).
set -euo pipefail
seed_dir=/var/lib/allternit-profile-seed
dest=/var/lib/allternit/bot-profile
trap 'rm -rf "$seed_dir"' EXIT
rm -f /var/lib/allternit/bot-profile.delivered
mkdir -p "$dest"
find "$dest" -mindepth 1 -delete
zstd -q -dc "$seed_dir/profile.tar.zst" | tar -C "$dest" --no-same-owner -xf -
chown -R meetbot:meetbot "$dest"
chmod 0700 "$dest"
touch /var/lib/allternit/bot-profile.delivered
echo "bot profile unpacked into $dest"
SCRIPT
cat > "${WORK}/allternit-meeting-profile-save" <<'SCRIPT'
#!/bin/bash
# ExecStopPost (root) of allternit-meeting-bot.service: leave the bot
# account's Chrome profile at /var/lib/allternit-profile-out/profile.tar.zst
# for the reaper, which pulls it before deleting the pod and seals it at most
# once a day per account (docs/meetings/BOT-ACCOUNTS.md). Never fails the unit.
set -uo pipefail
src=/var/lib/allternit/bot-profile
out=/var/lib/allternit-profile-out
# Only a profile the launcher delivered: never a guest or empty one.
[ -f /var/lib/allternit/bot-profile.delivered ] || exit 0
if journalctl -b -u allternit-meeting-bot.service -o cat --no-pager 2>/dev/null | grep -q bot_profile_signed_out; then
    echo "bot profile was signed out; not saved"
    exit 0
fi
umask 077
mkdir -p "$out" && chmod 0700 "$out" || exit 0
tmp="$out/profile.tar.zst.tmp"
tar -C "$src" \
    --exclude=Cache --exclude='Code Cache' --exclude=GPUCache --exclude='Service Worker/CacheStorage' \
    --exclude=Crashpad --exclude=GrShaderCache --exclude=ShaderCache --exclude=DawnCache \
    --exclude=component_crx_cache --exclude=optimization_guide_model_store --exclude=WasmTtsEngine --exclude="Safe Browsing" --exclude=CertificateRevocation --exclude=OnDeviceHeadSuggestModel --exclude="BrowserMetrics*" --exclude=ActorSafetyLists --exclude=extensions_crx_cache \
    --exclude='Singleton*' \
    -cf - . 2>/dev/null | zstd -q -3 -c > "$tmp"
status=("${PIPESTATUS[@]}")
# tar 1 = a file changed while read (Chrome has exited; harmless).
if [ "${status[0]}" -le 1 ] && [ "${status[1]}" -eq 0 ] && [ -s "$tmp" ]; then
    chmod 0600 "$tmp" && mv -f "$tmp" "$out/profile.tar.zst" && echo "bot profile saved for write-back"
else
    rm -f "$tmp"
    echo "saving the bot profile failed (tar ${status[0]}, zstd ${status[1]})"
fi
exit 0
SCRIPT
cat > "${WORK}/allternit-meeting-dns-fallback" <<'SCRIPT'
#!/bin/bash
# allternit-meeting-dns.service: guest images expect provisioning to write
# /etc/resolv.conf, and the meeting launcher does not. Without it the pod
# cannot resolve meet.google.com, so point it at the default gateway (the
# Incus bridge runs a DNS forwarder there). Runs only when the file is missing.
set -uo pipefail
for _ in $(seq 1 30); do
    gw=$(ip -4 route show default 2>/dev/null | awk '{print $3; exit}')
    [ -n "$gw" ] && break
    sleep 1
done
[ -n "${gw:-}" ] || { echo "no default route; resolv.conf not written"; exit 0; }
getent hosts meet.google.com >/dev/null 2>&1 && exit 0
rm -f /etc/resolv.conf
echo "nameserver $gw" > /etc/resolv.conf
echo "resolv.conf: nameserver $gw"
SCRIPT
cat > "${WORK}/allternit-meeting-dns.service" <<'UNIT'
[Unit]
Description=Allternit meeting pod: DNS fallback when provisioning wrote no resolv.conf
Wants=network-online.target
After=network-online.target
Before=allternit-meeting-bot.service allternit-headless.service
ConditionPathExists=!/etc/resolv.conf

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/allternit-meeting-dns-fallback

[Install]
WantedBy=multi-user.target
UNIT
cat > "${WORK}/allternit-meeting-profile.service" <<'UNIT'
[Unit]
Description=Allternit meeting pod: unpack the bot Google account profile
Before=allternit-meeting-bot.service
ConditionPathExists=/var/lib/allternit-profile-seed/profile.tar.zst

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/allternit-meeting-profile-unpack

[Install]
WantedBy=multi-user.target
UNIT

# 0. The bot package, built on the host (dist + production node_modules).
if [ -n "${BOT_DIR:-}" ]; then
    [ -f "${BOT_DIR}/dist/runner.js" ] || { echo "ERROR: ${BOT_DIR}/dist/runner.js not found" >&2; exit 1; }
    DEPLOY_DIR="${BOT_DIR}"
else
    command -v pnpm >/dev/null || { echo "ERROR: pnpm is needed to build the bot (or pass BOT_DIR)" >&2; exit 1; }
    log "building @allternit/meeting-bot from ${REPO_DIR}"
    (cd "${REPO_DIR}" && pnpm --filter @allternit/meeting-bot run build)
    DEPLOY_DIR="${WORK}/meeting-bot"
    (cd "${REPO_DIR}" && pnpm --filter @allternit/meeting-bot deploy --prod --legacy "${DEPLOY_DIR}")
    [ -f "${DEPLOY_DIR}/dist/runner.js" ] || { echo "ERROR: deploy dir has no dist/runner.js" >&2; exit 1; }
fi
# The spike stays in the repo; the pod only needs the package.
rm -rf "${DEPLOY_DIR:?}/spike" "${DEPLOY_DIR:?}/test" "${DEPLOY_DIR:?}/src"
tar -C "${DEPLOY_DIR}" -cf "${WORK}/meeting-bot.tar" .

log "launching ${BASE_IMAGE} as ${BUILD_CONTAINER}"
incus launch --quiet "${BASE_IMAGE}" "${BUILD_CONTAINER}" -c limits.cpu=2 -c limits.memory=3GiB -c limits.cpu.priority=1
for _ in $(seq 1 60); do
    incus exec "${BUILD_CONTAINER}" -- true >/dev/null 2>&1 && break
    sleep 1
done
for _ in $(seq 1 90); do
    incus exec "${BUILD_CONTAINER}" -- test -S /run/systemd/private >/dev/null 2>&1 && break
    sleep 1
done
incus exec "${BUILD_CONTAINER}" -- sh -c 'timeout 120 systemctl is-system-running --wait >/dev/null 2>&1 || true'

# 0b. DNS for apt. Guest images expect provisioning to write /etc/resolv.conf;
#     a bare builder may have none. Use the bridge gateway (Incus runs a DNS
#     forwarder there) and remember what was there, so cleaning restores it.
if ! incus exec "${BUILD_CONTAINER}" -- getent hosts archive.ubuntu.com >/dev/null 2>&1; then
    gateway="$(incus network get "${BUILD_NETWORK}" ipv4.address 2>/dev/null | cut -d/ -f1)"
    [ -n "${gateway}" ] || { echo "ERROR: the builder cannot resolve names and ${BUILD_NETWORK} has no ipv4.address (set BUILD_NETWORK)" >&2; exit 1; }
    log "builder has no working DNS; using ${gateway} (${BUILD_NETWORK}) for the build"
    incus exec "${BUILD_CONTAINER}" -- sh -c "
        set -e
        if [ -L /etc/resolv.conf ]; then printf 'link %s\n' \"\$(readlink /etc/resolv.conf)\" > /etc/resolv.conf.allternit-builder;
        elif [ -e /etc/resolv.conf ]; then cp -a /etc/resolv.conf /etc/resolv.conf.allternit-orig && echo file > /etc/resolv.conf.allternit-builder;
        else echo none > /etc/resolv.conf.allternit-builder; fi
        rm -f /etc/resolv.conf
        echo 'nameserver ${gateway}' > /etc/resolv.conf
    "
    incus exec "${BUILD_CONTAINER}" -- getent hosts archive.ubuntu.com >/dev/null 2>&1 \
        || { echo "ERROR: the builder still cannot resolve archive.ubuntu.com via ${gateway}" >&2; exit 1; }
fi

# 1. Packages: PulseAudio, Xvfb (already in the base; checked), Node 20+.
log "installing PulseAudio and Node"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    set -e
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends pulseaudio pulseaudio-utils xvfb ca-certificates curl fonts-noto-color-emoji zstd iproute2
    major=$(node -v 2>/dev/null | sed -E "s/^v([0-9]+).*/\1/" || echo 0)
    if [ "${major:-0}" -lt 20 ]; then
        curl -fsSL https://deb.nodesource.com/setup_20.x | bash -
        apt-get install -y -qq nodejs
    fi
    node -v
    # The unit runs /usr/bin/node; the headless base ships Node in /usr/local/bin.
    [ -x /usr/bin/node ] || ln -sf "$(command -v node)" /usr/bin/node
    /usr/bin/node -v
    for c in /usr/bin/google-chrome /usr/bin/google-chrome-stable /opt/google/chrome/chrome; do
        [ -x "$c" ] && { ln -sf "$c" /usr/local/bin/allternit-meeting-chrome; break; }
    done
    test -x /usr/local/bin/allternit-meeting-chrome || { echo "Google Chrome missing from the base image" >&2; exit 1; }
'

# 2. The bot package and its user.
log "installing the bot package"
incus file push --quiet "${WORK}/meeting-bot.tar" "${BUILD_CONTAINER}/root/meeting-bot.tar"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    set -e
    id meetbot >/dev/null 2>&1 || useradd --system --create-home --home-dir /var/lib/meetbot --shell /usr/sbin/nologin meetbot
    usermod -aG audio meetbot
    rm -rf /opt/allternit/meeting-bot && mkdir -p /opt/allternit/meeting-bot
    tar -C /opt/allternit/meeting-bot -xf /root/meeting-bot.tar
    rm -f /root/meeting-bot.tar
    chown -R root:root /opt/allternit/meeting-bot
    test -f /opt/allternit/meeting-bot/dist/runner.js
    test -d /opt/allternit/meeting-bot/node_modules/playwright-core
    test -d /opt/allternit/meeting-bot/node_modules/livekit-client
    # The bot account profile volume (persisted, encrypted at rest on the host; BOT-ACCOUNTS.md).
    mkdir -p /var/lib/allternit/bot-profile && chown meetbot:meetbot /var/lib/allternit/bot-profile && chmod 0700 /var/lib/allternit/bot-profile
    mkdir -p /etc/allternit && chmod 0755 /etc/allternit
'

# 2b. Optional newer allternit-api over every bundled copy (the meeting turn
#     routes live in allternit-api; an old base image lacks them).
if [ -n "${API_BINARY:-}" ]; then
    log "installing $(basename "${API_BINARY}") over the bundled allternit-api"
    incus file push --quiet "${API_BINARY}" "${BUILD_CONTAINER}/root/allternit-api.new"
    incus exec "${BUILD_CONTAINER}" -- sh -c '
        set -e
        find /opt /usr/lib -type f -name allternit-api -perm -u+x 2>/dev/null > /root/api-paths
        [ -s /root/api-paths ] || { echo "no bundled allternit-api found" >&2; exit 1; }
        while IFS= read -r f; do
            install -m 0755 /root/allternit-api.new "$f"; echo "replaced $f"
        done < /root/api-paths
        rm -f /root/allternit-api.new /root/api-paths
    '
fi

# 2b'. Optional Zoom sidecar (the native Meeting SDK process the Zoom adapter
#      spawns; index.ts looks for /opt/allternit/zoom/zoom-sidecar).
if [ -n "${ZOOM_SIDECAR_TGZ:-}" ]; then
    log "installing the Zoom sidecar from $(basename "${ZOOM_SIDECAR_TGZ}")"
    incus file push --quiet "${ZOOM_SIDECAR_TGZ}" "${BUILD_CONTAINER}/root/zoom-sidecar.tgz"
    incus exec "${BUILD_CONTAINER}" -- sh -c "
        set -e
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends ${ZOOM_RUNTIME_PACKAGES}
        rm -rf /root/zoom-unpack && mkdir -p /root/zoom-unpack
        tar -C /root/zoom-unpack --no-same-owner -xzf /root/zoom-sidecar.tgz
        test -x /root/zoom-unpack/zoom/zoom-sidecar || { echo 'the tarball has no zoom/zoom-sidecar' >&2; exit 1; }
        rm -rf /opt/allternit/zoom && mkdir -p /opt/allternit
        mv /root/zoom-unpack/zoom /opt/allternit/zoom
        rm -rf /root/zoom-unpack /root/zoom-sidecar.tgz
        chown -R root:root /opt/allternit/zoom
        chmod -R u=rwX,go=rX /opt/allternit/zoom
        chmod 0755 /opt/allternit/zoom/zoom-sidecar
    "
fi

# 2c. The profile helpers.
log "installing allternit-meeting-profile.service and the profile save hook"
incus file push --quiet --uid 0 --gid 0 --mode 0755 "${WORK}/allternit-meeting-profile-unpack" "${BUILD_CONTAINER}/usr/local/sbin/allternit-meeting-profile-unpack"
incus file push --quiet --uid 0 --gid 0 --mode 0755 "${WORK}/allternit-meeting-profile-save" "${BUILD_CONTAINER}/usr/local/sbin/allternit-meeting-profile-save"
incus file push --quiet --uid 0 --gid 0 --mode 0644 "${WORK}/allternit-meeting-profile.service" "${BUILD_CONTAINER}/etc/systemd/system/allternit-meeting-profile.service"
incus file push --quiet --uid 0 --gid 0 --mode 0755 "${WORK}/allternit-meeting-dns-fallback" "${BUILD_CONTAINER}/usr/local/sbin/allternit-meeting-dns-fallback"
incus file push --quiet --uid 0 --gid 0 --mode 0644 "${WORK}/allternit-meeting-dns.service" "${BUILD_CONTAINER}/etc/systemd/system/allternit-meeting-dns.service"

# 3. Services: per-user PulseAudio with the null sink, then the runner.
log "installing allternit-meeting-pulse.service and allternit-meeting-bot.service"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    set -e
    mkdir -p /etc/allternit/pulse
    cat > /etc/allternit/pulse/meeting.pa <<PA
.fail
load-module module-null-sink sink_name=meet_out sink_properties=device.description=meet_out rate=48000
set-default-sink meet_out
load-module module-native-protocol-unix auth-anonymous=1 socket=/run/meetbot/pulse/native
load-module module-always-sink
PA
    cat > /etc/systemd/system/allternit-meeting-pulse.service <<UNIT
[Unit]
Description=Allternit meeting pod audio (PulseAudio, null sink meet_out)

[Service]
User=meetbot
Group=meetbot
RuntimeDirectory=meetbot meetbot/pulse
RuntimeDirectoryMode=0700
Environment=XDG_RUNTIME_DIR=/run/meetbot
ExecStart=/usr/bin/pulseaudio --daemonize=no --exit-idle-time=-1 --disallow-exit --disallow-module-loading=no -n -F /etc/allternit/pulse/meeting.pa
Restart=always
RestartSec=2
UNIT
    cat > /etc/systemd/system/allternit-meeting-bot.service <<UNIT
[Unit]
Description=Allternit meeting pod (services/meeting-bot runner, one session)
After=network-online.target allternit-xvfb.service allternit-meeting-pulse.service allternit-headless.service allternit-meeting-profile.service
Wants=network-online.target allternit-headless.service
Requires=allternit-xvfb.service allternit-meeting-pulse.service
ConditionPathExists=/etc/allternit/meeting-pod.json

[Service]
Type=simple
User=meetbot
Group=meetbot
WorkingDirectory=/opt/allternit/meeting-bot
Environment=NODE_ENV=production
Environment=DISPLAY=:0
Environment=XDG_RUNTIME_DIR=/run/meetbot
Environment=PULSE_SERVER=unix:/run/meetbot/pulse/native
Environment=PULSE_SINK=meet_out
Environment=CHROME_PATH=/usr/local/bin/allternit-meeting-chrome
Environment=ALLTERNIT_CHROME_NO_SANDBOX=1
Environment=ALLTERNIT_POD_CONFIG_FILE=/etc/allternit/meeting-pod.json
Environment=MEETING_BOT_SCREENSHOT_DIR=/var/lib/meetbot/shots
Environment=ALLTERNIT_MEETING_TURN_URL=http://127.0.0.1:8013
EnvironmentFile=-/etc/allternit/meeting-pod.env
ExecStartPre=/bin/mkdir -p /var/lib/meetbot/shots
ExecStart=/usr/bin/node /opt/allternit/meeting-bot/dist/runner.js
# As root, after the runner and Chrome exited: save the profile for write-back.
ExecStopPost=-+/usr/local/sbin/allternit-meeting-profile-save
# One session per container: the runner exits when the session ends (0 = ended,
# 3 = not admitted / blocked / join failed, 1 = error). The launcher reads the
# exit and deletes the container; systemd never restarts a finished session.
Restart=no
TimeoutStopSec=30
KillSignal=SIGTERM
UNIT
    # The in-pod allternit-api (inside the Allternit app) gets the same env file,
    # so ALLTERNIT_MEETING_CLOUD_API_URL / ALLTERNIT_MEETING_POD_TOKEN /
    # ALLTERNIT_INTERNAL_SERVICE_TOKEN reach the turn routes.
    if [ -f /etc/systemd/system/allternit-headless.service ]; then
        mkdir -p /etc/systemd/system/allternit-headless.service.d
        cat > /etc/systemd/system/allternit-headless.service.d/meeting-pod.conf <<UNIT
[Service]
Environment=ALLTERNIT_API_PORT=8013
EnvironmentFile=-/etc/allternit/meeting-pod.env
UNIT
    fi
    systemctl daemon-reload
    systemctl enable allternit-meeting-pulse.service
    systemctl enable allternit-meeting-profile.service
    systemctl enable allternit-meeting-dns.service
    systemctl disable allternit-meeting-bot.service >/dev/null 2>&1 || true
'

if [ -n "${DROP_RUNTIME:-}" ]; then
    log "disabling the Allternit app service (DROP_RUNTIME set; no turns or summary in these pods)"
    incus exec "${BUILD_CONTAINER}" -- sh -c 'systemctl disable allternit-headless.service >/dev/null 2>&1 || true'
fi

# 4. Smoke check inside the builder: the runner loads and refuses to start
#    without a pod config (no network, no Chrome launch).
log "smoke check"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    # The exact interpreter the unit runs (a PATH-only node passed this check
    # once while the unit could not start: status=203/EXEC).
    exec_node=$(sed -n "s|^ExecStart=\([^ ]*\) .*|\1|p" /etc/systemd/system/allternit-meeting-bot.service)
    [ -x "$exec_node" ] || { echo "unit ExecStart interpreter $exec_node is missing" >&2; exit 1; }
    out=$(cd /opt/allternit/meeting-bot && su -s /bin/sh meetbot -c "$exec_node dist/runner.js" 2>&1; true)
    echo "$out" | grep -q "ALLTERNIT_POD_CONFIG" || { echo "runner smoke check failed: $out" >&2; exit 1; }
    echo "runner refuses to start without a pod config: ok"
    systemctl is-enabled allternit-meeting-profile.service >/dev/null || { echo "allternit-meeting-profile.service is not enabled" >&2; exit 1; }
    test -x /usr/local/sbin/allternit-meeting-profile-unpack && test -x /usr/local/sbin/allternit-meeting-profile-save
    systemctl show -p ExecStopPost allternit-meeting-bot.service | grep -q allternit-meeting-profile-save || { echo "runner unit has no profile save hook" >&2; exit 1; }
    command -v zstd >/dev/null || { echo "zstd missing" >&2; exit 1; }
    systemctl is-enabled allternit-meeting-dns.service >/dev/null || { echo "allternit-meeting-dns.service is not enabled" >&2; exit 1; }
    command -v ip >/dev/null || { echo "iproute2 missing (the DNS fallback needs ip)" >&2; exit 1; }
    echo "profile unit installed: ok"
'
if [ -n "${ZOOM_SIDECAR_TGZ:-}" ]; then
    incus exec "${BUILD_CONTAINER}" -- sh -c '
        missing=$(ldd /opt/allternit/zoom/zoom-sidecar 2>&1 | grep "not found" || true)
        [ -z "$missing" ] || { echo "zoom-sidecar has unresolved libraries:" >&2; echo "$missing" >&2; exit 1; }
        echo "zoom-sidecar libraries resolve: ok"
    '
fi

# 5. Clean and publish.
log "cleaning"
incus exec "${BUILD_CONTAINER}" -- sh -c 'apt-get clean; rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*; rm -f /etc/allternit/bootstrap.json /etc/allternit/meeting-pod.json /etc/allternit/meeting-pod.env; rm -rf /var/lib/allternit-profile-seed /var/lib/allternit-profile-out /var/lib/allternit/bot-profile.delivered; find /var/lib/allternit/bot-profile -mindepth 1 -delete; truncate -s 0 /etc/machine-id'
# Put /etc/resolv.conf back the way the base had it (step 0b).
incus exec "${BUILD_CONTAINER}" -- sh -c '
    [ -f /etc/resolv.conf.allternit-builder ] || exit 0
    was=$(cat /etc/resolv.conf.allternit-builder)
    rm -f /etc/resolv.conf
    case "$was" in
        "link "*) ln -s "${was#link }" /etc/resolv.conf ;;
        file) mv /etc/resolv.conf.allternit-orig /etc/resolv.conf ;;
    esac
    rm -f /etc/resolv.conf.allternit-builder /etc/resolv.conf.allternit-orig
'
incus stop "${BUILD_CONTAINER}"
PUBLISH_DESC="Allternit Meetings pod: Chrome on Xvfb + PulseAudio null sink + @allternit/meeting-bot (from ${BASE_IMAGE})"
if [ -n "${SKIP_PUBLISH:-}" ]; then
    KEEP_BUILDER=1
    log "SKIP_PUBLISH set: builder ${BUILD_CONTAINER} left stopped, not published. To publish:"
    printf '  incus image delete %q 2>/dev/null; incus publish %q --alias %q description=%q --compression=zstd\n' \
        "${IMAGE_NAME}" "${BUILD_CONTAINER}" "${IMAGE_NAME}" "${PUBLISH_DESC}"
    log "then delete the builder: incus delete ${BUILD_CONTAINER}"
    exit 0
fi
log "publishing ${IMAGE_NAME}"
incus image delete "${IMAGE_NAME}" >/dev/null 2>&1 || true
incus publish "${BUILD_CONTAINER}" --alias "${IMAGE_NAME}" description="${PUBLISH_DESC}" --compression=zstd
incus image info "${IMAGE_NAME}" | head -8
