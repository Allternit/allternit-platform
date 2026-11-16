import Foundation

/// The three request shapes the client needs. Bulk bytes never pass through
/// memory: downloads land in a file, uploads stream from a file.
public protocol FilesHTTP: Sendable {
    /// Small request and response (JSON control calls).
    func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse)
    /// Response body written to a new file the caller owns (move or delete it).
    func download(for request: URLRequest) async throws -> (URL, HTTPURLResponse)
    /// Request body streamed from `file`; the response is small.
    func upload(for request: URLRequest, fromFile file: URL) async throws -> (Data, HTTPURLResponse)
}

/// Foreground `URLSession` transport (the app, tests, control calls).
public struct URLSessionFilesHTTP: FilesHTTP {
    public let session: URLSession
    /// Where downloads are moved before returning (same volume as the
    /// caller's destination keeps the final move cheap).
    public let downloadDirectory: URL

    public init(session: URLSession = .shared, downloadDirectory: URL = FileManager.default.temporaryDirectory) {
        self.session = session
        self.downloadDirectory = downloadDirectory
    }

    public func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw FilesError.invalidResponse("not HTTP") }
        return (data, http)
    }

    public func download(for request: URLRequest) async throws -> (URL, HTTPURLResponse) {
        let (temp, response) = try await session.download(for: request)
        guard let http = response as? HTTPURLResponse else {
            try? FileManager.default.removeItem(at: temp)
            throw FilesError.invalidResponse("not HTTP")
        }
        let owned = downloadDirectory.appendingPathComponent("dl-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: downloadDirectory, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: temp, to: owned)
        return (owned, http)
    }

    public func upload(for request: URLRequest, fromFile file: URL) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.upload(for: request, fromFile: file)
        guard let http = response as? HTTPURLResponse else { throw FilesError.invalidResponse("not HTTP") }
        return (data, http)
    }
}

/// Background `URLSession` transport for bulk bytes in the File Provider
/// extension: iOS keeps transfers going when the extension is suspended.
/// Control calls (`data(for:)`) go through a foreground session, since
/// background sessions only run upload and download tasks.
///
/// A transfer whose process died is not resumed through here: the system
/// asks the extension again, and the upload session (`GET /uploads/:id`) or
/// the `.partial` download file picks up where it stopped.
public final class BackgroundFilesHTTP: NSObject, FilesHTTP, URLSessionDownloadDelegate, URLSessionDataDelegate, @unchecked Sendable {
    private static let registryLock = NSLock()
    private static var registry: [String: BackgroundFilesHTTP] = [:]

    /// One instance per identifier per process (iOS allows one background
    /// session per identifier).
    public static func shared(identifier: String, sharedContainerIdentifier: String?, downloadDirectory: URL) -> BackgroundFilesHTTP {
        registryLock.lock()
        defer { registryLock.unlock() }
        if let existing = registry[identifier] { return existing }
        let made = BackgroundFilesHTTP(identifier: identifier, sharedContainerIdentifier: sharedContainerIdentifier,
                                       downloadDirectory: downloadDirectory)
        registry[identifier] = made
        return made
    }

    private enum Pending {
        case download(CheckedContinuation<(URL, HTTPURLResponse), Error>, URL?)
        case upload(CheckedContinuation<(Data, HTTPURLResponse), Error>, Data)
    }

    private let lock = NSLock()
    private var pending: [Int: Pending] = [:]
    private let downloadDirectory: URL
    private let foreground: URLSessionFilesHTTP
    private var session: URLSession!

    private init(identifier: String, sharedContainerIdentifier: String?, downloadDirectory: URL) {
        self.downloadDirectory = downloadDirectory
        self.foreground = URLSessionFilesHTTP(session: URLSession(configuration: .default), downloadDirectory: downloadDirectory)
        super.init()
        let configuration = URLSessionConfiguration.background(withIdentifier: identifier)
        configuration.sharedContainerIdentifier = sharedContainerIdentifier
        configuration.sessionSendsLaunchEvents = false
        configuration.isDiscretionary = false
        session = URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
    }

    public func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        try await foreground.data(for: request)
    }

    public func download(for request: URLRequest) async throws -> (URL, HTTPURLResponse) {
        let task = session.downloadTask(with: request)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                pending[task.taskIdentifier] = .download(continuation, nil)
                lock.unlock()
                task.resume()
            }
        } onCancel: {
            task.cancel()
        }
    }

    public func upload(for request: URLRequest, fromFile file: URL) async throws -> (Data, HTTPURLResponse) {
        let task = session.uploadTask(with: request, fromFile: file)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                pending[task.taskIdentifier] = .upload(continuation, Data())
                lock.unlock()
                task.resume()
            }
        } onCancel: {
            task.cancel()
        }
    }

    // MARK: Delegate

    public func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        // The system deletes `location` when this returns: move it now.
        let owned = downloadDirectory.appendingPathComponent("dl-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: downloadDirectory, withIntermediateDirectories: true)
        let moved = (try? FileManager.default.moveItem(at: location, to: owned)) != nil
        lock.lock()
        if case .download(let continuation, _) = pending[downloadTask.taskIdentifier] {
            pending[downloadTask.taskIdentifier] = .download(continuation, moved ? owned : nil)
        }
        lock.unlock()
    }

    public func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        lock.lock()
        if case .upload(let continuation, var body) = pending[dataTask.taskIdentifier] {
            body.append(data)
            pending[dataTask.taskIdentifier] = .upload(continuation, body)
        }
        lock.unlock()
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        let entry = pending.removeValue(forKey: task.taskIdentifier)
        lock.unlock()
        let http = task.response as? HTTPURLResponse
        switch entry {
        case .download(let continuation, let file):
            if let error { continuation.resume(throwing: error); return }
            guard let http, let file else {
                continuation.resume(throwing: FilesError.invalidResponse("download produced no file"))
                return
            }
            continuation.resume(returning: (file, http))
        case .upload(let continuation, let body):
            if let error { continuation.resume(throwing: error); return }
            guard let http else {
                continuation.resume(throwing: FilesError.invalidResponse("not HTTP"))
                return
            }
            continuation.resume(returning: (body, http))
        case nil:
            // A transfer from a previous process: its caller is gone.
            break
        }
    }
}
