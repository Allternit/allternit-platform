import Foundation

/// Voice activity detector over 16 kHz mono floats (Silero via sherpa-onnx
/// in production, a scripted fake in tests).
protocol LocalSpeechDetector: AnyObject {
    /// Feeds exactly `LocalVoiceEngine.windowSamples` samples.
    func accept(_ window: [Float])
    var isSpeechDetected: Bool { get }
    /// Completed speech segments since the last call.
    func takeSegments() -> [[Float]]
    func reset()
}

protocol LocalRecognizer: AnyObject {
    func transcribe(_ samples: [Float]) -> String
}

/// Speech synthesis for replies. Produces PCM16 mono chunks at
/// `outputSampleRate` via `onChunk` (called from any thread) and returns when
/// the sentence is done or cancelled.
protocol LocalSynthesizer: AnyObject, Sendable {
    var outputSampleRate: Int { get }
    func synthesize(_ text: String, voice: String?, onChunk: @escaping @Sendable (Data) -> Void) async
    func cancel()
}

/// The Voice Session protocol served in-process: Silero VAD + Moonshine STT
/// on this phone, so a conversation never leaves the device on the input
/// side. It is a `VoiceTransport`, so `VoiceSessionClient` drives it exactly
/// like the cloud socket.
///
/// Turn detection is `vad` mode: a turn ends `silenceMs` after the last
/// speech (the first 300 ms of that is the VAD's own min-silence). No Smart
/// Turn on iOS. Barge-in: speech detected while a reply is playing emits
/// `speak.interrupted` and stops synthesis.
final class LocalVoiceEngine: VoiceTransport, @unchecked Sendable {
    static let windowSamples = 512
    static let sampleRate = 16_000
    /// What the VAD's own `min_silence_duration` already waited.
    static let vadSilenceMs = 300

    private let detector: LocalSpeechDetector
    private let recognizer: LocalRecognizer
    private let synthesizer: LocalSynthesizer
    private let queue = DispatchQueue(label: "com.allternit.voice.local-engine")
    private let speakLock = NSLock()

    private var onEvent: (@Sendable (VoiceTransportEvent) -> Void)?
    private var started = false
    private var options = VoiceSessionOptions()
    private var silenceMs = 700
    private var muted = false
    private var pending: [Float] = []
    private var micSamples = 0
    private var inSpeech = false
    private var speechOnsetSample = 0
    private var turnTexts: [String] = []
    private var lastSegmentEndSample = 0
    private var segmentCounter = 0

    // Speech output (guarded by speakLock).
    private enum SpeakItem { case sentence(id: String, text: String), end(id: String) }
    private var chunkers: [String: SpeechSentenceChunker] = [:]
    private var speakQueue: [SpeakItem] = []
    private var liveIds: [String] = []
    private var startedIds: Set<String> = []
    private var sentSamples: [String: Int] = [:]
    private var epoch = 0
    private var workerRunning = false

    init(detector: LocalSpeechDetector, recognizer: LocalRecognizer, synthesizer: LocalSynthesizer) {
        self.detector = detector
        self.recognizer = recognizer
        self.synthesizer = synthesizer
    }

    // MARK: - VoiceTransport

    func connect(onEvent: @escaping @Sendable (VoiceTransportEvent) -> Void) async throws {
        queue.sync { self.onEvent = onEvent }
    }

    func send(_ message: VoiceClientMessage) {
        queue.async { self.handle(message) }
    }

    func sendAudio(_ data: Data) {
        queue.async { self.ingest(VoicePCM.decode(data)) }
    }

    func close() {
        queue.async {
            self.started = false
            self.onEvent = nil
        }
        cancelSpeech(emitInterrupted: false)
        synthesizer.cancel()
    }

    // MARK: - Control messages

    private func handle(_ message: VoiceClientMessage) {
        switch message {
        case .start(let opts):
            options = opts
            applyTurn(opts)
            started = true
            detector.reset()
            emit(.event(.ready(.init(
                sessionId: UUID().uuidString, engine: .device,
                outputSampleRate: synthesizer.outputSampleRate, voices: [], protocolVersion: 1))))
        case .update(let opts):
            options = opts
            applyTurn(opts)
        case .micMute:
            muted = true
            if inSpeech {
                inSpeech = false
                emit(.event(.speechStopped(atMs: ms(micSamples))))
            }
            pending.removeAll()
            turnTexts.removeAll()
            detector.reset()
        case .micUnmute:
            muted = false
        case .speakDelta(let id, let text):
            enqueueText(id: id, text: text)
        case .speakDone(let id):
            enqueueDone(id: id)
        case .speakCancel(let id):
            cancelSpeech(id: id, emitInterrupted: false, emitEnded: true)
        case .end:
            started = false
        }
    }

    private func applyTurn(_ opts: VoiceSessionOptions) {
        silenceMs = 700
    }

    // MARK: - Mic → turns

    private func ingest(_ samples: [Float]) {
        guard started else { return }
        guard !muted else { micSamples += samples.count; return }
        pending.append(contentsOf: samples)
        while pending.count >= Self.windowSamples {
            let window = Array(pending.prefix(Self.windowSamples))
            pending.removeFirst(Self.windowSamples)
            micSamples += Self.windowSamples
            process(window: window)
        }
    }

