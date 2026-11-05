import AVFoundation
import Foundation
import os
import Speech

/// Silero VAD on sherpa-onnx 1.13.8 (same model + thresholds as the native
/// engine, `services/voice/src/stt.rs`).
final class SherpaSpeechDetector: LocalSpeechDetector {
    private let vad: SherpaOnnxVoiceActivityDetectorWrapper

    init(modelPath: String) {
        var config = sherpaOnnxVadModelConfig(
            sileroVad: sherpaOnnxSileroVadModelConfig(
                model: modelPath, threshold: 0.5, minSilenceDuration: 0.3,
                minSpeechDuration: 0.25, windowSize: LocalVoiceEngine.windowSamples, maxSpeechDuration: 20),
            sampleRate: 16_000, numThreads: 1)
        vad = SherpaOnnxVoiceActivityDetectorWrapper(config: &config, buffer_size_in_seconds: 30)
    }

    func accept(_ window: [Float]) { vad.acceptWaveform(samples: window) }
    var isSpeechDetected: Bool { vad.isSpeechDetected() }

    func takeSegments() -> [[Float]] {
        var segments: [[Float]] = []
        while !vad.isEmpty() {
            segments.append(vad.front().samples)
            vad.pop()
        }
        return segments
    }

    func reset() { vad.reset() }
}

/// Moonshine tiny EN (quantized, v2 export) on sherpa-onnx.
final class SherpaMoonshineRecognizer: LocalRecognizer {
    private let recognizer: SherpaOnnxOfflineRecognizer

    init(paths: VoicePackPaths) {
        let moonshine = sherpaOnnxOfflineMoonshineModelConfig(encoder: paths.encoder, mergedDecoder: paths.decoder)
        let model = sherpaOnnxOfflineModelConfig(tokens: paths.tokens, numThreads: 2, moonshine: moonshine)
        var config = sherpaOnnxOfflineRecognizerConfig(featConfig: sherpaOnnxFeatureConfig(), modelConfig: model)
        recognizer = SherpaOnnxOfflineRecognizer(config: &config)
    }

    func transcribe(_ samples: [Float]) -> String {
        recognizer.decode(samples: samples, sampleRate: LocalVoiceEngine.sampleRate).text
    }
}

/// The system voice (AVSpeechSynthesizer): replies on iOS 17, and any
/// sentence Kokoro (KokoroSpeech.swift, iOS 18+) can't speak. Audio is captured with `write(_:toBufferCallback:)` and played
/// through the session's own engine so the OS echo canceller hears it
/// (barge-in works on the speaker).
/// Per-utterance state handed to AVSpeechSynthesizer's callback queue; only
/// that callback touches `converter` after construction.
private final class SynthState: @unchecked Sendable {
    let utterance: AVSpeechUtterance
    let target: AVAudioFormat
    var converter: AVAudioConverter?
    init(utterance: AVSpeechUtterance, target: AVAudioFormat) { self.utterance = utterance; self.target = target }
}

final class SystemSpeechSynthesizer: LocalSynthesizer, @unchecked Sendable {
    let outputSampleRate = 24_000
    private let synthesizer = AVSpeechSynthesizer()
    private let lock = NSLock()
    private var cancelled = false
    private var resume: CheckedContinuation<Void, Never>?

