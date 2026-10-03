import Foundation

/// Connection lifecycle surfaced to the UI.
enum VoiceConnectionState: Equatable, Sendable {
    case idle
    case connecting
    case ready(engine: VoiceEngineKind)
    /// A dropped connection is being re-established (attempt 1…max).
    case reconnecting(attempt: Int)
    case ended
    case failed(String)
}

/// Events the client hands to its owner (the voice view model).
enum VoiceSessionClientEvent: Sendable, Equatable {
    case state(VoiceConnectionState)
    case server(VoiceServerEvent)
    /// The connection came back after a drop. Speech in flight is lost.
    case reconnected
}

/// Opens one transport. Called again on every reconnect (cloud: mints a
/// fresh single-use ticket each time).
typealias VoiceTransportFactory = @Sendable () async throws -> VoiceTransport

/// Speaks the Voice Session protocol over any transport: `session.start`,
/// mic frames out, speech frames in, instant flush on `speak.interrupted`,
/// reconnect with backoff when the socket drops mid-session.
@MainActor
final class VoiceSessionClient {
    static let maxReconnectAttempts = 3

    var onEvent: ((VoiceSessionClientEvent) -> Void)?

    private let factory: VoiceTransportFactory
    private let audio: VoiceAudioIO
    private var transport: VoiceTransport?
    private var options = VoiceSessionOptions()
    private var outputSampleRate = 24_000
    private var ending = false
    private var micMuted = false
    /// Speech frames are only played between `speak.started` and the
    /// utterance ending/interrupting — late frames after a barge-in are
    /// dropped even if they were already in flight.
    private var speaking = false
    private var generation = 0
    /// True between `session.ready` and a drop: mic frames before the
    /// handshake (or during a reconnect) would be rejected as `not_started`.
    private var live = false
    private var readyContinuation: CheckedContinuation<VoiceServerEvent.Ready, Error>?

    init(factory: @escaping VoiceTransportFactory, audio: VoiceAudioIO = VoiceAudioIO()) {
        self.factory = factory
        self.audio = audio
    }

    // MARK: - Lifecycle

    /// Connects, sends `session.start`, waits for `session.ready` and starts
    /// the mic. Throws the transport's/cloud's error (typed `VoiceCloudError`
    /// for ticket failures) or `VoiceClientError.server` for a fatal `error`.
    func start(options: VoiceSessionOptions) async throws {
        self.options = options
        ending = false
        emit(.state(.connecting))
        do {
            let ready = try await connectAndHandshake()
            try startAudio(ready)
            emit(.state(.ready(engine: ready.engine)))
        } catch {
            transport?.close()
            transport = nil
            emit(.state(.failed(error.localizedDescription)))
            throw error
        }
    }

    func end() {
        guard !ending else { return }
        ending = true
        generation += 1
        transport?.send(.end)
        transport?.close()
        transport = nil
        audio.stop()
        emit(.state(.ended))
    }

    // MARK: - Controls

    func updateOptions(_ new: VoiceSessionOptions) {
        options = new
        transport?.send(.update(new))
    }

    func setMuted(_ muted: Bool) {
        micMuted = muted
        audio.setMicMuted(muted)
        transport?.send(muted ? .micMute : .micUnmute)
    }

    func speakDelta(id: String, text: String) {
        guard !text.isEmpty else { return }
        transport?.send(.speakDelta(id: id, text: text))
    }

    func speakDone(id: String) { transport?.send(.speakDone(id: id)) }

    /// Stops speaking now and drops queued audio (tap-to-interrupt).
    func cancelSpeech(id: String? = nil) {
        transport?.send(.speakCancel(id: id))
        speaking = false
        audio.flushPlayback()
    }

    // MARK: - Connect

