import Foundation

/// Where on-device models live: Application Support/OnDeviceModels, marked
/// excluded from iCloud backup (multi-GB files the user can download again).
///
///   OnDeviceModels/<model id with / as __>/<quant>/      installed files + install.json
///   OnDeviceModels/.partial/<model>/<quant>/            files still downloading
///   OnDeviceModels/.resume/<key>.resume                 URLSession resume data
struct OnDeviceInstall: Codable, Sendable, Equatable, Identifiable {
    var modelId: String
    var quant: String
    var engine: QuantEngine
    /// Freeweights id whose signed manifest the files were checked against.
    var manifestId: String
    var files: [String]
    var sizeBytes: Int64
    var verified: Bool
    var installedAt: Date

    var id: String { "\(modelId)#\(quant)" }
}

enum OnDeviceStorage {
    static let installFile = "install.json"

    static var root: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("OnDeviceModels", isDirectory: true)
        ensure(dir)
        return dir
    }

    /// Creates `dir` and marks it excluded from backup (the flag on a folder
    /// covers everything inside).
    private static func ensure(_ dir: URL) {
        let fm = FileManager.default
        if !fm.fileExists(atPath: dir.path) {
            try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        }
        var u = dir
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? u.setResourceValues(values)
    }

    static func safe(_ s: String) -> String {
        s.replacingOccurrences(of: "/", with: "__").replacingOccurrences(of: ":", with: "_")
    }

    static func folder(modelId: String, quant: String) -> URL {
        root.appendingPathComponent(safe(modelId), isDirectory: true).appendingPathComponent(safe(quant), isDirectory: true)
    }

    static func partialFolder(modelId: String, quant: String) -> URL {
        let d = root.appendingPathComponent(".partial", isDirectory: true)
            .appendingPathComponent(safe(modelId), isDirectory: true).appendingPathComponent(safe(quant), isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    static func resumeFile(key: String) -> URL {
        let d = root.appendingPathComponent(".resume", isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d.appendingPathComponent(safe(key) + ".resume")
    }

    /// What the engine loads: the folder for MLX, the .gguf file for llama.cpp.
    static func loadLocation(_ install: OnDeviceInstall) -> URL {
        let dir = folder(modelId: install.modelId, quant: install.quant)
        if install.engine == .llamaCpp, let gguf = install.files.first(where: { $0.hasSuffix(".gguf") }) {
            return dir.appendingPathComponent(gguf)
        }
        return dir
    }

    static func installed() -> [OnDeviceInstall] {
        let fm = FileManager.default
        let dec = JSONDecoder()
        dec.dateDecodingStrategy = .iso8601
        var out: [OnDeviceInstall] = []
        for model in (try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? [] where !model.lastPathComponent.hasPrefix(".") {
            for quant in (try? fm.contentsOfDirectory(at: model, includingPropertiesForKeys: nil)) ?? [] {
                if let data = try? Data(contentsOf: quant.appendingPathComponent(installFile)),
                   let rec = try? dec.decode(OnDeviceInstall.self, from: data) {
                    out.append(rec)
                }
            }
        }
        return out.sorted { $0.installedAt > $1.installedAt }
    }

    static func install(modelId: String, quant: String) -> OnDeviceInstall? {
        installed().first { $0.modelId == modelId && $0.quant == quant }
    }

    static func write(_ rec: OnDeviceInstall) throws {
        let enc = JSONEncoder()
        enc.dateEncodingStrategy = .iso8601
        enc.outputFormatting = [.prettyPrinted, .sortedKeys]
        try enc.encode(rec).write(to: folder(modelId: rec.modelId, quant: rec.quant).appendingPathComponent(installFile), options: .atomic)
    }

    /// Moves a fully verified partial folder into place and records it.
    static func commit(_ rec: OnDeviceInstall) throws {
        let fm = FileManager.default
        let dest = folder(modelId: rec.modelId, quant: rec.quant)
        try? fm.removeItem(at: dest)
        try fm.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
        try fm.moveItem(at: partialFolder(modelId: rec.modelId, quant: rec.quant), to: dest)
        try write(rec)
    }

    /// Deletes an installed quant and any partial download of it.
    static func delete(modelId: String, quant: String) {
        let fm = FileManager.default
        try? fm.removeItem(at: folder(modelId: modelId, quant: quant))
        try? fm.removeItem(at: partialFolder(modelId: modelId, quant: quant))
        let model = root.appendingPathComponent(safe(modelId), isDirectory: true)
        if (try? fm.contentsOfDirectory(atPath: model.path))?.isEmpty == true { try? fm.removeItem(at: model) }
    }

    /// Bytes on disk under `url` (allocated size, so sparse partials count right).
    static func bytes(at url: URL) -> Int64 {
        let keys: [URLResourceKey] = [.totalFileAllocatedSizeKey, .isRegularFileKey]
        guard let e = FileManager.default.enumerator(at: url, includingPropertiesForKeys: keys) else { return 0 }
        var total: Int64 = 0
        for case let f as URL in e {
            let v = try? f.resourceValues(forKeys: Set(keys))
            if v?.isRegularFile == true { total += Int64(v?.totalFileAllocatedSize ?? 0) }
        }
        return total
    }

    /// Everything the feature holds, including partial downloads.
    static var totalBytes: Int64 { bytes(at: root) }

    static var freeDiskBytes: Int64 {
        let v = try? root.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        return v?.volumeAvailableCapacityForImportantUsage ?? 0
    }
}
