// Replaces iCTRL's loopback IngestLink (https://github.com/Allternit/allternit-ictrl,
// iCTRLBroadcast/SampleHandler.swift). Copyright iCTRL authors. Licensed under
// the Apache License, Version 2.0; see http://www.apache.org/licenses/LICENSE-2.0.
// Modified for Allternit: an outbound, sealed WebSocket instead of 127.0.0.1 TCP.

import Foundation

/// The extension's end of `WS /api/v1/phones/app/ingest?phone=<id>`, dialled
/// out through the cloud relay with a ticket minted by the app phone key.
///
/// The Desktop may be asleep, the relay may restart and mobile networks drop,
/// so the socket keeps retrying (with backoff) for as long as the broadcast
/// lives. Each binary frame is one LiveScreenPacket sealed in a SecureEnvelope.
/// Text frames from the node are controls: `{"type":"resume"|"pause"|"keyframe"}`,
/// plain or sealed. All members are used on the handler's queue.
final class IngestSocket: NSObject, URLSessionWebSocketDelegate {
    var onReady: (() -> Void)?
    var onClosed: (() -> Void)?
    var onControl: ((String) -> Void)?
    /// The phone was revoked or removed: retrying can't help, end the broadcast.
    var onFatal: ((String) -> Void)?

    private let queue: DispatchQueue
    private lazy var session: URLSession = {
        let delegateQueue = OperationQueue()
        delegateQueue.underlyingQueue = queue
        delegateQueue.maxConcurrentOperationCount = 1
        let config = URLSessionConfiguration.ephemeral
        config.waitsForConnectivity = false
        return URLSession(configuration: config, delegate: self, delegateQueue: delegateQueue)
    }()
    private var task: URLSessionWebSocketTask?
    private var credentials: AppPhoneCredentials?
    private var key = Data()
    private var finished = false
    private var connecting = false
    private var retryDelay: TimeInterval = 1
    private(set) var isReady = false
    /// Frames submitted to the encoder or written but not yet taken by the
    /// socket. Only touched on `queue`.
    private(set) var inFlight = 0
    /// AAC packets written but not yet taken by the socket. Audio is never
    /// queued: past `maxAudioInFlight` (~170 ms of sound) new packets are
    /// dropped until the socket catches up.
    private(set) var audioInFlight = 0
    static let maxAudioInFlight = 8
    /// The relay drops a silent socket; a still screen sends nothing.
    private static let pingInterval: TimeInterval = 20

    init(queue: DispatchQueue) {
        self.queue = queue
    }

    func start(credentials: AppPhoneCredentials, key: Data) {
        self.credentials = credentials
        self.key = key
        connect()
    }

    private func connect() {
        guard !finished, task == nil, !connecting, let credentials else { return }
        connecting = true
        Task { [weak self] in
            do {
                let url = try await AppPhoneRelay.socketURL(.ingest, credentials: credentials)
                self?.queue.async { self?.open(url) }
            } catch let failure as AppPhoneRelay.Failure where failure.isFatal {
                self?.queue.async {
                    self?.connecting = false
                    self?.onFatal?("This iPhone is no longer paired. Pair it again in Settings → Phones.")
                }
            } catch {
                // Desktop offline or waking, no network, rate-limited: later.
                self?.queue.async {
                    self?.connecting = false
                    self?.scheduleRetry()
                }
            }
        }
    }

    private func open(_ url: URL) {
        connecting = false
        guard !finished, task == nil else { return }
        let t = session.webSocketTask(with: url)
        t.maximumMessageSize = 16 << 20
        task = t
        t.resume()
    }

    private func scheduleRetry() {
        guard !finished else { return }
        let delay = retryDelay
        retryDelay = min(retryDelay * 2, 15)
        queue.asyncAfter(deadline: .now() + delay) { [weak self] in self?.connect() }
    }

    // MARK: - URLSessionWebSocketDelegate (on `queue`)

    func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask,
                    didOpenWithProtocol protocol: String?) {
        guard webSocketTask === task else { return }
        isReady = true
        retryDelay = 1
        receive(on: webSocketTask)
        schedulePing(on: webSocketTask)
        onReady?()
    }

    func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask,
                    didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        closed(webSocketTask)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let ws = task as? URLSessionWebSocketTask else { return }
        closed(ws)
    }

    private func closed(_ t: URLSessionWebSocketTask) {
        guard t === task else { return }
        let wasReady = isReady
        isReady = false
        task = nil
        inFlight = 0
        audioInFlight = 0
        if wasReady { onClosed?() }
        scheduleRetry()
    }

    private func receive(on t: URLSessionWebSocketTask) {
        t.receive { [weak self] result in
            self?.queue.async {
                guard let self, self.task === t else { return }
                switch result {
                case .success(.string(let text)):
                    if let type = self.controlType(Data(text.utf8)) { self.onControl?(type) }
                case .success(.data(let data)):
                    if let type = self.controlType(data) { self.onControl?(type) }
                case .success:
                    break
                case .failure:
                    t.cancel(with: .goingAway, reason: nil)
                    return
                }
                self.receive(on: t)
            }
        }
    }

    /// `{"type":…}`, either as is or inside a SecureEnvelope.
    private func controlType(_ data: Data) -> String? {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        if obj["ct"] != nil {
            guard let opened = try? SecureEnvelope.open(data, key: key) else { return nil }
            return controlType(opened.plaintext)
        }
        return obj["type"] as? String
    }

    private func schedulePing(on t: URLSessionWebSocketTask) {
        queue.asyncAfter(deadline: .now() + Self.pingInterval) { [weak self] in
            guard let self, self.task === t, self.isReady else { return }
            t.sendPing { _ in }
            self.schedulePing(on: t)
        }
    }

    // MARK: - Sending

    func reserve() { inFlight += 1 }
    func release() { inFlight = max(0, inFlight - 1) }

    private func write(_ packet: Data, completion: @escaping (Error?) -> Void) -> Bool {
        guard let t = task, isReady, let sealed = SecureEnvelope.seal(packet, key: key) else { return false }
        t.send(.data(sealed)) { [weak self] error in
            self?.queue.async {
                completion(error)
                if error != nil, self?.task === t { t.cancel(with: .goingAway, reason: nil) }
            }
        }
        return true
    }

    /// A frame the encoder produced against an earlier `reserve()`.
    func sendFrame(_ packet: Data) {
        if !write(packet, completion: { [weak self] _ in self?.release() }) { release() }
    }

    /// One AAC packet; dropped (not queued) when the socket is backed up.
    func sendAudio(_ packet: Data) {
        guard audioInFlight < Self.maxAudioInFlight else { return }
        if write(packet, completion: { [weak self] _ in
            if let self { self.audioInFlight = max(0, self.audioInFlight - 1) }
        }) {
            audioInFlight += 1
        }
    }

    /// STATUS / CONFIG / AUDIO_CONFIG / ENDED — small, never skipped.
    func send(_ packet: Data) {
        _ = write(packet, completion: { _ in })
    }

    /// Last word, then close and stop retrying.
    func finish(with packet: Data) {
        finished = true
        guard let t = task, isReady else {
            task?.cancel(with: .goingAway, reason: nil)
            session.invalidateAndCancel()
            return
        }
        let s = session
        if !write(packet, completion: { _ in
            t.cancel(with: .normalClosure, reason: nil)
            s.finishTasksAndInvalidate()
        }) {
            t.cancel(with: .normalClosure, reason: nil)
            s.finishTasksAndInvalidate()
        }
    }
}