    private func connectAndHandshake() async throws -> VoiceServerEvent.Ready {
        let transport = try await factory()
        self.transport = transport
        generation += 1
        let myGeneration = generation
        let sink: @Sendable (VoiceTransportEvent) -> Void = { [weak self] event in
            Task { @MainActor [weak self] in self?.handle(event, generation: myGeneration) }
        }
        try await transport.connect(onEvent: sink)
        live = false
        let ready: VoiceServerEvent.Ready = try await withCheckedThrowingContinuation { continuation in
            readyContinuation = continuation
            transport.send(.start(options))
            // Cloud tickets live 60 s; a handshake that takes >10 s is dead.
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                self?.resumeReady(.failure(VoiceClientError.handshakeTimeout))
            }
        }
        live = true
        return ready
    }

    private func startAudio(_ ready: VoiceServerEvent.Ready) throws {
        outputSampleRate = ready.outputSampleRate
        audio.setMicMuted(micMuted)
        audio.onMicFrame = { [weak self] data in
            // Audio thread → the transport is thread-safe; no actor hop.
            self?.sendMic(data)
        }
        try audio.start(outputSampleRate: ready.outputSampleRate)
    }

    private nonisolated func sendMic(_ data: Data) {
        // `transport` is main-actor state; hop only to read it.
        Task { @MainActor [weak self] in
            guard let self, self.live else { return }
            self.transport?.sendAudio(data)
        }
    }

    private func resumeReady(_ result: Result<VoiceServerEvent.Ready, Error>) {
        guard let continuation = readyContinuation else { return }
        readyContinuation = nil
        continuation.resume(with: result)
    }

    // MARK: - Incoming

    private func handle(_ event: VoiceTransportEvent, generation eventGeneration: Int) {
        guard eventGeneration == generation, !ending else { return }
        switch event {
        case .audio(let data):
            guard speaking else { return }
            audio.play(pcm16: data)
        case .event(let server):
            switch server {
            case .ready(let ready):
                resumeReady(.success(ready))
            case .speakStarted:
                speaking = true
            case .speakEnded:
                audio.finishUtterance()
            case .speakInterrupted:
                speaking = false
                audio.flushPlayback()
            case .error(let code, let message, let fatal):
                if fatal {
                    resumeReady(.failure(VoiceClientError.server(code: code, message: message)))
                    // A session cap/engine failure is final: don't loop reconnects.
                    ending = true
                    emit(.server(server))
                    transport?.close()
                    transport = nil
                    audio.stop()
                    emit(.state(.failed(message)))
                    return
                }
            default:
                break
            }
            emit(.server(server))
        case .closed(let reason):
            handleDrop(reason: reason)
        }
    }

    private func handleDrop(reason: String?) {
        guard readyContinuation == nil else {
            resumeReady(.failure(VoiceClientError.dropped(reason ?? "connection closed")))
            return
        }
        live = false
        speaking = false
        audio.flushPlayback()
        Task { await reconnect() }
    }

    private func reconnect() async {
        let myGeneration = generation
        var delay: UInt64 = 500_000_000
        for attempt in 1...Self.maxReconnectAttempts {
            guard !ending, myGeneration == generation else { return }
            emit(.state(.reconnecting(attempt: attempt)))
            try? await Task.sleep(nanoseconds: delay)
            delay *= 2
            guard !ending, myGeneration == generation else { return }
            transport?.close()
            transport = nil
            do {
                let ready = try await connectAndHandshake()
                outputSampleRate = ready.outputSampleRate
                if micMuted { transport?.send(.micMute) }
                emit(.state(.ready(engine: ready.engine)))
                emit(.reconnected)
                return
            } catch let error as VoiceCloudError {
                // No minutes / not signed in won't fix itself by retrying.
                finishFailed(error.localizedDescription)
                return
            } catch {
                continue
            }
        }
        finishFailed("Lost the voice connection.")
    }

    private func finishFailed(_ message: String) {
        guard !ending else { return }
        ending = true
        transport?.close()
        transport = nil
        audio.stop()
        emit(.state(.failed(message)))
    }

    private func emit(_ event: VoiceSessionClientEvent) {
        onEvent?(event)
    }
}

enum VoiceClientError: Error, LocalizedError, Equatable {
    case server(code: String, message: String)
    case handshakeTimeout
    case dropped(String)

    var errorDescription: String? {
        switch self {
        case .server(_, let message): return message
        case .handshakeTimeout: return "Voice didn't respond. Try again."
        case .dropped(let reason): return "Voice connection dropped (\(reason))."
        }
    }
}
