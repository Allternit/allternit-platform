import FreeweightsVerify
import Foundation
import os

private let dlLog = Logger(subsystem: "com.allternit.app", category: "ondevice")

struct InstallProgress: Sendable, Equatable {
    enum Phase: Sendable, Equatable {
        case manifest, downloading, verifying, done
        case failed(String)
    }
    var modelId: String
    var quant: String
    var receivedBytes: Int64
    var totalBytes: Int64
    var phase: Phase

    var fraction: Double { totalBytes > 0 ? min(1, Double(receivedBytes) / Double(totalBytes)) : 0 }
}

enum InstallError: Error, CustomStringConvertible {
    case noFiles
    case noSource
    case notEnoughSpace(needed: Int64, free: Int64)
    case http(Int, String)

    var description: String {
        switch self {
        case .noFiles: return "This build lists no files"
        case .noSource: return "The manifest has no download source"
        case let .notEnoughSpace(n, f):
            let fmt = ByteCountFormatter.string(fromByteCount:countStyle:)
            return "Needs \(fmt(n, .file)) free, \(fmt(f, .file)) available"
        case let .http(code, path): return "Download of \(path) failed (HTTP \(code))"
        }
    }
}

/// Downloads a quant's files in a background URLSession (any network,
/// cellular included, as Eoj picked), resumable, with progress. Before the
/// first load every file is checked against the Freeweights manifest whose
/// minisign signature was verified first: size and sha256, or the file is
/// deleted. Files land in OnDeviceStorage's backup-excluded folder.
///
/// App wiring (AppDelegate): forward
/// `application(_:handleEventsForBackgroundURLSession:completionHandler:)`
/// to `OnDeviceDownloader.handleBackgroundEvents(identifier:completion:)`
/// so downloads that finish while the app is suspended wake it to commit.
final class OnDeviceDownloader: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    static let shared = OnDeviceDownloader()

    static var sessionIdentifier: String {
        (Bundle.main.bundleIdentifier ?? "com.allternit.mobile") + ".ondevice-models"
    }

    /// Called on an arbitrary queue, at most ~4 times a second per install.
    var onProgress: (@Sendable (InstallProgress) -> Void)?

    private let lock = NSLock()
    private var waiters: [String: CheckedContinuation<Void, Error>] = [:]
    private var targets: [String: (url: URL, size: Int64, attempts: Int)] = [:]
    private var received: [String: Int64] = [:]
    private var moveErrors: [String: Error] = [:]
    private var installTotals: [String: (total: Int64, base: Int64, files: Set<String>)] = [:]
    private var lastReport: [String: Date] = [:]
    private var backgroundCompletion: (() -> Void)?

    private lazy var session: URLSession = {
        let c = URLSessionConfiguration.background(withIdentifier: Self.sessionIdentifier)
        c.allowsCellularAccess = true
        c.allowsExpensiveNetworkAccess = true
        c.allowsConstrainedNetworkAccess = true
        c.isDiscretionary = false
        c.sessionSendsLaunchEvents = true
        c.timeoutIntervalForResource = 60 * 60 * 24
        return URLSession(configuration: c, delegate: self, delegateQueue: nil)
    }()

    private override init() { super.init() }

    /// Reattaches to downloads that kept running while the app was gone.
    func activate() { _ = session }

    static func handleBackgroundEvents(identifier: String, completion: @escaping () -> Void) {
        guard identifier == sessionIdentifier else { return }
        shared.lock.withLock { shared.backgroundCompletion = completion }
        shared.activate()
    }

    // MARK: Install

    /// Fetches + verifies the signed manifest, downloads every file of the
    /// quant, checks size and sha256, then moves the folder into place.
    func install(modelId: String, quant: Quant) async throws -> OnDeviceInstall {
        let manifestId = quant.repo ?? modelId
        let ikey = "\(modelId)#\(quant.name)"
        report(ikey, modelId, quant.name, phase: .manifest, force: true)
        do {
            let manifest = try await Self.fetchVerifiedManifest(id: manifestId)
            guard let paths = quant.files, !paths.isEmpty else { throw InstallError.noFiles }
            let entries = try paths.map { p -> FreeweightsManifest.File in
                guard let f = manifest.file(p) else { throw ManifestError.fileNotInManifest(p) }
                return f
            }
            guard let base = manifest.baseURLs.first else { throw InstallError.noSource }

            let partial = OnDeviceStorage.partialFolder(modelId: modelId, quant: quant.name)
            let fm = FileManager.default
            func onDisk(_ f: FreeweightsManifest.File) -> Int64 {
                ((try? fm.attributesOfItem(atPath: partial.appendingPathComponent(f.path).path))?[.size] as? NSNumber)?.int64Value ?? -1
            }
            let todo = entries.filter { onDisk($0) != $0.size }
            let total = entries.reduce(0) { $0 + $1.size }
            let have = total - todo.reduce(0) { $0 + $1.size }
            let needed = todo.reduce(0) { $0 + $1.size }
            let free = OnDeviceStorage.freeDiskBytes
            // 5% + 300 MB headroom so the phone is not left full.
            if needed > 0, free > 0, Double(needed) * 1.05 + 300e6 > Double(free) {
                throw InstallError.notEnoughSpace(needed: needed, free: free)
            }
            lock.withLock {
                installTotals[ikey] = (total, have, Set(todo.map { "\(ikey)#\($0.path)" }))
            }
            dlLog.info("install \(modelId, privacy: .public) \(quant.name, privacy: .public): \(entries.count) files, \(total) bytes, \(needed) to fetch")
            report(ikey, modelId, quant.name, phase: .downloading, force: true)

            for f in todo {
                try await download(key: "\(ikey)#\(f.path)", url: base.appendingPathComponent(f.path), size: f.size)
            }

            report(ikey, modelId, quant.name, phase: .verifying, force: true)
            let verifyStart = Date()
            try await Task.detached(priority: .utility) {
                for f in entries {
                    let url = partial.appendingPathComponent(f.path)
                    do { try FileHash.verify(url, against: f) } catch {
                        try? FileManager.default.removeItem(at: url)
                        throw error
                    }
                }
            }.value
            dlLog.info("verified \(entries.count) files against the signed manifest of \(manifestId, privacy: .public) in \(String(format: "%.1f", Date().timeIntervalSince(verifyStart)), privacy: .public)s")

            let rec = OnDeviceInstall(modelId: modelId, quant: quant.name, engine: quant.engine, manifestId: manifestId,
                                      files: paths, sizeBytes: total, verified: true, installedAt: Date())
            try OnDeviceStorage.commit(rec)
            report(ikey, modelId, quant.name, phase: .done, force: true)
            lock.withLock { _ = installTotals.removeValue(forKey: ikey) }
            return rec
        } catch {
            report(ikey, modelId, quant.name, phase: .failed(String(describing: error)), force: true)
            lock.withLock { _ = installTotals.removeValue(forKey: ikey) }
            throw error
        }
    }

    /// Stops a running install, keeping resume data so a later install continues.
    func cancel(modelId: String, quant: String) {
        let prefix = "\(modelId)#\(quant)#"
        session.getAllTasks { tasks in
            for t in tasks where t.taskDescription?.hasPrefix(prefix) == true {
                if let d = t as? URLSessionDownloadTask, let key = t.taskDescription {
                    d.cancel { data in if let data { try? data.write(to: OnDeviceStorage.resumeFile(key: key)) } }
                } else { t.cancel() }
            }
        }
    }

    static func fetchVerifiedManifest(id: String) async throws -> FreeweightsManifest {
        async let m = URLSession.shared.data(from: FreeweightsManifest.manifestURL(id: id))
        async let s = URLSession.shared.data(from: FreeweightsManifest.signatureURL(id: id))
        let (mData, mResp) = try await m
        let (sData, sResp) = try await s
        for (resp, what) in [(mResp, "manifest"), (sResp, "signature")] {
            let code = (resp as? HTTPURLResponse)?.statusCode ?? 0
            guard code == 200 else { throw InstallError.http(code, "\(id) \(what)") }
        }
        return try FreeweightsManifest.verified(data: mData, signature: String(decoding: sData, as: UTF8.self), expectedId: id)
    }

    // MARK: Per-file download

    private func download(key: String, url: URL, size: Int64) async throws {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                lock.withLock {
                    waiters[key] = cont
                    targets[key] = (url, size, targets[key]?.attempts ?? 0)
                }
                session.getAllTasks { [self] tasks in
                    // A download for this file survived a relaunch: wait for it.
                    if tasks.contains(where: { $0.taskDescription == key && $0.state == .running }) { return }
                    start(key: key)
                }
            }
        } onCancel: { [self] in
            session.getAllTasks { tasks in
                for t in tasks where t.taskDescription == key {
                    (t as? URLSessionDownloadTask)?.cancel { data in
                        if let data { try? data.write(to: OnDeviceStorage.resumeFile(key: key)) }
                    }
                }
            }
        }
    }

    private func start(key: String) {
        guard let target = lock.withLock({ targets[key] }) else { return }
        let resumeURL = OnDeviceStorage.resumeFile(key: key)
        let task: URLSessionDownloadTask
        if let data = try? Data(contentsOf: resumeURL) {
            try? FileManager.default.removeItem(at: resumeURL)
            task = session.downloadTask(withResumeData: data)
            dlLog.info("resuming \(key, privacy: .public)")
        } else {
            task = session.downloadTask(with: target.url)
        }
        task.taskDescription = key
        task.countOfBytesClientExpectsToReceive = target.size
        task.resume()
    }

    // MARK: URLSession delegate

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard let key = downloadTask.taskDescription else { return }
        let parts = key.split(separator: "#", maxSplits: 2).map(String.init)
        guard parts.count == 3 else { return }
        lock.withLock { received[key] = totalBytesWritten }
        report("\(parts[0])#\(parts[1])", parts[0], parts[1], phase: .downloading, force: false)
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        guard let key = downloadTask.taskDescription else { return }
        let parts = key.split(separator: "#", maxSplits: 2).map(String.init)
        guard parts.count == 3 else { return }
        let code = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 0
        guard code == 200 else {
            lock.withLock { moveErrors[key] = InstallError.http(code, parts[2]) }
            return
        }
        // The temp file is deleted when this returns: move it now.
        let dest = OnDeviceStorage.partialFolder(modelId: parts[0], quant: parts[1]).appendingPathComponent(parts[2])
        do {
            let fm = FileManager.default
            try fm.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? fm.removeItem(at: dest)
            try fm.moveItem(at: location, to: dest)
        } catch {
            lock.withLock { moveErrors[key] = error }
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let key = task.taskDescription else { return }
        let (waiter, moveError, attempts) = lock.withLock { () -> (CheckedContinuation<Void, Error>?, Error?, Int) in
            let m = moveErrors.removeValue(forKey: key)
            let a = targets[key]?.attempts ?? 0
            if error == nil || a >= 3 || (error as? URLError)?.code == .cancelled {
                return (waiters.removeValue(forKey: key), m, a)
            }
            return (nil, m, a)
        }
        if let error {
            let resume = (error as NSError).userInfo[NSURLSessionDownloadTaskResumeData] as? Data
            if let resume { try? resume.write(to: OnDeviceStorage.resumeFile(key: key)) }
            if waiter == nil {
                // Network dropped: retry from the resume data, up to 3 times.
                lock.withLock { targets[key]?.attempts = attempts + 1 }
                dlLog.info("retry \(attempts + 1) for \(key, privacy: .public): \(error.localizedDescription, privacy: .public)")
                DispatchQueue.global().asyncAfter(deadline: .now() + Double(2 << attempts)) { [self] in start(key: key) }
                return
            }
            waiter?.resume(throwing: error)
        } else if let moveError {
            waiter?.resume(throwing: moveError)
        } else {
            waiter?.resume()
        }
        lock.withLock { _ = targets.removeValue(forKey: key) }
    }

    func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        let done = lock.withLock { () -> (() -> Void)? in
            defer { backgroundCompletion = nil }
            return backgroundCompletion
        }
        DispatchQueue.main.async { done?() }
    }

    // MARK: Progress

    private func report(_ ikey: String, _ modelId: String, _ quant: String, phase: InstallProgress.Phase, force: Bool) {
        let p: InstallProgress? = lock.withLock {
            let now = Date()
            if !force, let last = lastReport[ikey], now.timeIntervalSince(last) < 0.25 { return nil }
            lastReport[ikey] = now
            let t = installTotals[ikey]
            let got = (t?.files ?? []).reduce(t?.base ?? 0) { $0 + (received[$1] ?? 0) }
            return InstallProgress(modelId: modelId, quant: quant, receivedBytes: got, totalBytes: t?.total ?? 0, phase: phase)
        }
        if let p { onProgress?(p) }
    }
}
