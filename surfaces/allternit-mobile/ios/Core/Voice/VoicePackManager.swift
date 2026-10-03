import CryptoKit
import Foundation

/// The on-device voice pack (the native engine's `small` pack minus Smart
/// Turn, which iOS doesn't run): Silero VAD + Moonshine tiny EN. ~30 MB,
/// downloaded on first use of "This device" voice — never bundled in the app.
///
/// Same files, same sha256 pins as `services/voice/src/models.rs` (`small`),
/// so the Rust engine and the iOS engine run byte-identical models.
struct VoicePackFile: Sendable {
    let asset: String
    let sha256: String
    let size: Int64
    var isArchive: Bool { asset.hasSuffix(".tar.bz2") }
    var fileName: String { String(asset.split(separator: "/").last ?? "") }
}

enum VoicePackSpec {
    /// Release-asset base (Phase 2 moves hosting to runtime.allternit.com;
    /// `ALLTERNIT_VOICE_MODEL_BASE` in Info.plist/env overrides it, like the
    /// native service).
    static let defaultBase = "https://github.com/k2-fsa/sherpa-onnx/releases/download"

    static let vad = VoicePackFile(
        asset: "asr-models/silero_vad.onnx",
        sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
        size: 643_854)
    static let moonshine = VoicePackFile(
        asset: "asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
        sha256: "9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d",
        size: 29_858_559)
    static let files = [vad, moonshine]
    static let moonshineDir = "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27"
    static var totalBytes: Int64 { files.reduce(0) { $0 + $1.size } }
}

enum VoicePackState: Equatable, Sendable {
    case missing
    case downloading(progress: Double)
    case ready
    case failed(String)
}

/// Resolved model paths for the local engine.
struct VoicePackPaths: Sendable, Equatable {
    let vad: String
    let encoder: String
    let decoder: String
    let tokens: String
}

@MainActor
final class VoicePackManager: ObservableObject {
    static let shared = VoicePackManager()

    @Published private(set) var state: VoicePackState = .missing

    private let root: URL
    private var downloadTask: Task<Void, Never>?

    init(root: URL? = nil) {
        self.root = root ?? FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("voice/small", isDirectory: true)
        state = Self.isInstalled(at: self.root) ? .ready : .missing
    }

    var isReady: Bool { state == .ready }

    /// Paths to the model files; nil until the pack is installed.
    var paths: VoicePackPaths? { Self.paths(at: root) }

    nonisolated static func paths(at root: URL) -> VoicePackPaths? {
        guard isInstalled(at: root) else { return nil }
        let model = root.appendingPathComponent(VoicePackSpec.moonshineDir)
        return VoicePackPaths(
            vad: root.appendingPathComponent("silero_vad.onnx").path,
            encoder: model.appendingPathComponent("encoder_model.ort").path,
            decoder: model.appendingPathComponent("decoder_model_merged.ort").path,
            tokens: model.appendingPathComponent("tokens.txt").path)
    }

    nonisolated static func isInstalled(at root: URL) -> Bool {
        let fm = FileManager.default
        let model = root.appendingPathComponent(VoicePackSpec.moonshineDir)
        return fm.fileExists(atPath: root.appendingPathComponent(".ready").path)
            && fm.fileExists(atPath: root.appendingPathComponent("silero_vad.onnx").path)
            && fm.fileExists(atPath: model.appendingPathComponent("encoder_model.ort").path)
            && fm.fileExists(atPath: model.appendingPathComponent("decoder_model_merged.ort").path)
            && fm.fileExists(atPath: model.appendingPathComponent("tokens.txt").path)
    }

    /// Downloads and installs the pack. Idempotent: a second call while one
    /// is running awaits the same download.
    func install() async {
        if isReady { return }
        if downloadTask == nil {
            state = .downloading(progress: 0)
            let root = self.root
            downloadTask = Task { [weak self] in
                let weakSelf = self
                do {
                    try await Self.download(into: root) { progress in
                        Task { @MainActor in weakSelf?.state = .downloading(progress: progress) }
                    }
                    self?.state = .ready
                } catch is CancellationError {
                    self?.state = .missing
                } catch {
                    self?.state = .failed(error.localizedDescription)
                }
                self?.downloadTask = nil
            }
        }
        await downloadTask?.value
    }

