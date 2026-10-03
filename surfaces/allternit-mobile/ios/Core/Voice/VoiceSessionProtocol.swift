import Foundation

/// Voice Session protocol v1 (`services/voice/spec/VOICE_SESSION.md`) — the
/// one wire format shared by Desktop, web and iOS, whether the engine runs in
/// the cloud (WebSocket) or on this phone (in-process, `LocalVoiceEngine`).
///
/// Pure value types: parsing and encoding only, no I/O, so every frame the
/// spec defines is unit-tested without a socket.

/// Where a session's engine runs (`session.ready.engine`).
enum VoiceEngineKind: String, Sendable, Equatable {
    case device
    case cloud
}

/// A server → client event. Unknown event types and fields are ignored per
/// the spec's versioning rule (`parse` returns nil for an unknown type).
enum VoiceServerEvent: Sendable, Equatable {
    struct Ready: Sendable, Equatable {
        let sessionId: String
        let engine: VoiceEngineKind
        let outputSampleRate: Int
        let voices: [String]
        let protocolVersion: Int
    }

    case ready(Ready)
    case speechStarted(atMs: Int)
    case speechStopped(atMs: Int)
    case transcriptDelta(segmentId: String, text: String)
    case transcriptFinal(segmentId: String, text: String)
    case turnEnded(text: String, confidence: Double)
    case speakStarted(id: String)
    case speakEnded(id: String)
    case speakInterrupted(id: String, sentMs: Int)
    case error(code: String, message: String, fatal: Bool)

    /// Parses one JSON text frame. Returns nil for a non-JSON frame, a frame
    /// without a `type`, an unknown `type`, or a known type missing a
    /// required field (a malformed frame must never crash the client).
    static func parse(_ text: String) -> VoiceServerEvent? {
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = object["type"] as? String else { return nil }

        func int(_ key: String) -> Int? {
            if let value = object[key] as? Int { return value }
            if let value = object[key] as? Double { return Int(value) }
            return nil
        }
        func string(_ key: String) -> String? { object[key] as? String }

        switch type {
        case "session.ready":
            guard let sessionId = string("sessionId") else { return nil }
            let engine = string("engine").flatMap(VoiceEngineKind.init(rawValue:)) ?? .cloud
            return .ready(Ready(
                sessionId: sessionId,
                engine: engine,
                outputSampleRate: int("outputSampleRate") ?? 24_000,
                voices: (object["voices"] as? [String]) ?? [],
                protocolVersion: int("protocol") ?? 1
            ))
        case "speech.started":
            return .speechStarted(atMs: int("atMs") ?? 0)
        case "speech.stopped":
            return .speechStopped(atMs: int("atMs") ?? 0)
        case "transcript.delta":
            guard let text = string("text") else { return nil }
            return .transcriptDelta(segmentId: string("segmentId") ?? "", text: text)
        case "transcript.final":
            guard let text = string("text") else { return nil }
            return .transcriptFinal(segmentId: string("segmentId") ?? "", text: text)
        case "turn.ended":
            guard let text = string("text") else { return nil }
            return .turnEnded(text: text, confidence: (object["confidence"] as? Double) ?? 1.0)
        case "speak.started":
            guard let id = string("id") else { return nil }
            return .speakStarted(id: id)
        case "speak.ended":
            guard let id = string("id") else { return nil }
            return .speakEnded(id: id)
        case "speak.interrupted":
            guard let id = string("id") else { return nil }
            return .speakInterrupted(id: id, sentMs: int("sentMs") ?? 0)
        case "error":
            return .error(
                code: string("code") ?? "error",
                message: string("message") ?? "Voice error",
                fatal: (object["fatal"] as? Bool) ?? false
            )
        default:
            return nil
        }
    }
}

/// Settings sent in `session.start` / `session.update`.
struct VoiceSessionOptions: Sendable, Equatable {
    var voice: String?
    var sttModel: String = "light"
    var language: String = "en"
    var inputSampleRate: Int = 16_000
    var bargeIn: Bool = true
    /// `smart` (VAD + Smart Turn) or `vad`.
    var turnMode: String = "smart"
}

/// A client → server control frame.
enum VoiceClientMessage: Sendable, Equatable {
    case start(VoiceSessionOptions)
    case update(VoiceSessionOptions)
    case speakDelta(id: String, text: String)
    case speakDone(id: String)
    case speakCancel(id: String?)
    case micMute
    case micUnmute
    case end

