/**
 * Privacy level controls how much nonessential network traffic and telemetry
 * Gizzi generates.
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
 *   GIZZI_DISABLE_NONESSENTIAL_TRAFFIC  →  essential-traffic
 *   GIZZI_TELEMETRY=off|0|false|no      →  no-telemetry (canonical kill switch)
 *   GIZZI_DISABLE_TELEMETRY             →  no-telemetry
 *   DISABLE_TELEMETRY                   →  no-telemetry
 *   telemetry.json "enabled": false     →  no-telemetry (persistent settings flag)
 */

import { isSelfHosted } from '../selfhost/offline.js'

import { isTelemetryDisabledInSettings } from './telemetrySettings.js'

type PrivacyLevel = 'default' | 'no-telemetry' | 'essential-traffic'

const TELEMETRY_OFF_VALUES = new Set(['off', '0', 'false', 'no', 'disabled'])

export function isGizziTelemetryEnvOff(): boolean {
  const raw = process.env.GIZZI_TELEMETRY?.trim().toLowerCase()
  return raw !== undefined && raw !== '' && TELEMETRY_OFF_VALUES.has(raw)
}

export function getPrivacyLevel(): PrivacyLevel {
  // Self-hosted mode (ALLTERNIT_SELF_HOSTED=1) makes no Allternit calls at all.
  if (isSelfHosted() || process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC) {
    return 'essential-traffic'
  }
  if (
    isGizziTelemetryEnvOff() ||
    process.env.GIZZI_DISABLE_TELEMETRY ||
    process.env.DISABLE_TELEMETRY ||
    isTelemetryDisabledInSettings()
  ) {
    return 'no-telemetry'
  }
  return 'default'
}

/**
 * True when all nonessential network traffic should be suppressed.
 * Equivalent to the old `process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC` check.
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
  if (process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC) {
    return 'GIZZI_DISABLE_NONESSENTIAL_TRAFFIC'
  }
  return null
}
