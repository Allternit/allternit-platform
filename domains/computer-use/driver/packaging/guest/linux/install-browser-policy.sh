#!/bin/bash
# Force-install the Allternit Chrome extension in every agent Chrome on a
# Linux guest, through Chrome's managed policy
# (/etc/opt/chrome/policies/managed/allternit.json, ExtensionSettings).
# Runs inside the guest as root; idempotent. install-driver.sh calls it.
#
# Chrome only force-installs an extension it can fetch from an update
# manifest. The extension (surfaces/allternit-extensions/allternit-extension,
# id chheieepkpbhkiimdmbdjmnhcooclpok from the manifest key in wxt.config.ts)
# is NOT on the Chrome Web Store (the store's update service answers
# error-unknownApplication for that id, checked 2026-11-26), and nothing hosts
# a CRX or an update.xml for it yet. So the policy is written only when both
# are given:
#
#   ALLTERNIT_EXTENSION_UPDATE_URL  https URL of an update manifest (gupdate
#                                   XML) whose <updatecheck codebase=...> is a
#                                   CRX3 of the extension signed with the
#                                   private key behind the manifest key
#   ALLTERNIT_EXTENSION_ID          optional; default chheieepkpbhkiimdmbdjmnhcooclpok
#
# Without ALLTERNIT_EXTENSION_UPDATE_URL it removes any old policy file and
# says what is missing. It never points Chrome at the Web Store for this id.
set -euo pipefail

POLICY_DIR=/etc/opt/chrome/policies/managed
POLICY_FILE="${POLICY_DIR}/allternit.json"
EXT_ID="${ALLTERNIT_EXTENSION_ID:-chheieepkpbhkiimdmbdjmnhcooclpok}"
UPDATE_URL="${ALLTERNIT_EXTENSION_UPDATE_URL:-}"

case "${EXT_ID}" in
    *[!a-p]*|"") echo "ERROR: ALLTERNIT_EXTENSION_ID must be 32 letters a-p" >&2; exit 1 ;;
esac
[ "${#EXT_ID}" -eq 32 ] || { echo "ERROR: ALLTERNIT_EXTENSION_ID must be 32 letters a-p" >&2; exit 1; }

if [ -z "${UPDATE_URL}" ]; then
    rm -f "${POLICY_FILE}"
    echo "[install-browser-policy] extension NOT force-installed: no ALLTERNIT_EXTENSION_UPDATE_URL." >&2
    echo "[install-browser-policy] missing: (1) a CRX3 of the extension signed with the key behind id ${EXT_ID}," >&2
    echo "[install-browser-policy]          (2) an https update manifest that serves it (or a Web Store listing)." >&2
    exit 0
fi
case "${UPDATE_URL}" in
    https://*) ;;
    *) echo "ERROR: ALLTERNIT_EXTENSION_UPDATE_URL must be https" >&2; exit 1 ;;
esac
case "${UPDATE_URL}" in
    *\"*|*\\*) echo "ERROR: ALLTERNIT_EXTENSION_UPDATE_URL has a quote or backslash" >&2; exit 1 ;;
esac

mkdir -p "${POLICY_DIR}"
cat > "${POLICY_FILE}.tmp" <<EOF
{
  "ExtensionSettings": {
    "${EXT_ID}": {
      "installation_mode": "force_installed",
      "update_url": "${UPDATE_URL}",
      "toolbar_pin": "force_pinned"
    }
  }
}
EOF
chmod 0644 "${POLICY_FILE}.tmp"
mv "${POLICY_FILE}.tmp" "${POLICY_FILE}"
echo "[install-browser-policy] ${EXT_ID} force-installed from ${UPDATE_URL}"
