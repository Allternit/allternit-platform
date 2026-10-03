import Foundation

/// What a transport delivers to `VoiceSessionClient`.
enum VoiceTransportEvent: Sendable {
    case event(VoiceServerEvent)
    /// Speech audio (PCM16 LE mono at `session.ready.outputSampleRate`).
    case audio(Data)
    /// The connection ended. `nil` reason = the peer closed cleanly.
    case closed(String?)
}

/// One end of a Voice Session: the cloud WebSocket, or the in-process
/// `LocalVoiceEngine`. The client can't tell them apart.
protocol VoiceTransport: AnyObject, Sendable {
    /// Opens the connection. Delivered events start flowing to `onEvent`.
    func connect(onEvent: @escaping @Sendable (VoiceTransportEvent) -> Void) async throws
    func send(_ message: VoiceClientMessage)
    /// Mic audio, PCM16 LE mono at the `inputSampleRate` from `session.start`.
    func sendAudio(_ data: Data)
    func close()
}

/// Cloud transport: `URLSessionWebSocketTask`, text frames = JSON, binary
/// frames = PCM16.
final class WebSocketVoiceTransport: NSObject, VoiceTransport, @unchecked Sendable {
    private let url: URL
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private let lock = NSLock()
    private var closedByUs = false

    init(url: URL, session: URLSession = URLSession(configuration: .default)) {
        self.url = url
        self.session = session
    }

    func connect(onEvent: @escaping @Sendable (VoiceTransportEvent) -> Void) async throws {
        let task = session.webSocketTask(with: url)
        lock.lock(); self.task = task; closedByUs = false; lock.unlock()
        task.resume()
        receiveLoop(task: task, onEvent: onEvent)
    }

    func send(_ message: VoiceClientMessage) {
        currentTask()?.send(.string(message.jsonString)) { _ in }
    }

    func sendAudio(_ data: Data) {
        currentTask()?.send(.data(data)) { _ in }
    }

    func close() {
        lock.lock()
        closedByUs = true
        let task = self.task
        self.task = nil
        lock.unlock()
        task?.cancel(with: .normalClosure, reason: nil)
    }

    private func currentTask() -> URLSessionWebSocketTask? {
        lock.lock(); defer { lock.unlock() }
        return task
    }

    private func receiveLoop(task: URLSessionWebSocketTask, onEvent: @escaping @Sendable (VoiceTransportEvent) -> Void) {
        task.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let message):
                switch message {
                case .string(let text):
                    if let event = VoiceServerEvent.parse(text) { onEvent(.event(event)) }
                case .data(let data):
                    onEvent(.audio(data))
                @unknown default:
                    break
                }
                self.receiveLoop(task: task, onEvent: onEvent)
            case .failure(let error):
                self.lock.lock()
                let byUs = self.closedByUs
                self.lock.unlock()
                // A close we asked for is not a drop worth reporting.
                if !byUs { onEvent(.closed(error.localizedDescription)) }
            }
        }
    }
}
