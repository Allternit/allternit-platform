#!/usr/bin/env bash
#
# Gizzi Code installer (thin redirect).
# The canonical script is cmd/gizzi-code/install, served as
# https://install.gizziio.com/install. It reads the version from the signed
# release feed and always verifies sha256. This file only hands off to it,
# so the two can never drift.
#
# Usage: ./install.sh [options]   (same options as the canonical script)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
canonical="$here/../../install"

if [ -f "$canonical" ]; then
    exec bash "$canonical" "$@"
fi

# Packaged without the repo: fetch the canonical script.
exec bash -c 'curl -fsSL https://install.gizziio.com/install | bash -s -- "$@"' _ "$@"
