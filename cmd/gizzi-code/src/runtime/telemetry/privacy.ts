import { redactTelemetryString } from "../../shared/utils/telemetryRedact"
import { isGizziTelemetryEnvOff } from "../../shared/utils/privacyLevel"
import { isTelemetryDisabledInSettings } from "../../shared/utils/telemetrySettings"
import { isSelfHosted } from "../../shared/selfhost/offline"

export function cleanTelemetryString(value: string) {
  return redactTelemetryString(value)
}

export function telemetryDisabled() {
  const value = (name: string) => ["1", "true", "yes"].includes((process.env[name] ?? "").toLowerCase())
  return (
    process.env.NODE_ENV === "test" ||
    isSelfHosted() ||
    value("DO_NOT_TRACK") ||
    value("DISABLE_TELEMETRY") ||
    value("GIZZI_DISABLE_TELEMETRY") ||
    value("GIZZI_DISABLE_NONESSENTIAL_TRAFFIC") ||
    isGizziTelemetryEnvOff() ||
    isTelemetryDisabledInSettings()
  )
}

