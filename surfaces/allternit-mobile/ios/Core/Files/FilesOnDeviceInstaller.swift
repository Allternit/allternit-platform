import Foundation

/// `FilesModelImporter`'s link to the on-device models lane: its iPhone
/// storage, `OnDeviceStorage` (`Core/OnDevice/OnDeviceStorage.swift`, platform
/// PR #2437). This is the only Files file that names those types, so if the
/// on-device lane changes its storage, only this file follows.
struct FilesOnDeviceInstaller: FilesOnDeviceInstalling {
    func engine(forWeightsFormat format: String) -> String? {
        switch format.lowercased() {
        case "gguf": return QuantEngine.llamaCpp.rawValue
        case "mlx": return QuantEngine.mlx.rawValue
        default: return nil
        }
    }

    func partialFolder(modelId: String, quant: String) -> URL {
        OnDeviceStorage.partialFolder(modelId: modelId, quant: quant)
    }

    var freeDiskBytes: UInt64 { UInt64(max(0, OnDeviceStorage.freeDiskBytes)) }

    /// `verified` is true: every file matched the sha256 the owner's own
    /// computer sent in the hand-off.
    func commit(modelId: String, quant: String, engine: String, manifestId: String, files: [String], sizeBytes: Int64) throws {
        guard let engine = QuantEngine(rawValue: engine) else { return }
        try OnDeviceStorage.commit(OnDeviceInstall(
            modelId: modelId, quant: quant, engine: engine, manifestId: manifestId,
            files: files, sizeBytes: sizeBytes, verified: true, installedAt: Date()
        ))
    }
}