    func synthesize(_ text: String, voice: String?, onChunk: @escaping @Sendable (Data) -> Void) async {
        lock.withLock { cancelled = false }
        let utterance = AVSpeechUtterance(string: text)
        if let voice, let v = AVSpeechSynthesisVoice(identifier: voice) { utterance.voice = v }
        else { utterance.voice = AVSpeechSynthesisVoice(language: Locale.current.identifier) ?? AVSpeechSynthesisVoice(language: "en-US") }
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate
        let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(outputSampleRate),
                                   channels: 1, interleaved: false)!
        let state = SynthState(utterance: utterance, target: target)

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.lock(); resume = continuation; lock.unlock()
            DispatchQueue.main.async { [self] in
                synthesizer.write(state.utterance) { [weak self] buffer in
                    guard let self, let pcm = buffer as? AVAudioPCMBuffer else { return }
                    // An empty buffer marks the end of the utterance.
                    if pcm.frameLength == 0 { self.complete(); return }
                    self.lock.lock(); let stop = self.cancelled; self.lock.unlock()
                    if stop { return }
                    if state.converter == nil || state.converter?.inputFormat != pcm.format {
                        state.converter = AVAudioConverter(from: pcm.format, to: state.target)
                    }
                    guard let converter = state.converter else { return }
                    let target = state.target
                    let capacity = AVAudioFrameCount(Double(pcm.frameLength) * target.sampleRate / pcm.format.sampleRate) + 16
                    guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return }
                    nonisolated(unsafe) var supplied = false
                    var error: NSError?
                    converter.convert(to: out, error: &error) { _, status in
                        if supplied { status.pointee = .noDataNow; return nil }
                        supplied = true; status.pointee = .haveData; return pcm
                    }
                    guard error == nil, out.frameLength > 0, let channel = out.floatChannelData?[0] else { return }
                    onChunk(VoicePCM.encode(Array(UnsafeBufferPointer(start: channel, count: Int(out.frameLength)))))
                }
            }
            // Safety net: never strand the speaker if the end marker is lost.
            DispatchQueue.global().asyncAfter(deadline: .now() + 45) { [weak self] in self?.complete() }
        }
    }

    func cancel() {
        lock.lock(); cancelled = true; lock.unlock()
        DispatchQueue.main.async { [synthesizer] in synthesizer.stopSpeaking(at: .immediate) }
        complete()
    }

    private func complete() {
        lock.lock()
        let continuation = resume
        resume = nil
        lock.unlock()
        continuation?.resume()
    }
}

/// Builds the on-device engine from an installed pack.
enum LocalVoiceEngineFactory {
    /// `kokoroVoice` is a Kokoro voice id (`KokoroVoices.resolve`); replies
    /// use it on iOS 18+ devices, the system voice elsewhere.
    static func make(paths: VoicePackPaths, listening: ListeningPreference = .automatic,
                     kokoroVoice: String = KokoroVoices.defaultId, speed: Double = 1) -> LocalVoiceEngine {
        let moonshine = { SherpaMoonshineRecognizer(paths: paths) as LocalRecognizer }
        let recognizer: LocalRecognizer = switch listening {
        case .automatic: SystemFirstRecognizer(native: AppleSpeech.makeTranscriber(), moonshine: moonshine)
        case .allternit: moonshine()
        }
        return LocalVoiceEngine(
            detector: SherpaSpeechDetector(modelPath: paths.vad),
            recognizer: recognizer,
            synthesizer: synthesizer(kokoroVoice: kokoroVoice, speed: speed))
    }

    static func synthesizer(kokoroVoice: String, speed: Double) -> LocalSynthesizer {
        guard KokoroVoices.deviceSupported else {
            Logger(subsystem: "com.allternit.app", category: "voice-tts")
                .info("Kokoro needs iOS 18 on a device; replies use the system voice")
            return SystemSpeechSynthesizer()
        }
        return KokoroSpeechSynthesizer(voice: kokoroVoice, speed: speed,
                                       root: VoicePackManager.defaultRoot(.kokoro), fallback: SystemSpeechSynthesizer())
    }
}

// MARK: - Listening on Apple's on-device recognizer

private let speechLog = Logger(subsystem: "com.allternit.app", category: "voice-stt")

/// Transcribes one finished VAD segment (16 kHz mono floats). Throws when the
/// system recognizer can't serve it, so the caller falls back to Moonshine.
protocol NativeSegmentTranscriber: AnyObject, Sendable {
    /// False while the model is still being prepared or permission is missing.
    var isReady: Bool { get }
    func transcribe(_ samples: [Float]) throws -> String
}

