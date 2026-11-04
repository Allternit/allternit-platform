import Foundation

// Voice with the user's own provider account (Settings › Voice), behind the
// same Voice Session protocol as the on-device engine. Mirrors the web's
// `src/lib/voice/byok/` (allternit-ai #976):
//
// - Your own OpenAI: listening through the OpenAI Realtime API (a
//   transcription session over WebSocket, authorised by a short-lived client
//   secret minted with the user's key); replies through OpenAI text-to-speech
//   (`/v1/audio/speech`, gpt-4o-mini-tts, raw 24 kHz PCM, streamed).
// - Your own ElevenLabs: listening stays on this phone (`LocalVoiceEngine`);
//   replies through ElevenLabs streaming TTS (eleven_flash_v2_5, pcm_24000).
//
// Speech output reuses `LocalVoiceEngine`'s speak.* machinery with a vendor
// synthesizer, so captions, transcript, barge-in and the speaker ring (PCM
// played through `VoiceAudioIO`) are shared code. Calls go from this phone
// straight to the vendor; nothing passes through Allternit.

// MARK: - Keys

/// The user's own voice keys, in the Keychain on this phone. Sent only to the
/// vendor (api.openai.com, api.elevenlabs.io), never to Allternit.
enum VoiceProviderKeys {
    enum Field: String, CaseIterable, Sendable {
        case openai, elevenlabs, elevenlabsVoiceId
    }

    /// "Rachel", an ElevenLabs default voice every account has.
    static let defaultElevenLabsVoice = "21m00Tcm4TlvDq8ikWAM"

    private static func account(_ field: Field) -> String { "voice.byok.\(field.rawValue)" }

    static func value(_ field: Field) -> String? {
        let value = KeychainHelper.load(account(field))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return value.isEmpty ? nil : value
    }

    /// Saves the trimmed value; an empty one removes it.
    static func set(_ field: Field, _ value: String?) {
        let value = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if value.isEmpty { KeychainHelper.delete(account(field)) } else { KeychainHelper.save(value, for: account(field)) }
    }

    /// "sk-…abcd": enough to recognise a saved key without showing it.
    static func masked(_ key: String) -> String {
        "\(key.hasPrefix("sk-") ? "sk-" : "")…\(key.suffix(4))"
    }
}

// MARK: - Vendor errors

struct VoiceVendorError: Error, LocalizedError, Sendable {
    let message: String
    let fatal: Bool
    var errorDescription: String? { message }

    static func status(_ vendor: String, _ status: Int) -> VoiceVendorError {
        switch status {
        case 401, 403:
            return .init(message: "\(vendor) did not accept your API key. Check it in Settings › Voice.", fatal: true)
        case 402, 429:
            return .init(message: "Your \(vendor) account is out of credit or over its rate limit. Check your \(vendor) account, or choose another option in Settings › Voice.", fatal: true)
        default:
            return .init(message: "\(vendor) could not read this reply (error \(status)).", fatal: false)
        }
    }

    static let unreachable = VoiceVendorError(message: "The voice provider could not be reached. Check your connection.", fatal: false)
}

// MARK: - Speech (vendor TTS)

/// Reads one sentence through the vendor's streaming TTS as PCM16 mono at
/// 24 kHz. A failure is reported through `onError` (the engine's synthesizer
/// contract doesn't throw).
final class VendorSpeechSynthesizer: LocalSynthesizer, @unchecked Sendable {
    enum Vendor: Sendable {
        case openai(key: String)
        case elevenlabs(key: String, voiceId: String)

        var name: String {
            switch self {
            case .openai: return "OpenAI"
            case .elevenlabs: return "ElevenLabs"
            }
        }
    }

    let outputSampleRate = 24_000
    /// 100 ms of PCM16 at 24 kHz (even, so frames stay sample-aligned).
    private static let chunkBytes = 4_800
    private static let openAIVoices: Set<String> = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"]

