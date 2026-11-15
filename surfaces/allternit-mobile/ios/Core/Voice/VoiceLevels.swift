import AVFoundation
import Foundation

/// How loud voice is right now, 0–1, for live mode's speaker look: the mic
/// while you talk, the bot's spoken audio while it speaks.
///
/// Fed from the audio taps that already exist (dictation's mic tap, the voice
/// session's mic tap and its playback mix). It opens no audio of its own.
/// The system speech voice (`SpeechSpeaker`) gives no audio to measure, so
/// while it speaks `outputLevel()` stays 0 and the view follows the speaking
/// state instead. Thread-safe: written on audio threads, read every frame.
final class VoiceLevels: @unchecked Sendable {
    static let shared = VoiceLevels()

    private let lock = NSLock()
    private var mic: Double = 0
    private var micAt: CFTimeInterval = 0
    private var out: Double = 0
    private var outAt: CFTimeInterval = 0

    /// A level older than this has gone quiet (the tap stopped).
    private static let stale: CFTimeInterval = 0.25

    func pushMic(_ buffer: AVAudioPCMBuffer) {
        let v = Self.level(of: buffer)
        lock.lock(); mic = v; micAt = CACurrentMediaTime(); lock.unlock()
    }

    func pushOutput(_ buffer: AVAudioPCMBuffer) {
        let v = Self.level(of: buffer)
        lock.lock(); out = v; outAt = CACurrentMediaTime(); lock.unlock()
    }

    func micLevel() -> Double {
        lock.lock(); defer { lock.unlock() }
        return CACurrentMediaTime() - micAt > Self.stale ? 0 : mic
    }

    func outputLevel() -> Double {
        lock.lock(); defer { lock.unlock() }
        return CACurrentMediaTime() - outAt > Self.stale ? 0 : out
    }

    /// RMS of the first channel, mapped from about −50…−10 dBFS onto 0…1.
    static func level(of buffer: AVAudioPCMBuffer) -> Double {
        guard let channel = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return 0 }
        let n = Int(buffer.frameLength)
        var sum: Float = 0
        for i in 0..<n { sum += channel[i] * channel[i] }
        let rms = sqrt(sum / Float(n))
        guard rms > 0 else { return 0 }
        let db = 20 * log10(Double(rms))
        return min(1, max(0, (db + 50) / 40))
    }
}
