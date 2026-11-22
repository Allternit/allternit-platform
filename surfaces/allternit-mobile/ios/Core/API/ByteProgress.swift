import Foundation

/// Real byte progress for a download (stored clips and pictures, saving to
/// Photos or Files). Nothing here guesses: `totalBytes` is the response's
/// content length and is nil when there is none (or the body is
/// content-encoded, because then the length counts compressed bytes while we
/// count decoded ones); `etaSeconds` comes from a smoothed rate and is nil
/// until it means something. Mirrors allternit-ai `src/lib/media/byte-progress.ts`.
struct ByteProgress: Equatable, Sendable {
    /// Bytes received so far.
    var bytes: Int64
    /// Whole size from the content length. Nil when unknown.
    var totalBytes: Int64?
    /// Seconds left, from a smoothed rate. Nil until meaningful.
    var etaSeconds: Int?

    /// 0...1 when the total is known; nil otherwise.
    var fraction: Double? {
        guard let totalBytes, totalBytes > 0 else { return nil }
        return min(1, Double(bytes) / Double(totalBytes))
    }
}

/// The size and ETA rules, with time passed in so they can be tested.
struct ByteProgressTracker {
    /// Rate smoothing window: an exponentially weighted average over about this long.
    static let rateWindow: TimeInterval = 2
    /// No ETA until the download has run this long.
    static let etaMinElapsed: TimeInterval = 1

    let total: Int64?
    private let startedAt: TimeInterval
    private var lastAt: TimeInterval
    private var bytes: Int64 = 0
    private var pending: Int64 = 0
    private var rate: Double? // bytes per second

    init(total: Int64?, startedAt: TimeInterval) {
        self.total = total
        self.startedAt = startedAt
        self.lastAt = startedAt
    }

    /// The whole size from a response: nil for an unknown length (-1 or 0)
    /// and for a content-encoded body.
    static func totalBytes(expectedContentLength: Int64, contentEncoding: String?) -> Int64? {
        if let encoding = contentEncoding?.trimmingCharacters(in: .whitespaces).lowercased(), !encoding.isEmpty, encoding != "identity" { return nil }
        return expectedContentLength > 0 ? expectedContentLength : nil
    }

    /// Records a chunk and returns the progress so far.
    mutating func add(_ count: Int, at now: TimeInterval) -> ByteProgress {
        bytes += Int64(count)
        pending += Int64(count)
        let dt = now - lastAt
        if dt > 0 {
            let instant = Double(pending) / dt
            let alpha = 1 - exp(-dt / Self.rateWindow)
            rate = rate.map { $0 + alpha * (instant - $0) } ?? instant
            pending = 0
            lastAt = now
        }
        var progress = ByteProgress(bytes: bytes)
        if let total {
            progress.totalBytes = max(total, bytes)
            if let rate, rate > 0, now - startedAt >= Self.etaMinElapsed, bytes < total {
                progress.etaSeconds = Int((Double(total - bytes) / rate).rounded(.up))
            }
        }
        return progress
    }

    /// The last report, once the body is complete.
    func finished() -> ByteProgress {
        ByteProgress(bytes: bytes, totalBytes: total.map { max($0, bytes) } ?? bytes)
    }
}

enum ByteProgressText {
    /// "3.0 MB", "412 KB".
    static func formatBytes(_ n: Int64) -> String {
        if n >= 1_048_576 { return String(format: "%.1f MB", Double(n) / 1_048_576) }
        if n >= 1024 { return "\(Int((Double(n) / 1024).rounded())) KB" }
        return "\(n) B"
    }

    /// "Loading 41% · 3.0 of 7.4 MB · ETA 3 s", or with no size "Loading · 3.0 MB · — ETA".
    static func status(verb: String, _ p: ByteProgress) -> String {
        let eta = p.etaSeconds.map { "ETA \($0) s" } ?? "— ETA"
        guard let total = p.totalBytes else { return "\(verb) · \(formatBytes(p.bytes)) · \(eta)" }
        let pct = min(100, Int((Double(p.bytes) / Double(total) * 100).rounded(.down)))
        let sameUnit = (total >= 1_048_576) == (p.bytes >= 1_048_576)
        var done = formatBytes(p.bytes)
        if sameUnit, let space = done.lastIndex(of: " ") { done = String(done[..<space]) }
        return "\(verb) \(pct)% · \(done) of \(formatBytes(total)) · \(eta)"
    }

    /// Poster blur in points: 28 at 0 bytes down to 0 at the end; held at 16 when the size is unknown.
    static func blur(_ p: ByteProgress?) -> CGFloat {
        guard let p else { return 28 }
        guard let fraction = p.fraction else { return 16 }
        return CGFloat((28 * (1 - fraction)).rounded())
    }
}

/// Downloads a URL to memory and reports real bytes as chunks arrive
/// (URLSession data delegate). Cancelling the task cancels the download.
final class ByteDownloader: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let request: URLRequest
    private let onProgress: @Sendable (ByteProgress) -> Void
    private var session: URLSession?
    private var continuation: CheckedContinuation<Data, Error>?
    private var data = Data()
    private var tracker: ByteProgressTracker?
    private var failure: Error?
    private let lock = NSLock()

    init(request: URLRequest, onProgress: @escaping @Sendable (ByteProgress) -> Void) {
        self.request = request
        self.onProgress = onProgress
    }

    func run() async throws -> Data {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
                lock.lock()
                continuation = cont
                let session = URLSession(configuration: .ephemeral, delegate: self, delegateQueue: nil)
                self.session = session
                lock.unlock()
                session.dataTask(with: request).resume()
            }
        } onCancel: { [weak self] in self?.session?.invalidateAndCancel() }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            failure = APIError.httpError(statusCode: http.statusCode, message: nil)
            completionHandler(.cancel)
            return
        }
        let encoding = (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Content-Encoding")
        tracker = ByteProgressTracker(
            total: ByteProgressTracker.totalBytes(expectedContentLength: response.expectedContentLength, contentEncoding: encoding),
            startedAt: ProcessInfo.processInfo.systemUptime
        )
        onProgress(ByteProgress(bytes: 0, totalBytes: tracker?.total))
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive chunk: Data) {
        data.append(chunk)
        if var t = tracker {
            onProgress(t.add(chunk.count, at: ProcessInfo.processInfo.systemUptime))
            tracker = t
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        let cont = continuation
        continuation = nil
        lock.unlock()
        session.finishTasksAndInvalidate()
        if let failure { cont?.resume(throwing: failure) }
        else if let error {
            cont?.resume(throwing: (error as? URLError)?.code == .cancelled ? CancellationError() : error)
        } else {
            if let tracker { onProgress(tracker.finished()) }
            cont?.resume(returning: data)
        }
    }
}
