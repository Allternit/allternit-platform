import Foundation

/// An engine a voice conversation can run on. Allternit hosts no voice
/// (Eoj, 2026-11-04): every engine here runs on the phone.
enum VoiceRoute: Equatable, Sendable {
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
        case .automatic, .device: return [.device, .legacy]
        }
    }

    /// User-facing line when the on-device engine can't start and the
    /// system voice takes over. Points to Settings › Voice, never to minutes.
    static func fallbackNotice(for error: Error) -> String {
        "\(error.localizedDescription) Using the iPhone's built-in voice for now; Settings › Voice can download on-device voice."
    }
}