    func cancel() { downloadTask?.cancel() }

    /// Removes the pack (Settings → free the space).
    func remove() {
        downloadTask?.cancel()
        try? FileManager.default.removeItem(at: root)
        state = .missing
    }

    // MARK: - Download

    private static func baseURL() -> String {
        if let override = Bundle.main.object(forInfoDictionaryKey: "ALLTERNIT_VOICE_MODEL_BASE") as? String,
           !override.isEmpty, !override.hasPrefix("$(") {
            return override.hasSuffix("/") ? String(override.dropLast()) : override
        }
        return VoicePackSpec.defaultBase
    }

    private static func download(into root: URL, progress: @escaping @Sendable (Double) -> Void) async throws {
        let fm = FileManager.default
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        var excluded = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? excluded.setResourceValues(values)
        try? fm.removeItem(at: root.appendingPathComponent(".ready"))

        var finishedBytes: Int64 = 0
        let total = VoicePackSpec.totalBytes
        for file in VoicePackSpec.files {
            try Task.checkCancellation()
            guard let url = URL(string: "\(baseURL())/\(file.asset)") else { throw VoicePackError.badURL }
            let base = finishedBytes
            let delegate = ProgressDelegate { written in
                progress(min(1, Double(base + written) / Double(total)))
            }
            let (tmp, response) = try await URLSession.shared.download(from: url, delegate: delegate)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                try? fm.removeItem(at: tmp)
                throw VoicePackError.http((response as? HTTPURLResponse)?.statusCode ?? 0)
            }
            guard try sha256(of: tmp) == file.sha256 else {
                try? fm.removeItem(at: tmp)
                throw VoicePackError.checksum(file.fileName)
            }
            if file.isArchive {
                defer { try? fm.removeItem(at: tmp) }
                let staging = root.appendingPathComponent(".staging", isDirectory: true)
                try? fm.removeItem(at: staging)
                try Bz2Tar.extract(archive: tmp, to: staging)
                let target = root.appendingPathComponent(VoicePackSpec.moonshineDir)
                try? fm.removeItem(at: target)
                try fm.moveItem(at: staging.appendingPathComponent(VoicePackSpec.moonshineDir), to: target)
                try? fm.removeItem(at: staging)
            } else {
                let target = root.appendingPathComponent(file.fileName)
                try? fm.removeItem(at: target)
                try fm.moveItem(at: tmp, to: target)
            }
            finishedBytes += file.size
            progress(min(1, Double(finishedBytes) / Double(total)))
        }
        guard Self.paths(atUnmarked: root) else { throw VoicePackError.incomplete }
        fm.createFile(atPath: root.appendingPathComponent(".ready").path, contents: Data())
    }

    private nonisolated static func paths(atUnmarked root: URL) -> Bool {
        let fm = FileManager.default
        let model = root.appendingPathComponent(VoicePackSpec.moonshineDir)
        return fm.fileExists(atPath: root.appendingPathComponent("silero_vad.onnx").path)
            && fm.fileExists(atPath: model.appendingPathComponent("encoder_model.ort").path)
            && fm.fileExists(atPath: model.appendingPathComponent("decoder_model_merged.ort").path)
            && fm.fileExists(atPath: model.appendingPathComponent("tokens.txt").path)
    }

    nonisolated static func sha256(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

private final class ProgressDelegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let onWritten: @Sendable (Int64) -> Void
    init(onWritten: @escaping @Sendable (Int64) -> Void) { self.onWritten = onWritten }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        onWritten(totalBytesWritten)
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
}

enum VoicePackError: Error, LocalizedError {
    case badURL
    case http(Int)
    case checksum(String)
    case incomplete

    var errorDescription: String? {
        switch self {
        case .badURL: return "The voice model address is invalid."
        case .http(let code): return "The voice model download failed (HTTP \(code))."
        case .checksum(let name): return "The voice model \(name) failed its integrity check."
        case .incomplete: return "The voice model download was incomplete."
        }
    }
}
