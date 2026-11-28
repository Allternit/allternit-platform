/**
 * Ed25519 public key that verifies the Gizzi release feed
 * (https://runtime.allternit.com/gizzi/<channel>/latest.json + .sig).
 *
 * This key is for Gizzi only, on purpose: it is NOT the Desktop runtime key
 * (surfaces/allternit-desktop/src/main/runtime-package.ts RUNTIME_PUBLIC_KEY),
 * so one leaked private half never covers both products.
 *
 * Private half: GitHub secret GIZZI_FEED_SIGNING_KEY (Allternit/allternit-platform)
 * and Keychain service `allternit-gizzi-feed-signing-key`. The release workflow
 * signs the exact bytes of latest.json and publishes the base64 signature.
 *
 * Tests (and a staging feed) override it with env GIZZI_FEED_PUBLIC_KEY.
 * If the active key is empty or still the placeholder marker below, the
 * updater does nothing and reports "feed key not set" (fail closed).
 */
export const GIZZI_FEED_PUBLIC_KEY = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEALAlANLp9v9/gNgi58OKl5N2X2MfeSD4cRGs+ONKmUYQ=
-----END PUBLIC KEY-----`

/** Marker a key must never contain to be used. A key holding it counts as "not set". */
export const GIZZI_FEED_KEY_PLACEHOLDER = "GIZZI_FEED_KEY_PLACEHOLDER"
