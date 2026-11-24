import Foundation

/// An engine a voice conversation can run on. Allternit hosts no voice
/// (Eoj, 2026-11-04): engines run on the phone, or on the user's own
/// OpenAI / ElevenLabs account with their key (`ByokVoiceTransport`).
enum VoiceRoute: Equatable, Sendable {
    /// In-process engine: Silero VAD + Moonshine on this phone.
    case device
    /// The pre-session path: Apple's on-device recognizer + system voice,
    /// push-to-talk and the last-resort fallback (no pack, no network).
    case legacy
    /// The user's own OpenAI account: Realtime listening + OpenAI speech.
    case openai
    /// The user's own ElevenLabs account reads replies; listening on this phone.
    case elevenlabs
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
        // A missing/bad key or no credit falls back to this phone, with a notice.
        case .openai: return [.openai, .device, .legacy]
        case .elevenlabs: return [.elevenlabs, .device, .legacy]
        }
    }

    /// User-facing line when the on-device engine can't start and the
    /// system voice takes over. Points to Settings › Voice, never to minutes.
    static func fallbackNotice(for error: Error, from route: VoiceRoute = .device) -> String {
        switch route {
        case .openai, .elevenlabs:
            return "\(error.localizedDescription) Using voice on this phone for now."
        case .device, .legacy:
            return "\(error.localizedDescription) Using the iPhone's built-in voice for now; Settings › Voice can download on-device voice."
        }
    }
}