/// "Listening: Automatic": Apple's on-device recognizer when it can run on
/// this phone, Moonshine otherwise. Only the recognizer changes: Silero VAD,
/// turn-taking and barge-in stay in `LocalVoiceEngine`, and segments still
/// come from the echo-cancelled mic, so the bot's own voice isn't
/// transcribed. Any native failure drops to Moonshine for the rest of the
/// session, with one log line.
final class SystemFirstRecognizer: LocalRecognizer {
    private let native: NativeSegmentTranscriber?
    private let makeMoonshine: () -> LocalRecognizer
    private var moonshine: LocalRecognizer?
    private var nativeFailed = false
    private var loggedNotReady = false

    init(native: NativeSegmentTranscriber?, moonshine: @escaping () -> LocalRecognizer) {
        self.native = native
        self.makeMoonshine = moonshine
        if native == nil { speechLog.info("system recognizer unavailable; listening on Moonshine") }
    }

    func transcribe(_ samples: [Float]) -> String {
        if let native, !nativeFailed, native.isReady {
            do { return try native.transcribe(samples) } catch {
                nativeFailed = true
                speechLog.error("system recognizer failed, falling back to Moonshine: \(String(describing: error), privacy: .public)")
            }
        } else if native != nil, !nativeFailed, !loggedNotReady {
            loggedNotReady = true
            speechLog.info("system recognizer not ready (model or permission); using Moonshine for now")
        }
        if moonshine == nil { moonshine = makeMoonshine() }
        return moonshine?.transcribe(samples) ?? ""
    }
}

enum AppleSpeechError: Error { case format, timeout, notReady }

enum AppleSpeech {
    /// SpeechAnalyzer/SpeechTranscriber on iOS 26+, else SFSpeechRecognizer
    /// when it can recognize on-device for this locale, else nil (Moonshine).
    static func makeTranscriber() -> NativeSegmentTranscriber? {
        if #available(iOS 26.0, *), SpeechTranscriber.isAvailable { return AnalyzerSegmentTranscriber() }
        return LegacySegmentTranscriber()
    }

    static let engineFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false)!

    /// The segment as a PCM buffer in `format` (converted when it isn't 16 kHz float).
    static func buffer(_ samples: [Float], format: AVAudioFormat) -> AVAudioPCMBuffer? {
        guard !samples.isEmpty,
              let source = AVAudioPCMBuffer(pcmFormat: engineFormat, frameCapacity: AVAudioFrameCount(samples.count)),
              let channel = source.floatChannelData?[0] else { return nil }
        source.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: samples.count) }
        if format == engineFormat { return source }
        guard let converter = AVAudioConverter(from: engineFormat, to: format) else { return nil }
        let capacity = AVAudioFrameCount(Double(samples.count) * format.sampleRate / 16_000) + 16
        guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return nil }
        nonisolated(unsafe) var supplied = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if supplied { status.pointee = .endOfStream; return nil }
            supplied = true; status.pointee = .haveData; return source
        }
        return error == nil && out.frameLength > 0 ? out : nil
    }

    /// Runs async recognition from the engine's serial queue (never the main
    /// thread), with a timeout so a stuck recognizer can't stall the turn.
    static func blocking<T: Sendable>(timeout: TimeInterval = 10, _ work: @escaping @Sendable () async throws -> T) throws -> T {
        let box = OnceBox<T>()
        let task = Task.detached(priority: .userInitiated) {
            do { box.set(.success(try await work())) } catch { box.set(.failure(error)) }
        }
        guard let result = box.wait(timeout: timeout) else { task.cancel(); throw AppleSpeechError.timeout }
        return try result.get()
    }
}

/// First result wins; `wait` blocks until it arrives.
final class OnceBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private let done = DispatchSemaphore(value: 0)
    private var value: Result<T, Error>?

    func set(_ result: Result<T, Error>) {
        let first: Bool = lock.withLock {
            guard value == nil else { return false }
            value = result
            return true
        }
        if first { done.signal() }
    }

    func wait(timeout: TimeInterval) -> Result<T, Error>? {
        guard done.wait(timeout: .now() + timeout) == .success else { return nil }
        return lock.withLock { value }
    }
}

