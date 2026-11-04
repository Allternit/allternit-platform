import Foundation
import KokoroSwift
import MisakiSwift
import MLX
import os
import UIKit

/// The Kokoro reply-voice pack on our mirror (R2 `allternit-runtime`,
/// `voice-models/ios/v1/…`, built by `scripts/build-ios-kokoro-pack.sh`):
/// Kokoro-82M (Apache-2.0) as fp16 safetensors, the 28 stock voices, and
/// MisakiSwift's English G2P data (gold/silver lexicons + BART fallback,
/// Apache-2.0). No espeak-ng: nothing GPL ships in the app.
enum KokoroPackSpec {
    static let defaultBase = "https://runtime.allternit.com/voice-models/ios/v1"
    static let model = "kokoro/kokoro-v1_0-fp16.safetensors"
    static let voices = "kokoro/voices-v1_0-fp16.safetensors"
    static let misakiDir = "misaki"

    static let files: [VoicePackFile] = [
        .init(asset: model, sha256: "67d4b879ce0f0bc6d30169cca8dc424f192cbed5be811078d8ef7c4a34a34b1f",
              size: 163_587_908, installPath: model),
        .init(asset: voices, sha256: "e9c828fec6c3f1b2637a0f12ad5e6755d05c28d1091da7c132dac5b15abae332",
              size: 7_313_600, installPath: voices),
        misaki("us_gold.json", "8507f89840f0813b10cf584740942f58e9cc9ad3660e24088b442ab0a6b126be", 3_001_196),
        misaki("us_silver.json", "ea0e1abca0c9b18fb0d3402034633a337154a3153e9a9f49f97d668c908e140c", 3_105_352),
        misaki("us_bart.safetensors", "dc4a02e62d4fcb4bb4097ecf00db89b8e1a12a549a52ab6adfbba220b80a55c5", 3_011_692),
        misaki("us_bart_config.json", "8deb3537fb29c63cd9f20d75515ae06e4c92f1b6db0703a2d45bca95b33a53a4", 1_257),
        misaki("gb_gold.json", "090cdecbe6bda9516a2cdc21563d62def0b91f5b55ea74fe6ef729eef682785f", 2_841_062),
        misaki("gb_silver.json", "cb8655f5e614e6306146e0da1bf93522ac542ebd39dcd9892afcf51c9aa52e1a", 3_677_750),
        misaki("gb_bart.safetensors", "4994f474bb6f4584076a4e98189caaec11aa3f773a0d82d5bc263a03dd07e703", 3_011_692),
        misaki("gb_bart_config.json", "e4f248e6af0cfb6cb54aea6ff0168d16b6f3cdabed438c487cde4ab0815d1bac", 1_246),
    ]

    private static func misaki(_ name: String, _ sha: String, _ size: Int64) -> VoicePackFile {
        .init(asset: "\(misakiDir)/\(name)", sha256: sha, size: size, installPath: "\(misakiDir)/\(name)")
    }
}

/// Kokoro's stock English voices (same ids as the web and the bot API).
enum KokoroVoices {
    static let defaultId = "af_heart"
    /// UserDefaults key for Settings › Voice › Reply voice (this phone).
    static let settingKey = "allternit-settings-voice-kokoro"
    static let all = [
        "af_heart", "af_alloy", "af_aoede", "af_bella", "af_jessica", "af_kore", "af_nicole", "af_nova",
        "af_river", "af_sarah", "af_sky", "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam",
        "am_michael", "am_onyx", "am_puck", "am_santa", "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
        "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
    ]

    /// The bot's own Kokoro voice, else this phone's pick, else af_heart.
    static func resolve(bot: String?, setting: String? = UserDefaults.standard.string(forKey: settingKey)) -> String {
        for id in [bot, setting] { if let id, all.contains(id) { return id } }
        return defaultId
    }

    /// "Heart · American" for a settings row.
    static func label(_ id: String) -> String {
        let name = id.split(separator: "_").last.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? id
        return "\(name) · \(id.hasPrefix("b") ? "British" : "American")"
    }

    /// Kokoro runs where MLX does: iOS 18+, a real device (MLX has no
    /// simulator GPU).
    static var deviceSupported: Bool {
        #if targetEnvironment(simulator)
        return false
        #else
        if #available(iOS 18.0, *) { return true }
        return false
        #endif
    }
}

private let kokoroLog = Logger(subsystem: "com.allternit.app", category: "voice-tts")

/// Process-wide Kokoro engine: the 327 MB of weights load once and are
/// dropped on a memory warning. Every MLX call runs on one serial queue
/// (KokoroTTS isn't thread-safe) inside `withError`, so an MLX failure
/// throws instead of trapping.
private final class KokoroRuntime: @unchecked Sendable {
    static let shared = KokoroRuntime()
    let queue = DispatchQueue(label: "com.allternit.voice.kokoro", qos: .userInitiated)

    private var tts: KokoroTTS?
    private var voices: [String: MLXArray] = [:]
    /// Set after a load or MLX failure: the system voice reads replies
    /// until the app restarts.
    private(set) var failed = false
    private var loggedFallback = false
    /// Read from the serial queue; flipped by app lifecycle notifications
    /// (iOS refuses GPU work from the background).
    private let stateLock = NSLock()
    private var backgrounded = false