    private let vendor: Vendor
    private let session: URLSession
    private let lock = NSLock()
    private var current: Task<Void, Never>?
    private var errorSink: (@Sendable (VoiceVendorError) -> Void)?

    init(vendor: Vendor, session: URLSession = .shared) {
        self.vendor = vendor
        self.session = session
    }

    func onError(_ sink: @escaping @Sendable (VoiceVendorError) -> Void) {
        lock.withLock { errorSink = sink }
    }

    func synthesize(_ text: String, voice: String?, onChunk: @escaping @Sendable (Data) -> Void) async {
        let request = makeRequest(text: text, voice: voice)
        let vendorName = vendor.name
        let task = Task { [session, weak self] in
            do {
                let (bytes, response) = try await session.bytes(for: request)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                guard (200..<300).contains(status) else { throw VoiceVendorError.status(vendorName, status) }
                var buffer = Data()
                buffer.reserveCapacity(Self.chunkBytes)
                for try await byte in bytes {
                    buffer.append(byte)
                    if buffer.count >= Self.chunkBytes {
                        onChunk(buffer)
                        buffer = Data()
                        buffer.reserveCapacity(Self.chunkBytes)
                    }
                }
                let even = buffer.count - buffer.count % 2
                if even > 0 { onChunk(buffer.prefix(even)) }
            } catch {
                guard !Task.isCancelled else { return }
                self?.report(error as? VoiceVendorError ?? .unreachable)
            }
        }
        lock.withLock { current = task }
        await task.value
    }

    func cancel() {
        let task = lock.withLock { () -> Task<Void, Never>? in
            defer { current = nil }
            return current
        }
        task?.cancel()
    }

    private func report(_ error: VoiceVendorError) {
        lock.withLock { errorSink }?(error)
    }

    private func makeRequest(text: String, voice: String?) -> URLRequest {
        var request: URLRequest
        let body: [String: Any]
        switch vendor {
        case .openai(let key):
            request = URLRequest(url: URL(string: "https://api.openai.com/v1/audio/speech")!)
            request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
            // Kokoro ids share OpenAI's voice names (af_alloy, am_onyx, bm_fable…).
            let name = voice?.split(separator: "_").last.map(String.init)
            body = ["model": "gpt-4o-mini-tts",
                    "voice": name.flatMap { Self.openAIVoices.contains($0) ? $0 : nil } ?? "alloy",
                    "input": text, "response_format": "pcm"]
        case .elevenlabs(let key, let voiceId):
            let id = voiceId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? voiceId
            request = URLRequest(url: URL(string: "https://api.elevenlabs.io/v1/text-to-speech/\(id)/stream?output_format=pcm_24000")!)
            request.setValue(key, forHTTPHeaderField: "xi-api-key")
            body = ["text": text, "model_id": "eleven_flash_v2_5"]
        }
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: body)
        return request
    }
}

/// Listening for the OpenAI route happens at OpenAI; the inner engine only speaks.
private final class SilentDetector: LocalSpeechDetector {
    func accept(_ window: [Float]) {}
    var isSpeechDetected: Bool { false }
    func takeSegments() -> [[Float]] { [] }
    func reset() {}
}

private final class NoRecognizer: LocalRecognizer {
    func transcribe(_ samples: [Float]) -> String { "" }
}

// MARK: - Transport

/// A `VoiceTransport` for the user's own OpenAI or ElevenLabs account. The
/// session client drives it exactly like `LocalVoiceEngine`.
final class ByokVoiceTransport: VoiceTransport, @unchecked Sendable {
    enum Vendor: Sendable { case openai, elevenlabs }

    private static let openAIRate = 24_000

    private let vendor: Vendor
    private let openAIKey: String?
    /// speak.* → vendor TTS (and, for ElevenLabs, listening on this phone).
    private let speech: LocalVoiceEngine
    private let session: URLSession
    private let lock = NSLock()
    private var onEvent: (@Sendable (VoiceTransportEvent) -> Void)?
    private var options = VoiceSessionOptions()
    private var socket: URLSessionWebSocketTask?
    private var socketOpen = false
    private var muted = false
    private var closed = false
    private var startedAt = Date()
    private var speakingIds: Set<String> = []
    private var interruptedIds: Set<String> = []
    private var partial: [String: String] = [:]

