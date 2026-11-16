import Foundation
import AllternitFilesKit

/// `allternit.model-package/1` (contracts/files-drive/SPEC.md section 19):
/// the hand-off a phone reads before it pulls a model, dataset or checkpoint
/// from one of the owner's computers (`GET /models/handoff/:item_id`).
/// Rust source of truth: `cmd/allternit-api/src/files_v2/models.rs`
/// (`PackageManifest`, `package_id`, `validate`). The rules here must match.
struct FilesPackageManifest: Codable, Sendable, Hashable {
    static let format = "allternit.model-package/1"
    /// The permission file inside a package folder.
    static let marker = ".allternit-package.json"
    static let maxFiles = 10_000

    struct File: Codable, Sendable, Hashable {
        var path: String
        var size: UInt64
        /// blake3 of the whole file (the drive's content hash).
        var contentHash: String
        var sha256: String
        var itemId: String?
        var rev: String?

        enum CodingKeys: String, CodingKey {
            case path, size, contentHash = "content_hash", sha256, itemId = "item_id", rev
        }
    }

    struct Grant: Codable, Sendable, Hashable {
        var transfer: Bool
        /// A model engine may load or run it. Never implied by `transfer`.
        var execute: Bool
    }

    struct Source: Codable, Sendable, Hashable {
        var computerId: String
        var itemId: String
        var rev: String

        enum CodingKeys: String, CodingKey { case computerId = "computer_id", itemId = "item_id", rev }
    }

    var format: String
    var packageId: String
    var name: String
    /// `model`, `dataset` or `checkpoint`.
    var kind: String
    /// `gguf`, `mlx`, `safetensors`, `onnx` or `other`.
    var weightsFormat: String
    var totalBytes: UInt64
    var files: [File]
    var grant: Grant
    var source: Source
    var createdAt: String

    enum CodingKeys: String, CodingKey {
        case format, packageId = "package_id", name, kind, weightsFormat = "weights_format"
        case totalBytes = "total_bytes", files, grant, source, createdAt = "created_at"
    }

    struct Invalid: Error, LocalizedError, Equatable {
        let reason: String
        var errorDescription: String? { "This model package can't be used: \(reason)." }
    }

    /// `pkg_` + the first 24 hex of blake3 over the `path\tcontent_hash\n`
    /// lines sorted by their bytes (Rust sorts `String`s byte-wise, which is
    /// not Swift's `String` order for every input).
    static func packageId(for files: [File]) -> String {
        let lines = files.map { "\($0.path)\t\($0.contentHash)\n" }
            .sorted { Array($0.utf8).lexicographicallyPrecedes(Array($1.utf8)) }
        return "pkg_" + String(Blake3Hasher.hashHex(Data(lines.joined().utf8)).prefix(24))
    }

    /// A path the importer can join under its own folder without leaving it.
    static func isSafeRelative(_ path: String) -> Bool {
        guard !path.isEmpty, !path.hasPrefix("/"), !path.contains("\\"), !path.contains("\0") else { return false }
        let parts = path.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else { return false }
        return parts.last.map(String.init) != marker
    }

    private static func isLowerHex(_ value: String, count: Int) -> Bool {
        value.utf8.count == count && value.utf8.allSatisfy { ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102) }
    }

    /// Every rule the receiver checks before it writes anything.
    func validate() throws {
        guard format == Self.format else { throw Invalid(reason: "unknown format \(format)") }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !name.contains("/"), !name.contains("\\"), name != ".", name != ".." else {
            throw Invalid(reason: "the name is not a plain folder name")
        }
        guard !files.isEmpty else { throw Invalid(reason: "it has no files") }
        guard files.count <= Self.maxFiles else { throw Invalid(reason: "it has more than \(Self.maxFiles) files") }
        // Rust compares lower-cased bytes: canonically equal Swift strings
        // (NFC vs NFD) are two different paths there, so compare bytes too.
        var seen = Set<Data>()
        var total: UInt64 = 0
        for file in files {
            guard Self.isSafeRelative(file.path) else { throw Invalid(reason: "\(file.path) is not a safe path") }
            guard seen.insert(Data(file.path.lowercased().utf8)).inserted else { throw Invalid(reason: "\(file.path) is listed twice") }
            guard Self.isLowerHex(file.contentHash, count: 64) else { throw Invalid(reason: "\(file.path) has no blake3 hash") }
            guard Self.isLowerHex(file.sha256, count: 64) else { throw Invalid(reason: "\(file.path) has no sha256") }
            let (sum, overflow) = total.addingReportingOverflow(file.size)
            guard !overflow else { throw Invalid(reason: "the sizes overflow") }
            total = sum
        }
        guard total == totalBytes else { throw Invalid(reason: "the files add up to \(total) bytes, not \(totalBytes)") }
        guard packageId == Self.packageId(for: files) else { throw Invalid(reason: "the package id does not match its files") }
    }
}