    private init() {
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: nil) { [weak self] _ in
            self?.queue.async { self?.unload() }
        }
        center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: nil) { [weak self] _ in
            self?.stateLock.withLock { self?.backgrounded = true }
        }
        center.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: nil) { [weak self] _ in
            self?.stateLock.withLock { self?.backgrounded = false }
        }
    }

    var isBackgrounded: Bool { stateLock.withLock { backgrounded } }

    /// One log line the first time replies fall back to the system voice.
    func logFallback(_ reason: String) {
        guard !loggedFallback else { return }
        loggedFallback = true
        kokoroLog.info("Kokoro unavailable, using the system voice: \(reason, privacy: .public)")
    }

    private func unload() {
        tts = nil
        voices = [:]
        MLX.Memory.clearCache()
    }

    /// Synthesizes one piece of text at 24 kHz. Queue-only.
    func generate(_ text: String, voice: String, speed: Float, root: URL) throws -> [Float] {
        if tts == nil {
            // Weights (fp32 in memory) + activations + the lexicons.
            let available = os_proc_available_memory()
            guard available == 0 || available > 900 << 20 else { throw KokoroError.memory(available) }
            MisakiResources.directory = root.appendingPathComponent(KokoroPackSpec.misakiDir)
            MLX.Memory.cacheLimit = 64 << 20
            let modelURL = root.appendingPathComponent(KokoroPackSpec.model)
            let voicesURL = root.appendingPathComponent(KokoroPackSpec.voices)
            try withError {
                voices = try MLX.loadArrays(url: voicesURL).mapValues { $0.asType(.float32) }
                tts = KokoroTTS(modelPath: modelURL, g2p: .misaki)
            }
        }
        guard let tts, let embedding = voices[voice] ?? voices[KokoroVoices.defaultId] else {
            throw KokoroError.voiceMissing(voice)
        }
        let language: Language = voice.hasPrefix("b") ? .enGB : .enUS
        return try withError {
            try tts.generateAudio(voice: embedding, language: language, text: text, speed: speed).0
        }
    }

    func markFailed(_ error: Error) {
        failed = true
        unload()
        logFallback(String(describing: error))
    }
}

enum KokoroError: Error {
    case memory(Int)
    case voiceMissing(String)
}

/// Replies in Kokoro's voice, sentence by sentence, as PCM16 at 24 kHz into
/// the voice session's audio path (so the ring's output level, echo
/// cancellation and barge-in work exactly as with the system voice). Any
/// sentence Kokoro can't speak (pack not downloaded yet, low memory, app in
/// the background, an MLX error) is read by `SystemSpeechSynthesizer`.
final class KokoroSpeechSynthesizer: LocalSynthesizer, @unchecked Sendable {
    let outputSampleRate = KokoroTTS.Constants.samplingRate
    private let fallback: LocalSynthesizer
    private let voiceId: String
    private let speed: Float
    private let root: URL
    private let lock = NSLock()
    private var generation = 0

    init(voice: String, speed: Double, root: URL, fallback: LocalSynthesizer) {
        self.voiceId = voice
        self.speed = Float(min(2, max(0.5, speed)))
        self.root = root
        self.fallback = fallback
    }

    func synthesize(_ text: String, voice: String?, onChunk: @escaping @Sendable (Data) -> Void) async {
        let runtime = KokoroRuntime.shared
        let myGeneration = lock.withLock { generation }
        let samples: [Float]? = await withCheckedContinuation { continuation in
            runtime.queue.async { [self] in
                guard !runtime.failed else { continuation.resume(returning: nil); return }
                guard VoicePackManager.isInstalled(.kokoro, at: root) else {
                    runtime.logFallback("pack not downloaded"); continuation.resume(returning: nil); return
                }
                guard !runtime.isBackgrounded else { continuation.resume(returning: nil); return }
                // Cancelled while waiting for the queue: skip the work.
                guard lock.withLock({ generation }) == myGeneration else { continuation.resume(returning: []); return }
                do {
                    var out: [Float] = []
                    for piece in Self.pieces(text) {
                        guard lock.withLock({ generation }) == myGeneration else { break }
                        out += try runtime.generate(piece, voice: voiceId, speed: speed, root: root)
                    }
                    continuation.resume(returning: out)
                } catch {
                    runtime.markFailed(error)
                    continuation.resume(returning: nil)
                }
            }
        }
        guard let samples else {
            await fallback.synthesize(text, voice: voice, onChunk: onChunk)
            return
        }
        // 200 ms chunks, so a barge-in drops the rest of the sentence promptly.
        let step = outputSampleRate / 5
        var start = 0
        while start < samples.count {
            guard lock.withLock({ generation }) == myGeneration else { return }
            let end = min(samples.count, start + step)
            onChunk(VoicePCM.encode(Array(samples[start..<end])))
            start = end
        }
    }

    func cancel() {
        lock.withLock { generation += 1 }
        fallback.cancel()
    }

    /// Kokoro takes at most 510 phonemes per call: long sentences are cut at
    /// word boundaries into pieces of about 300 characters.
    static func pieces(_ text: String, limit: Int = 300) -> [String] {
        guard text.count > limit else { return [text] }
        var pieces: [String] = []
        var current = ""
        for word in text.split(separator: " ") {
            if !current.isEmpty, current.count + word.count + 1 > limit {
                pieces.append(current)
                current = ""
            }
            current += current.isEmpty ? String(word) : " \(word)"
        }
        if !current.isEmpty { pieces.append(current) }
        return pieces
    }
}