    /// Throws "Add your … API key in Settings › Voice." when none is saved.
    @discardableResult
    static func requireKey(_ field: VoiceProviderKeys.Field) throws -> String {
        if let key = VoiceProviderKeys.value(field) { return key }
        let vendor = field == .openai ? "OpenAI" : "ElevenLabs"
        throw VoiceClientError.server(code: "byok_key", message: "Add your \(vendor) API key in Settings › Voice.")
    }

    /// Your own OpenAI. Throws when no key is saved.
    static func openAI(session: URLSession = .shared) throws -> ByokVoiceTransport {
        let key = try requireKey(.openai)
        let synth = VendorSpeechSynthesizer(vendor: .openai(key: key), session: session)
        let engine = LocalVoiceEngine(detector: SilentDetector(), recognizer: NoRecognizer(), synthesizer: synth)
        return ByokVoiceTransport(vendor: .openai, openAIKey: key, speech: engine, synth: synth, session: session)
    }

    /// Your own ElevenLabs; listening on this phone from `paths`. Throws when no key is saved.
    static func elevenLabs(paths: VoicePackPaths, listening: ListeningPreference, session: URLSession = .shared) throws -> ByokVoiceTransport {
        let key = try requireKey(.elevenlabs)
        let voiceId = VoiceProviderKeys.value(.elevenlabsVoiceId) ?? VoiceProviderKeys.defaultElevenLabsVoice
        let synth = VendorSpeechSynthesizer(vendor: .elevenlabs(key: key, voiceId: voiceId), session: session)
        // Same listening as `LocalVoiceEngineFactory`; only the voice differs.
        let moonshine = { SherpaMoonshineRecognizer(paths: paths) as LocalRecognizer }
        let recognizer: LocalRecognizer = switch listening {
        case .automatic: SystemFirstRecognizer(native: AppleSpeech.makeTranscriber(), moonshine: moonshine)
        case .allternit: moonshine()
        }
        let engine = LocalVoiceEngine(detector: SherpaSpeechDetector(modelPath: paths.vad), recognizer: recognizer, synthesizer: synth)
        return ByokVoiceTransport(vendor: .elevenlabs, openAIKey: nil, speech: engine, synth: synth, session: session)
    }

    private init(vendor: Vendor, openAIKey: String?, speech: LocalVoiceEngine, synth: VendorSpeechSynthesizer, session: URLSession) {
        self.vendor = vendor
        self.openAIKey = openAIKey
        self.speech = speech
        self.session = session
        synth.onError { [weak self] error in
            self?.emit(.event(.error(code: "byok-tts", message: error.message, fatal: error.fatal)))
        }
    }

    // MARK: VoiceTransport

    func connect(onEvent: @escaping @Sendable (VoiceTransportEvent) -> Void) async throws {
        lock.withLock { self.onEvent = onEvent; closed = false }
        try await speech.connect { [weak self] event in self?.fromEngine(event) }
    }

    func send(_ message: VoiceClientMessage) {
        switch message {
        case .start(let opts):
            lock.withLock { options = opts; startedAt = Date() }
            speech.send(message)
            if vendor == .openai { Task { await self.startOpenAI() } }
        case .update(let opts):
            lock.withLock { options = opts }
            speech.send(message)
        case .micMute, .micUnmute:
            lock.withLock { muted = message == .micMute }
            speech.send(message)
        default:
            speech.send(message)
        }
    }

