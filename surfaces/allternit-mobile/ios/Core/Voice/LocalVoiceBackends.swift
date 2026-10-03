import AVFoundation
import Foundation

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

/// Replies on "This device" are read in the system voice
/// (AVSpeechSynthesizer): sherpa's Kokoro for iOS needs espeak-ng (GPL-3),
/// which an App Store build can't carry, so the neural voices are Cloud
/// Voice only. Audio is captured with `write(_:toBufferCallback:)` and played
/// through the session's own engine so the OS echo canceller hears it
/// (barge-in works on the speaker).
final class SystemSpeechSynthesizer: LocalSynthesizer, @unchecked Sendable {
    let outputSampleRate = 24_000
    private let synthesizer = AVSpeechSynthesizer()
    private let lock = NSLock()
    private var cancelled = false
    private var resume: CheckedContinuation<Void, Never>?

    func synthesize(_ text: String, voice: String?, onChunk: @escaping @Sendable (Data) -> Void) async {
        lock.lock(); cancelled = false; lock.unlock()
        let utterance = AVSpeechUtterance(string: text)
        if let voice, let v = AVSpeechSynthesisVoice(identifier: voice) { utterance.voice = v }
        else { utterance.voice = AVSpeechSynthesisVoice(language: Locale.current.identifier) ?? AVSpeechSynthesisVoice(language: "en-US") }
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate
        let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(outputSampleRate),
                                   channels: 1, interleaved: false)!
        nonisolated(unsafe) var converter: AVAudioConverter?

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.lock(); resume = continuation; lock.unlock()
            DispatchQueue.main.async { [self] in
                synthesizer.write(utterance) { [weak self] buffer in
                    guard let self, let pcm = buffer as? AVAudioPCMBuffer else { return }
                    // An empty buffer marks the end of the utterance.
                    if pcm.frameLength == 0 { self.complete(); return }
                    self.lock.lock(); let stop = self.cancelled; self.lock.unlock()
                    if stop { return }
                    if converter == nil || converter?.inputFormat != pcm.format {
                        converter = AVAudioConverter(from: pcm.format, to: target)
                    }
                    guard let converter else { return }
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
    static func make(paths: VoicePackPaths) -> LocalVoiceEngine {
        LocalVoiceEngine(
            detector: SherpaSpeechDetector(modelPath: paths.vad),
            recognizer: SherpaMoonshineRecognizer(paths: paths),
            synthesizer: SystemSpeechSynthesizer())
    }
}
