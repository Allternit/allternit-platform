#!/usr/bin/env bash
# Rebuild the JavaScriptCore bundle the iPhone app uses to lay out document templates.
# Run after changing templates.ts, themes.ts or html.ts; commit the output.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="../../../surfaces/allternit-mobile/ios/Features/Artifacts/Resources/pdf-templates.js"
mkdir -p "$(dirname "$OUT")"
bun build src/ios-entry.ts --target=browser --format=iife --minify --outfile "$OUT"