    func sendAudio(_ data: Data) {
        guard vendor == .openai else { speech.sendAudio(data); return }
        let (socket, rate) = lock.withLock { () -> (URLSessionWebSocketTask?, Int) in
            (socketOpen && !muted ? self.socket : nil, options.inputSampleRate)
        }
        guard let socket else { return }
        let pcm = VoicePCM.encode(Self.resample(VoicePCM.decode(data), from: rate, to: Self.openAIRate))
        let frame = ["type": "input_audio_buffer.append", "audio": pcm.base64EncodedString()]
        socket.send(.string(Self.json(frame))) { _ in }
    }

    func close() {
        let socket = lock.withLock { () -> URLSessionWebSocketTask? in
            closed = true
            socketOpen = false
            onEvent = nil
            defer { self.socket = nil }
            return self.socket
        }
        socket?.cancel(with: .normalClosure, reason: nil)
        speech.close()
    }

    // MARK: Engine events

    private func emit(_ event: VoiceTransportEvent) {
        lock.withLock { onEvent }?(event)
    }

    private func fromEngine(_ event: VoiceTransportEvent) {
        guard case .event(let server) = event else { emit(event); return }
        switch (vendor, server) {
        case (.openai, .ready):
            return // ready once the Realtime socket is open
        case (.elevenlabs, .ready(let r)):
            emit(.event(.ready(.init(sessionId: r.sessionId, engine: .elevenlabs, outputSampleRate: r.outputSampleRate,
                                     voices: r.voices, protocolVersion: r.protocolVersion))))
        case (.openai, .speakStarted(let id)):
            lock.withLock { _ = speakingIds.insert(id) }
            emit(event)
        case (.openai, .speakEnded(let id)):
            let interrupted = lock.withLock { () -> Bool in
                speakingIds.remove(id)
                return interruptedIds.remove(id) != nil
            }
            emit(interrupted ? .event(.speakInterrupted(id: id, sentMs: 0)) : event)
        default:
            emit(event)
        }
    }

    // MARK: OpenAI Realtime listening

    private func transcriptionSession(_ opts: VoiceSessionOptions) -> [String: Any] {
        var transcription: [String: Any] = ["model": "gpt-4o-transcribe"]
        if !opts.language.isEmpty { transcription["language"] = opts.language }
        return [
            "type": "transcription",
            "audio": ["input": [
                "format": ["type": "audio/pcm", "rate": Self.openAIRate] as [String: Any],
                "noise_reduction": ["type": "near_field"],
                "transcription": transcription,
                "turn_detection": ["type": "server_vad", "silence_duration_ms": 600] as [String: Any],
            ] as [String: Any]],
        ]
    }

