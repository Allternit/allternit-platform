

/**
 * Kill-switch check for voice mode. Returns true unless the
 * `tengu_amber_quartz_disabled` GrowthBook flag is flipped on (emergency
 * off). Default `false` means a missing/stale disk cache reads as "not
 * killed" — so fresh installs get voice working immediately without
 * waiting for GrowthBook init. Use this for deciding whether voice mode
 * should be *visible* (e.g., command registration, config UI).
 */
export function isVoiceGrowthBookEnabled(): boolean {
  // Local (sherpa-onnx voice sidecar) dictation is always eligible. The Anthropic
  // GrowthBook kill-switch only applies to the gated voice_stream path.
  return true
}

/**
 * Auth-only check for voice mode. Returns true when the user has a valid
 * Anthropic OAuth token. Backed by the memoized getClaudeAIOAuthTokens —
 * first call spawns `security` on macOS (~20-50ms), subsequent calls are
 * cache hits. The memoize clears on token refresh (~once/hour), so one
 * cold spawn per refresh is expected. Cheap enough for usage-time checks.
 */
export function hasVoiceAuth(): boolean {
  // Local STT does not need Anthropic OAuth. Keep this true so the
  // /voice toggle is the only user-facing gate.
  return true
}

/**
 * Full runtime check: auth + GrowthBook kill-switch. Callers: `/voice`
 * (voice.ts, voice/index.ts), ConfigTool, VoiceModeNotice — command-time
 * paths where a fresh keychain read is acceptable. For React render
 * paths use useVoiceEnabled() instead (memoizes the auth half).
 */
export function isVoiceModeEnabled(): boolean {
  return hasVoiceAuth() && isVoiceGrowthBookEnabled()
}
