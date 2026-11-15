import CryptoKit
import Foundation

/// A Freeweights manifest (`https://freeweights.allternit.com/manifests/<id>.json`):
/// every file with its size and sha256, the license, and where to fetch it.
/// Decode only after `Minisign.verifyFreeweights` passed on the exact bytes.
public struct FreeweightsManifest: Codable, Sendable, Equatable {
    public struct File: Codable, Sendable, Equatable {
        public let path: String
        public let sha256: String
        public let size: Int64
        public let lfs: Bool?
    }
    public struct Source: Codable, Sendable, Equatable {
        public let type: String
        public let url: String
        public let priority: Int?
    }
    public struct License: Codable, Sendable, Equatable {
        public let id: String?
        public let name: String?
        public let redistribution: String?
    }

    public let id: String
    public let files: [File]
    public let sources: [Source]
    public let license: License?
    public let created_at: String?

    public func file(_ path: String) -> File? { files.first { $0.path == path } }

    /// Source base URLs, best first. Each ends with "/" and takes a file path.
    public var baseURLs: [URL] {
        sources.sorted { ($0.priority ?? 0) > ($1.priority ?? 0) }.compactMap { URL(string: $0.url) }
    }

    public static let origin = URL(string: "https://freeweights.allternit.com")!
    public static func manifestURL(id: String) -> URL { origin.appendingPathComponent("manifests/\(id).json") }
    public static func signatureURL(id: String) -> URL { origin.appendingPathComponent("manifests/\(id).json.minisig") }

    /// Checks the signature over `data`, then decodes it and checks the id.
    public static func verified(data: Data, signature: String, expectedId: String) throws -> FreeweightsManifest {
        try Minisign.verifyFreeweights(message: data, signatureFile: signature)
        let m = try JSONDecoder().decode(FreeweightsManifest.self, from: data)
        guard m.id == expectedId else { throw ManifestError.idMismatch(expected: expectedId, got: m.id) }
        return m
    }
}

public enum ManifestError: Error, Equatable, CustomStringConvertible {
    case idMismatch(expected: String, got: String)
    case fileNotInManifest(String)
    case sizeMismatch(path: String, expected: Int64, got: Int64)
    case hashMismatch(path: String)

    public var description: String {
        switch self {
        case let .idMismatch(e, g): return "Manifest is for \(g), expected \(e)"
        case let .fileNotInManifest(p): return "\(p) is not in the signed manifest"
        case let .sizeMismatch(p, e, g): return "\(p) is \(g) bytes, the manifest says \(e)"
        case let .hashMismatch(p): return "\(p) does not match the manifest's sha256"
        }
    }
}

public enum FileHash {
    /// Streams the file through SHA-256 (8 MB reads, constant memory).
    public static func sha256Hex(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while true {
            let chunk = try autoreleasepool { try handle.read(upToCount: 8 << 20) }
            guard let chunk, !chunk.isEmpty else { break }
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// Size and sha256 must both match the manifest entry.
    public static func verify(_ url: URL, against file: FreeweightsManifest.File) throws {
        let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? -1
        guard size == file.size else { throw ManifestError.sizeMismatch(path: file.path, expected: file.size, got: size) }
        guard try sha256Hex(of: url) == file.sha256.lowercased() else { throw ManifestError.hashMismatch(path: file.path) }
    }
}