    private func startOpenAI() async {
        guard let key = openAIKey else { return }
        let opts = lock.withLock { options }
        var request = URLRequest(url: URL(string: "https://api.openai.com/v1/realtime/client_secrets")!)
        request.httpMethod = "POST"
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: [
            "expires_after": ["anchor": "created_at", "seconds": 600] as [String: Any],
            "session": transcriptionSession(opts),
        ] as [String: Any])
        let secret: String
        do {
            let (data, response) = try await session.data(for: request)
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            guard (200..<300).contains(status) else {
                let error = VoiceVendorError.status("OpenAI", status)
                if error.fatal { emit(.event(.error(code: "byok-openai", message: error.message, fatal: true))) }
                else { emit(.closed("OpenAI error \(status)")) }
                return
            }
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            guard let value = object?["value"] as? String, !value.isEmpty else { throw VoiceVendorError.unreachable }
            secret = value
        } catch {
            // Network or vendor hiccup: drop so the client retries with backoff.
            emit(.closed(error.localizedDescription))
            return
        }
        let url = URL(string: "wss://api.openai.com/v1/realtime?intent=transcription")!
        let ws = session.webSocketTask(with: url, protocols: ["realtime", "openai-insecure-api-key.\(secret)"])
        let alive = lock.withLock { () -> Bool in
            guard !closed else { return false }
            socket = ws
            return true
        }
        guard alive else { return }
        ws.resume()
        receive(ws)
        ws.send(.string(Self.json(["type": "session.update", "session": transcriptionSession(opts)]))) { _ in }
        // The pong arrives once the handshake is done: the socket is open.
        ws.sendPing { [weak self] error in
            guard let self else { return }
            if let error { self.dropped(ws, error); return }
            let current = self.lock.withLock { () -> Bool in
                guard self.socket === ws else { return false }
                self.socketOpen = true
                return true
            }
            guard current else { return }
            self.emit(.event(.ready(.init(sessionId: "openai-\(UUID().uuidString.prefix(8))", engine: .openai,
                                          outputSampleRate: 24_000, voices: [], protocolVersion: 1))))
        }
    }

    private func receive(_ ws: URLSessionWebSocketTask) {
        ws.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(.string(let text)):
                self.onOpenAIEvent(text)
                self.receive(ws)
            case .success:
                self.receive(ws)
            case .failure(let error):
                self.dropped(ws, error)
            }
        }
    }

    private func dropped(_ ws: URLSessionWebSocketTask, _ error: Error) {
        let current = lock.withLock { () -> Bool in
            guard socket === ws, !closed else { return false }
            socket = nil
            socketOpen = false
            return true
        }
        if current { emit(.closed(error.localizedDescription)) }
    }

    private func onOpenAIEvent(_ raw: String) {
        guard let data = raw.data(using: .utf8),
              let ev = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = ev["type"] as? String else { return }
        let atMs = lock.withLock { Int(Date().timeIntervalSince(startedAt) * 1000) }
        switch type {
        case "input_audio_buffer.speech_started":
            emit(.event(.speechStarted(atMs: atMs)))
            let interrupt = lock.withLock { () -> Bool in
                guard options.bargeIn, !speakingIds.isEmpty else { return false }
                interruptedIds.formUnion(speakingIds)
                return true
            }
            if interrupt { speech.send(.speakCancel(id: nil)) }
        case "input_audio_buffer.speech_stopped":
            emit(.event(.speechStopped(atMs: atMs)))
        case "conversation.item.input_audio_transcription.delta":
            let id = ev["item_id"] as? String ?? "seg"
            let text = lock.withLock { () -> String in
                let text = (partial[id] ?? "") + (ev["delta"] as? String ?? "")
                partial[id] = text
                return text
            }
            emit(.event(.transcriptDelta(segmentId: id, text: text)))
        case "conversation.item.input_audio_transcription.completed":
            let id = ev["item_id"] as? String ?? "seg"
            _ = lock.withLock { partial.removeValue(forKey: id) }
            let text = (ev["transcript"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return }
            emit(.event(.transcriptFinal(segmentId: id, text: text)))
            emit(.event(.turnEnded(text: text, confidence: 1.0)))
        case "error":
            let error = ev["error"] as? [String: Any]
            let code = error?["code"] as? String ?? ""
            let fatal = code.range(of: "api_key|auth|quota|billing", options: [.regularExpression, .caseInsensitive]) != nil
            let message = fatal
                ? "OpenAI stopped this voice session. Check your API key and credit in Settings › Voice."
                : (error?["message"] as? String ?? "OpenAI reported a problem.")
            emit(.event(.error(code: "byok-openai", message: message, fatal: fatal)))
        default:
            break
        }
    }

    // MARK: Helpers

    private static func json(_ object: [String: Any]) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: object)) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    /// Linear resampling (16 kHz mic → OpenAI's 24 kHz).
    static func resample(_ samples: [Float], from: Int, to: Int) -> [Float] {
        guard from != to, from > 0, samples.count > 1 else { return samples }
        let count = Int((Double(samples.count) * Double(to) / Double(from)).rounded())
        let step = Double(from) / Double(to)
        return (0..<count).map { i in
            let pos = Double(i) * step
            let j = min(Int(pos), samples.count - 1)
            let k = min(j + 1, samples.count - 1)
            let frac = Float(pos - Double(j))
            return samples[j] + (samples[k] - samples[j]) * frac
        }
    }
}