/// iOS 26+: SpeechAnalyzer + SpeechTranscriber, fully on-device. The locale's
/// model is an Apple system asset, installed once in the background; until it
/// is ready, segments go to Moonshine.
@available(iOS 26.0, *)
final class AnalyzerSegmentTranscriber: NativeSegmentTranscriber, @unchecked Sendable {
    private let lock = NSLock()
    private var locale: Locale?
    private var format: AVAudioFormat?

    init() {
        Task.detached(priority: .utility) { [weak self] in await self?.prepare() }
    }

    var isReady: Bool { lock.withLock { locale != nil && format != nil } }

    private func prepare() async {
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else {
            speechLog.info("SpeechTranscriber has no model for \(Locale.current.identifier, privacy: .public); listening on Moonshine")
            return
        }
        let probe = SpeechTranscriber(locale: locale, preset: .transcription)
        do {
            if let request = try await AssetInventory.assetInstallationRequest(supporting: [probe]) {
                try await request.downloadAndInstall()
            }
        } catch {
            speechLog.error("SpeechTranscriber model install failed: \(String(describing: error), privacy: .public)")
            return
        }
        let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [probe]) ?? AppleSpeech.engineFormat
        lock.withLock { self.locale = locale; self.format = format }
    }

    func transcribe(_ samples: [Float]) throws -> String {
        let (locale, format) = lock.withLock { (self.locale, self.format) }
        guard let locale, let format else { throw AppleSpeechError.notReady }
        guard let pcm = AppleSpeech.buffer(samples, format: format) else { throw AppleSpeechError.format }
        let input = AnalyzerInput(buffer: pcm)
        return try AppleSpeech.blocking {
            let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
            let analyzer = SpeechAnalyzer(modules: [transcriber],
                                          options: .init(priority: .userInitiated, modelRetention: .processLifetime))
            let collect = Task {
                var text = ""
                for try await result in transcriber.results { text += String(result.text.characters) }
                return text
            }
            let (stream, continuation) = AsyncStream.makeStream(of: AnalyzerInput.self)
            continuation.yield(input)
            continuation.finish()
            if let end = try await analyzer.analyzeSequence(stream) {
                try await analyzer.finalizeAndFinish(through: end)
            } else {
                await analyzer.cancelAndFinishNow()
            }
            return try await collect.value.trimmingCharacters(in: .whitespacesAndNewlines)
        }
    }
}

/// iOS 17–25: SFSpeechRecognizer with `requiresOnDeviceRecognition`, used only
/// when the locale supports on-device recognition (audio never leaves the phone).
final class LegacySegmentTranscriber: NativeSegmentTranscriber, @unchecked Sendable {
    private let recognizer: SFSpeechRecognizer
    private let callbacks = OperationQueue()

    init?() {
        guard let recognizer = SFSpeechRecognizer(locale: Locale.current) ?? SFSpeechRecognizer(locale: Locale(identifier: "en-US")),
              recognizer.supportsOnDeviceRecognition else { return nil }
        self.recognizer = recognizer
        recognizer.queue = callbacks
        if SFSpeechRecognizer.authorizationStatus() == .notDetermined {
            SFSpeechRecognizer.requestAuthorization { _ in }
        }
    }

    var isReady: Bool {
        SFSpeechRecognizer.authorizationStatus() == .authorized && recognizer.isAvailable
    }

    func transcribe(_ samples: [Float]) throws -> String {
        guard let pcm = AppleSpeech.buffer(samples, format: AppleSpeech.engineFormat) else { throw AppleSpeechError.format }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false
        request.append(pcm)
        request.endAudio()
        let box = OnceBox<String>()
        let task = recognizer.recognitionTask(with: request) { result, error in
            if let result, result.isFinal {
                box.set(.success(result.bestTranscription.formattedString))
            } else if let error {
                // 1110 = no speech in the segment: an empty turn, not a failure.
                let nsError = error as NSError
                box.set(nsError.code == 1110 ? .success("") : .failure(error))
            }
        }
        guard let result = box.wait(timeout: 10) else { task.cancel(); throw AppleSpeechError.timeout }
        return try result.get()
    }
}