    private func process(window: [Float]) {
        detector.accept(window)
        let speech = detector.isSpeechDetected
        if speech && !inSpeech {
            inSpeech = true
            speechOnsetSample = micSamples
            emit(.event(.speechStarted(atMs: ms(micSamples))))
        } else if !speech && inSpeech {
            inSpeech = false
            emit(.event(.speechStopped(atMs: ms(micSamples))))
        }
        // Barge-in: user speech while the bot is speaking.
        if inSpeech, options.bargeIn, micSamples - speechOnsetSample >= Self.sampleRate / 5, isSpeaking {
            interruptSpeech()
        }
        for segment in detector.takeSegments() {
            let text = recognizer.transcribe(segment).trimmingCharacters(in: .whitespacesAndNewlines)
            lastSegmentEndSample = micSamples
            guard !text.isEmpty else { continue }
            segmentCounter += 1
            turnTexts.append(text)
            emit(.event(.transcriptFinal(segmentId: "seg-\(segmentCounter)", text: text)))
        }
        if !turnTexts.isEmpty, !inSpeech,
           ms(micSamples - lastSegmentEndSample) >= max(0, silenceMs - Self.vadSilenceMs) {
            let text = turnTexts.joined(separator: " ")
            turnTexts.removeAll()
            emit(.event(.turnEnded(text: text, confidence: 1.0)))
        }
    }

    private func ms(_ samples: Int) -> Int { samples * 1000 / Self.sampleRate }

    private func emit(_ event: VoiceTransportEvent) {
        onEvent?(event)
    }

    // MARK: - Speech output

    private var isSpeaking: Bool {
        speakLock.lock(); defer { speakLock.unlock() }
        return !liveIds.isEmpty
    }

    private func enqueueText(id: String, text: String) {
        speakLock.lock()
        if !liveIds.contains(id) { liveIds.append(id); chunkers[id] = SpeechSentenceChunker() }
        var chunker = chunkers[id] ?? SpeechSentenceChunker()
        let sentences = chunker.push(text)
        chunkers[id] = chunker
        for sentence in sentences { speakQueue.append(.sentence(id: id, text: sentence)) }
        speakLock.unlock()
        startWorkerIfNeeded()
    }

    private func enqueueDone(id: String) {
        speakLock.lock()
        guard liveIds.contains(id) else { speakLock.unlock(); return }
        if var chunker = chunkers[id], let tail = chunker.flush() {
            speakQueue.append(.sentence(id: id, text: tail))
        }
        chunkers[id] = nil
        speakQueue.append(.end(id: id))
        speakLock.unlock()
        startWorkerIfNeeded()
    }

    private func startWorkerIfNeeded() {
        speakLock.lock()
        guard !workerRunning, !speakQueue.isEmpty else { speakLock.unlock(); return }
        workerRunning = true
        let myEpoch = epoch
        speakLock.unlock()
        Task.detached { [self] in await runWorker(epoch: myEpoch) }
    }

    private func runWorker(epoch myEpoch: Int) async {
        while true {
            speakLock.lock()
            guard epoch == myEpoch, !speakQueue.isEmpty else {
                // A newer epoch owns the worker flag (cancel restarts it).
                if epoch == myEpoch { workerRunning = false }
                speakLock.unlock()
                return
            }
            let item = speakQueue.removeFirst()
            speakLock.unlock()

            switch item {
            case .sentence(let id, let text):
                markStarted(id: id)
                let voice = options.voice
                await synthesizer.synthesize(text, voice: voice) { [weak self] data in
                    self?.deliver(data, id: id, epoch: myEpoch)
                }
            case .end(let id):
                markStarted(id: id)
                finish(id: id, epoch: myEpoch)
            }
        }
    }

    private func markStarted(id: String) {
        speakLock.lock()
        let first = startedIds.insert(id).inserted
        speakLock.unlock()
        if first { emit(.event(.speakStarted(id: id))) }
    }

    private func deliver(_ data: Data, id: String, epoch myEpoch: Int) {
        speakLock.lock()
        let live = epoch == myEpoch && liveIds.contains(id)
        if live { sentSamples[id, default: 0] += data.count / 2 }
        speakLock.unlock()
        if live { emit(.audio(data)) }
    }

    private func finish(id: String, epoch myEpoch: Int) {
        speakLock.lock()
        let live = epoch == myEpoch && liveIds.contains(id)
        if live {
            liveIds.removeAll { $0 == id }
            startedIds.remove(id)
            sentSamples[id] = nil
        }
        speakLock.unlock()
        if live { emit(.event(.speakEnded(id: id))) }
    }

    /// Barge-in: every live utterance gets `speak.interrupted`.
    private func interruptSpeech() {
        cancelSpeech(emitInterrupted: true)
    }

    private func cancelSpeech(id: String? = nil, emitInterrupted: Bool, emitEnded: Bool = false) {
        speakLock.lock()
        let targets = id.map { [$0] } ?? liveIds
        let sent = targets.map { ($0, sentSamples[$0] ?? 0) }
        epoch += 1
        workerRunning = false
        speakQueue.removeAll()
        for target in targets {
            liveIds.removeAll { $0 == target }
            chunkers[target] = nil
            startedIds.remove(target)
            sentSamples[target] = nil
        }
        speakLock.unlock()
        synthesizer.cancel()
        for (target, samples) in sent {
            if emitInterrupted {
                emit(.event(.speakInterrupted(id: target, sentMs: samples * 1000 / max(1, synthesizer.outputSampleRate))))
            } else if emitEnded {
                emit(.event(.speakEnded(id: target)))
            }
        }
    }
}
