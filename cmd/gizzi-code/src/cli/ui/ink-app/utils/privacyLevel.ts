/**
 * Privacy level controls how much nonessential network traffic and telemetry
 * gizzi-code generates.
 *
 * Levels are ordered by restrictiveness:
 *   default < no-telemetry < essential-traffic
 *
 * - default:            Everything enabled.
 * - no-telemetry:       Analytics/telemetry disabled (Datadog, 1P events, feedback survey).
 * - essential-traffic:  ALL nonessential network traffic disabled
 *                       (telemetry + auto-updates, grove, release notes, model capabilities, etc.).
 *
 * The resolved level is the most restrictive signal from:
 *   GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC  →  essential-traffic
 *   DISABLE_TELEMETRY                         →  no-telemetry
 */

import { isSelfHosted } from '../../../../shared/selfhost/offline.js'

type PrivacyLevel = 'default' | 'no-telemetry' | 'essential-traffic'

export function getPrivacyLevel(): PrivacyLevel {
  // Self-hosted mode (ALLTERNIT_SELF_HOSTED=1) makes no Allternit calls at all.
  if (isSelfHosted() || process.env.GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC) {
    return 'essential-traffic'
  }
  if (process.env.DISABLE_TELEMETRY) {
    return 'no-telemetry'
  }
  return 'default'
}

/**
 * True when all nonessential network traffic should be suppressed.
 * Equivalent to the old `process.env.GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC` check.
 */
export function isEssentialTrafficOnly(): boolean {
  return getPrivacyLevel() === 'essential-traffic'
}

/**
 * True when telemetry/analytics should be suppressed.
 * True at both `no-telemetry` and `essential-traffic` levels.
 */
export function isTelemetryDisabled(): boolean {
  return getPrivacyLevel() !== 'default'
}

/**
 * Returns the env var name responsible for the current essential-traffic restriction,
 * or null if unrestricted. Used for user-facing "unset X to re-enable" messages.
 */
export function getEssentialTrafficOnlyReason(): string | null {
  if (isSelfHosted()) {
    return 'ALLTERNIT_SELF_HOSTED'
  }
  if (process.env.GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC) {
    return 'GIZZI_CODE_DISABLE_NONESSENTIAL_TRAFFIC'
  }
  return null
}