    /// The JSON text frame for this message (sorted keys: stable for tests).
    var jsonString: String {
        var object: [String: Any]
        switch self {
        case .start(let options):
            object = Self.optionsObject(options)
            object["type"] = "session.start"
        case .update(let options):
            object = Self.optionsObject(options)
            object["type"] = "session.update"
        case .speakDelta(let id, let text):
            object = ["type": "speak.delta", "id": id, "text": text]
        case .speakDone(let id):
            object = ["type": "speak.done", "id": id]
        case .speakCancel(let id):
            object = ["type": "speak.cancel"]
            if let id { object["id"] = id }
        case .micMute:
            object = ["type": "mic.mute"]
        case .micUnmute:
            object = ["type": "mic.unmute"]
        case .end:
            object = ["type": "session.end"]
        }
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    private static func optionsObject(_ options: VoiceSessionOptions) -> [String: Any] {
        var object: [String: Any] = [
            "sttModel": options.sttModel,
            "language": options.language,
            "inputSampleRate": options.inputSampleRate,
            "bargeIn": options.bargeIn,
            "turn": ["mode": options.turnMode],
        ]
        if let voice = options.voice { object["voice"] = voice }
        return object
    }
}

/// PCM16 little-endian ↔ float conversion for mic/speech audio frames.
enum VoicePCM {
    /// Float samples in [-1, 1] → PCM16 LE bytes (clamped).
    static func encode(_ samples: [Float]) -> Data {
        var data = Data(count: samples.count * 2)
        data.withUnsafeMutableBytes { raw in
            let out = raw.bindMemory(to: Int16.self)
            for (index, sample) in samples.enumerated() {
                let clamped = max(-1, min(1, sample))
                out[index] = Int16(clamped * 32_767).littleEndian
            }
        }
        return data
    }

    /// PCM16 LE bytes → floats in [-1, 1]. A trailing odd byte is dropped.
    static func decode(_ data: Data) -> [Float] {
        let count = data.count / 2
        guard count > 0 else { return [] }
        var samples = [Float](repeating: 0, count: count)
        data.withUnsafeBytes { raw in
            for index in 0..<count {
                let low = UInt16(raw[index * 2])
                let high = UInt16(raw[index * 2 + 1]) << 8
                samples[index] = Float(Int16(bitPattern: low | high)) / 32_768
            }
        }
        return samples
    }
}

/// Speech-audio queue between the socket and the speaker. The server paces
/// 20 ms frames at most 250 ms ahead of real time, so this stays small; the
/// buffer exists to hold the first ~`prebufferMs` before playback starts (so
/// network jitter doesn't glitch the first word) and to drop everything at
/// once on `speak.interrupted` (`flush`).
struct PlaybackJitterBuffer: Sendable {
    let sampleRate: Int
    let prebufferMs: Int
    private(set) var queued: [[Float]] = []
    private(set) var queuedSamples = 0
    private(set) var isPlaying = false

    init(sampleRate: Int, prebufferMs: Int = 60) {
        self.sampleRate = sampleRate
        self.prebufferMs = prebufferMs
    }

    var queuedMs: Int { queuedSamples * 1000 / max(1, sampleRate) }

    /// Adds a frame. Returns the frames ready to play now: nothing until the
    /// prebuffer is full, then everything queued, then each frame as it comes.
    mutating func push(_ samples: [Float]) -> [[Float]] {
        guard !samples.isEmpty else { return [] }
        queued.append(samples)
        queuedSamples += samples.count
        if !isPlaying {
            guard queuedMs >= prebufferMs else { return [] }
            isPlaying = true
        }
        return drain()
    }

    /// Releases a short utterance that never reached the prebuffer (the end
    /// of an utterance must not strand its last <60 ms of audio).
    mutating func finish() -> [[Float]] {
        isPlaying = true
        let frames = drain()
        isPlaying = false
        return frames
    }

    /// Drops everything queued and re-arms the prebuffer. The speaker's own
    /// scheduled audio is stopped by the caller.
    mutating func flush() {
        queued.removeAll()
        queuedSamples = 0
        isPlaying = false
    }

    private mutating func drain() -> [[Float]] {
        let frames = queued
        queued.removeAll()
        queuedSamples = 0
        return frames
    }
}
