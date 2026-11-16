import CryptoKit
import Foundation
import AllternitFilesKit

/// Pulls a model package from one of the owner's computers onto this iPhone
/// and gives it to the on-device engine (SPEC section 19, "Hand-off to the
/// on-device import path"):
///
/// 1. `GET /models/handoff/:item_id?execute=true` over the relay;
/// 2. validate the manifest (format, plain name, safe paths, hashes, total,
///    package id) before anything is written;
/// 3. stop unless `grant.execute` is true (run permission is given on the
///    computer the model is on; a transfer-only package stays a file in
///    Files and is never offered to the engine);
/// 4. download every file into `OnDeviceStorage.partialFolder` (ranged,
///    resumable, blake3 checked by `FilesClient.download`) and check its
///    sha256;
/// 5. keep the hand-off as `.allternit-package.json`, then
///    `OnDeviceStorage.commit`, so the model shows under Models > On this
///    iPhone like a catalog download.
///
/// The on-device storage is the on-device models lane's (platform #2437,
/// `Core/OnDevice/OnDeviceStorage.swift`); `FilesOnDeviceInstaller` is the
/// only file here that names it.
enum FilesModelImporter {
    enum Failure: Error, LocalizedError, Equatable {
        case noRunPermission
        case notInstallable(format: String)
        case notEnoughSpace(needed: UInt64, free: UInt64)
        case missingItem(path: String)
        case checksum(path: String)

        var errorDescription: String? {
            switch self {
            case .noRunPermission:
                return "This model can be copied but not run here. Allow it to run on the computer it is on, then try again."
            case .notInstallable(let format):
                return "iPhone runs GGUF and MLX models. This one is \(format)."
            case .notEnoughSpace(let needed, let free):
                let f = ByteCountFormatter()
                return "This model needs \(f.string(fromByteCount: Int64(needed))) and the iPhone has \(f.string(fromByteCount: Int64(free))) free."
            case .missingItem(let path):
                return "The computer did not say where \(path) is."
            case .checksum(let path):
                return "\(path) didn't match its checksum, so it was not installed."
            }
        }
    }

    /// Reads the hand-off without downloading anything (for the confirm step).
    static func handoff(client: FilesClient, itemId: ItemId) async throws -> FilesPackageManifest {
        let manifest: FilesPackageManifest = try await client.json(
            FilesRequest(path: "/models/handoff/\(FilesClient.pathSegment(itemId))",
                         query: [URLQueryItem(name: "execute", value: "true")])
        )
        try manifest.validate()
        return manifest
    }

    /// Everything after the hand-off. `progress` gets bytes done and total.
    static func install(_ manifest: FilesPackageManifest,
                        client: FilesClient,
                        installer: any FilesOnDeviceInstalling = FilesOnDeviceInstaller(),
                        progress: @escaping @Sendable (UInt64, UInt64) -> Void) async throws {
        try manifest.validate()
        guard manifest.grant.execute else { throw Failure.noRunPermission }
        guard let engine = installer.engine(forWeightsFormat: manifest.weightsFormat) else {
            throw Failure.notInstallable(format: manifest.weightsFormat.uppercased())
        }
        let modelId = "files/" + manifest.packageId
        let quant = manifest.name
        let folder = installer.partialFolder(modelId: modelId, quant: quant)
        let free = installer.freeDiskBytes
        let alreadyThere = manifest.files.reduce(UInt64(0)) { sum, file in
            sum + (fileSize(folder.appendingPathComponent(file.path)) == file.size ? file.size : 0)
        }
        // A margin of 512 MB so the phone is never filled to the brim.
        let needed = manifest.totalBytes - alreadyThere + 512 * 1_048_576
        if free < needed { throw Failure.notEnoughSpace(needed: manifest.totalBytes, free: free) }

        var done: UInt64 = 0
        progress(0, manifest.totalBytes)
        for file in manifest.files {
            try Task.checkCancellation()
            let dest = folder.appendingPathComponent(file.path)
            // Resume: a file already landed and checked is kept.
            if fileSize(dest) == file.size, (try? sha256(of: dest)) == file.sha256 {
                done += file.size
                progress(done, manifest.totalBytes)
                continue
            }
            guard let itemId = file.itemId, let rev = file.rev else { throw Failure.missingItem(path: file.path) }
            try FileManager.default.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? FileManager.default.removeItem(at: dest)
            let base = done
            let total = manifest.totalBytes
            try await client.download(itemId: itemId, rev: rev, size: file.size, contentHash: file.contentHash,
                                      to: dest, wake: .whenAutomatic) { bytes, _ in progress(base + bytes, total) }
            guard try sha256(of: dest) == file.sha256 else {
                try? FileManager.default.removeItem(at: dest)
                throw Failure.checksum(path: file.path)
            }
            done += file.size
            progress(done, manifest.totalBytes)
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(manifest).write(to: folder.appendingPathComponent(FilesPackageManifest.marker), options: .atomic)
        try installer.commit(modelId: modelId, quant: quant, engine: engine, manifestId: manifest.packageId,
                             files: manifest.files.map(\.path), sizeBytes: Int64(manifest.totalBytes))
    }

    static func sha256(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let data = try handle.read(upToCount: 4 * 1_048_576), !data.isEmpty {
            hasher.update(data: data)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private static func fileSize(_ url: URL) -> UInt64? {
        (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.uint64Value
    }
}

/// The seam with the on-device models lane, so the import steps above don't
/// depend on its types.
protocol FilesOnDeviceInstalling: Sendable {
    /// The engine name for a package's `weights_format` (`llama.cpp` for
    /// `gguf`, `mlx` for `mlx`), nil when iPhone can't run it.
    func engine(forWeightsFormat format: String) -> String?
    func partialFolder(modelId: String, quant: String) -> URL
    var freeDiskBytes: UInt64 { get }
    func commit(modelId: String, quant: String, engine: String, manifestId: String, files: [String], sizeBytes: Int64) throws
}
