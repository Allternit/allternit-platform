import Foundation

/// An engine a voice conversation can run on.
enum VoiceRoute: Equatable, Sendable {
    /// Allternit Cloud Voice (WebSocket, ticket from cloud-api).
    case cloud
    /// In-process engine: Silero VAD + Moonshine on this phone.
    case device
    /// The pre-session path: Apple's on-device recognizer + system voice,
    /// push-to-talk and the last-resort fallback (no pack, no network).
    case legacy
}

/// Decides which engines to try, in order, for a "Where voice runs"
/// preference. Pure, so the policy is unit-tested.
enum VoiceRouter {
    static func candidates(preference: VoiceRoutePreference, pushToTalk: Bool) -> [VoiceRoute] {
        // Push-to-talk gates recognition on a hold gesture the session
        // engines (continuous VAD) don't have — it stays on the system path.
        if pushToTalk { return [.legacy] }
        switch preference {
        case .automatic: return [.cloud, .device, .legacy]
        case .device: return [.device, .legacy]
        // Cloud is an explicit choice: failing loudly beats silently
        // sending the user's audio somewhere else, or nowhere.
        case .cloud: return [.cloud]
        }
    }

    /// User-facing line for why Cloud Voice was skipped in Automatic.
    static func fallbackNotice(for error: Error) -> String {
        switch error {
        case VoiceCloudError.noCloudMinutes: return "No Cloud Voice minutes left — using this device."
        case VoiceCloudError.signedOut: return "Signed out — using this device."
        default: return "Cloud Voice isn't available — using this device."
        }
    }
}
