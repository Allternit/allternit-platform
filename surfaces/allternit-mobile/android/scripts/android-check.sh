#!/usr/bin/env bash
# The Android checks the build train runs (no GitHub Actions workflow; coding
# sessions do not run this). Usage:
#   scripts/android-check.sh            run every check
#   scripts/android-check.sh --dry-run  print the commands only
# Needs JDK 17 (JAVA_HOME) and the Android SDK (ANDROID_HOME or local.properties).
# The phonelink build needs app/libs/Mesh.aar (gitignored, built by
# infrastructure/mesh/tsnet-ios/build-android.sh); without it those steps are skipped.
set -euo pipefail

cd "$(dirname "$0")/.."

dry_run=0
case "${1:-}" in
  --dry-run) dry_run=1 ;;
  "") ;;
  *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
esac

core_modules=(design api auth stream data)

run() {
  echo "+ $*"
  if [[ $dry_run -eq 0 ]]; then "$@"; fi
}

# 0. Design tokens must match the iOS source (no Gradle; Node 20+).
run node ../../../contracts/design-tokens/generate.mjs --check
# 0b. The generated API client must match the OpenAPI specs (contracts/mobile-api).
run node ../../../contracts/mobile-api/generate.mjs --check

# 1. Google Play build (the TWA app as shipped) + its unit tests + lint.
run ./gradlew :app:assemblePlayDebug :app:testPlayDebugUnitTest :app:lintPlayDebug

# 1b. Native app (step 7b): the nativeapp flavor (versionCode 1000+, Play internal track only).
run ./gradlew :app:assembleNativeappDebug :app:assembleNativeappRelease :app:lintNativeappDebug

# 2. Native foundation modules: unit tests (:core:api, :core:stream and :core:design have tests today;
#    the others still compile through the task).
core_tasks=()
for m in "${core_modules[@]}"; do core_tasks+=(":core:$m:testDebugUnitTest"); done
run ./gradlew "${core_tasks[@]}"

# 3. Sideload build (Phone Link), only when the mesh library is present.
if [[ -f app/libs/Mesh.aar ]]; then
  run ./gradlew :app:assemblePhonelinkDebug :app:testPhonelinkDebugUnitTest
else
  echo "skip: phonelink (app/libs/Mesh.aar missing; build it with infrastructure/mesh/tsnet-ios/build-android.sh)"
fi

# 4. Play Billing must never reach the sideload build.
if [[ $dry_run -eq 1 ]]; then
  echo "+ ./gradlew -q :app:dependencies --configuration phonelinkReleaseRuntimeClasspath | grep -E 'androidbrowserhelper:billing|com\\.android\\.billingclient'  (must match nothing)"
else
  if ./gradlew -q :app:dependencies --configuration phonelinkReleaseRuntimeClasspath \
      | grep -E 'androidbrowserhelper:billing|com\.android\.billingclient'; then
    echo "FAIL: Play Billing is on the phonelink runtime classpath" >&2
    exit 1
  fi
  echo "ok: no Play Billing in phonelink"
fi

# 5. The native app's SDKs (Clerk, Compose) must never reach the TWA builds (cutover rule,
#    handoffs/android-native-2026-11-18/PLAN.md section 3).
for cfg in playReleaseRuntimeClasspath phonelinkReleaseRuntimeClasspath; do
  if [[ $dry_run -eq 1 ]]; then
    echo "+ ./gradlew -q :app:dependencies --configuration $cfg | grep -E 'com\\.clerk|androidx\\.compose'  (must match nothing)"
  elif ./gradlew -q :app:dependencies --configuration "$cfg" | grep -E 'com\.clerk|androidx\.compose'; then
    echo "FAIL: native app dependencies on $cfg" >&2
    exit 1
  else
    echo "ok: no Clerk/Compose in $cfg"
  fi
done
