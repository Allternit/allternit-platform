import Foundation

// "Will this model run on this device." Swift port of the one fit spec:
// spec/on-device/fit.md in the platform repo (canonical, with
// fit-vectors.json). The TypeScript port (allternit-ai
// src/lib/on-device/fit.ts) and the Rust port follow the same file.
// Tests: Tests/OnDevice/FitVectorTests.swift runs every vector.
// Never fit on a model's total repo size: only on device.json quants.

enum DeviceBudgetKind: String, Codable, Sendable {
    case discrete, unified, cpu
}

/// What a device can hold, in bytes. Same shape as local-engine
/// `GET /hardware` `budget`, plus three client-only flags.
/// iPhone: kind unified, apple true, fast_bytes = 0.75 x the cold-start
/// `os_proc_available_memory()` (OnDeviceBudget.swift).
struct DeviceBudget: Codable, Sendable, Equatable {
    var kind: DeviceBudgetKind
    var fastBytes: Double
    var spillBytes: Double
    var apple: Bool?
    var browser: Bool?
    var webgpu: Bool?
    /// Free bytes on the volume that holds the models folder. Set only by a
    /// host that can run a colibri pack (Mac arm64, Windows x64, Linux x64/arm64).
    /// The phone never sets it, so it never gets a `stream` verdict for itself;
    /// a budget for another device (Remote, My other devices) may carry it.
    var diskFreeBytes: Double?

    enum CodingKeys: String, CodingKey {
        case kind, apple, browser, webgpu
        case diskFreeBytes = "disk_free_bytes"
        case fastBytes = "fast_bytes"
        case spillBytes = "spill_bytes"
    }
}

enum QuantEngine: String, Codable, Sendable {
    case llamaCpp = "llama.cpp"
    case mlx
    case webgpu
}

/// One entry of `quants` in a device.json model entry
/// (`GET /api/models/:org/:name/device`).
struct Quant: Codable, Sendable, Equatable, Hashable {
    var name: String
    var format: String
    var engine: QuantEngine
    var size: Double
    var ramEst: Double?
    var files: [String]?
    var bestFor: [String]?
    /// Freeweights id that holds this build (MLX and ONNX builds often live
    /// in a sibling repo). Downloads use its signed manifest.
    var repo: String?
    var mmproj: String?

    enum CodingKeys: String, CodingKey {
        case name, format, engine, size, files, repo, mmproj
        case ramEst = "ram_est"
        case bestFor = "best_for"
    }
}

/// One entry of `stream_quants` in a device.json model entry: a colibri pack
/// that keeps `ram_est` resident and reads the rest from disk. `engine` is a
/// plain String on purpose (always "colibri"); it never goes through
/// `QuantEngine`, which shipped builds decode strictly.
struct StreamQuant: Codable, Sendable, Equatable, Hashable {
    var name: String
    var format: String?
    var engine: String
    /// Download size, must be free on disk.
    var size: Double
    /// Resident weights (not file size). Required: an entry without it is dropped.
    var ramEst: Double
    /// "chat" or "background".
    var tier: String
    var colibriModel: String?
    var repo: String?
    var metalOk: Bool?
    var bestFor: String?

    enum CodingKeys: String, CodingKey {
        case name, format, engine, size, tier, repo
        case ramEst = "ram_est"
        case colibriModel = "colibri_model"
        case metalOk = "metal_ok"
        case bestFor = "best_for"
    }
}

/// Decodes one array element without ever throwing, so a bad element can be
/// dropped while its siblings are kept.
private struct Lossy<T: Decodable>: Decodable {
    let value: T?
    init(from decoder: Decoder) throws {
        value = try? T(from: decoder)
    }
}

extension KeyedDecodingContainer {
    /// An array that drops elements that fail to decode (an unknown engine,
    /// a missing required field) and keeps the rest. Nil when the key is
    /// missing, null, or not an array.
    func decodeLossyArray<T: Decodable>(_ type: T.Type, forKey key: Key) -> [T]? {
        guard let wrapped = try? decodeIfPresent([Lossy<T>].self, forKey: key) else { return nil }
        return wrapped.compactMap(\.value)
    }
}

enum FitKind: String, Codable, Sendable {
    case ok, slow, stream, no, na, unknown
}

struct FitResult: Sendable, Equatable {
    var k: FitKind
    var label: String
    var why: String
    var quant: Quant?
    /// The chosen colibri pack when `k == .stream` (then `quant` is nil).
    var stream: StreamQuant?

    init(k: FitKind, label: String, why: String, quant: Quant?, stream: StreamQuant? = nil) {
        self.k = k; self.label = label; self.why = why; self.quant = quant; self.stream = stream
    }

    /// Name of the chosen build, Quant or StreamQuant.
    var quantName: String? { quant?.name ?? stream?.name }
}

enum Fit {
    private static let gb = 1e9

    /// Memory a quant needs: ram_est, else size x 1.2 + 0.4 GB of native
    /// runtime overhead. webgpu quants skip the 0.4 GB: a browser budget is
    /// a GPU buffer cap, not process memory (fit.md "Need").
    static func needBytes(_ q: Quant) -> Double {
        if let r = q.ramEst { return r }
        return q.engine == .webgpu ? q.size * 1.2 : q.size * 1.2 + 0.4 * gb
    }

    private enum Verdict { case ok, slow, no }

    private static func verdict(_ d: DeviceBudget, _ q: Quant) -> Verdict {
        let need = needBytes(q)
        switch d.kind {
        case .discrete:
            if need * 10 <= d.fastBytes * 9 { return .ok }
            if need * 2 <= d.fastBytes * 2 + d.spillBytes { return .slow }
            return .no
        case .cpu:
            if need > d.fastBytes { return .no }
            return q.size < 1.5 * gb ? .ok : .slow
        case .unified:
            if need * 5 <= d.fastBytes * 4 { return .ok }
            if need <= d.fastBytes { return .slow }
            return .no
        }
    }

    /// Quants this device can run at all: webgpu for browsers, MLX else GGUF
    /// on Apple, GGUF elsewhere.
    static func candidates(_ d: DeviceBudget, _ quants: [Quant]) -> [Quant] {
        if d.browser == true { return quants.filter { $0.engine == .webgpu } }
        let gguf = quants.filter { $0.engine == .llamaCpp }
        if d.apple == true {
            let mlx = quants.filter { $0.engine == .mlx }
            return mlx.isEmpty ? gguf : mlx
        }
        return gguf
    }

    /// GB with one decimal, half away from zero, trailing ".0" dropped.
    private static func gbShort(_ bytes: Double) -> String {
        let v = (bytes / gb * 10).rounded() / 10
        return v == v.rounded() ? String(Int64(v)) : String(format: "%.1f", v)
    }

    /// GB with one decimal, always shown.
    private static func gbFixed(_ bytes: Double) -> String {
        String(format: "%.1f", (bytes / gb * 10).rounded() / 10)
    }

    private static func na(_ d: DeviceBudget) -> FitResult {
        if d.browser == true && d.webgpu == false {
            return FitResult(k: .na, label: "No WebGPU",
                             why: "This browser cannot run models. Use Desktop or the phone app", quant: nil)
        }
        if d.browser == true {
            return FitResult(k: .na, label: "No browser build", why: "Run it in Desktop or the phone app", quant: nil)
        }
        return FitResult(k: .na, label: "No build for this device", why: "Pick another model or device", quant: nil)
    }

    /// `quants == nil` means the model has no device.json entry.
    /// `ctx` is accepted and ignored in v1 (ram_est is at the default context).
    /// `streamQuants` is `stream_quants` from the same entry (fit.md "Stream (colibri)").
    static func fit(_ budget: DeviceBudget, _ quants: [Quant]?, ctx: Int? = nil,
                    streamQuants: [StreamQuant]? = nil) -> FitResult {
        let streams = streamQuants ?? []
        // quants == nil with stream entries is treated as an empty list;
        // unknown only when both are nil or empty.
        guard let quants = quants ?? (streams.isEmpty ? nil : []) else {
            return FitResult(k: .unknown, label: "Size not checked", why: "No device data for this model yet", quant: nil)
        }
        let base = inMemoryFit(budget, quants)
        guard base.k == .no || (base.k == .na && candidates(budget, quants).isEmpty),
              let s = streamPick(budget, streams) else { return base }
        return FitResult(k: .stream, label: "Streams from disk", why: streamWhy(s), quant: nil, stream: s)
    }

    private static func streamWhy(_ s: StreamQuant) -> String {
        s.tier == "chat"
            ? "Keeps \(gbFixed(s.ramEst)) GB in memory, reads the rest from disk"
            : "Slow. Best for jobs you leave running"
    }

    /// The stream step: never for a browser or a budget without disk_free_bytes.
    private static func streamPick(_ d: DeviceBudget, _ streams: [StreamQuant]) -> StreamQuant? {
        if d.browser == true { return nil }
        guard let disk = d.diskFreeBytes else { return nil }
        let ok = streams.filter { s in
            guard s.engine == "colibri", s.size <= disk else { return false }
            switch d.kind {
            case .discrete: return s.ramEst * 10 <= d.spillBytes * 6
            case .unified, .cpu: return s.ramEst <= d.fastBytes
            }
        }
        return preferredStream(ok)
    }

    /// Sort order of fit.md: chat tier first, then larger size, then name (byte order).
    static func preferredStream(_ streams: [StreamQuant]) -> StreamQuant? {
        streams.sorted { a, b in
            if (a.tier == "chat") != (b.tier == "chat") { return a.tier == "chat" }
            if a.size != b.size { return a.size > b.size }
            return Array(a.name.utf8).lexicographicallyPrecedes(Array(b.name.utf8))
        }.first
    }

    private static func inMemoryFit(_ budget: DeviceBudget, _ quants: [Quant]) -> FitResult {
        if budget.browser == true && budget.webgpu == false { return na(budget) }
        let cands = candidates(budget, quants)
        if cands.isEmpty { return na(budget) }

        // Stable sort by need ascending (same order as Array.prototype.sort).
        let bySize = cands.enumerated().sorted {
            let a = needBytes($0.element), b = needBytes($1.element)
            return a != b ? a < b : $0.offset < $1.offset
        }.map(\.element)
        let pick = bySize.last(where: { verdict(budget, $0) == .ok }) ?? bySize[0]

        let need = gbFixed(needBytes(pick))
        let b = gbShort(budget.fastBytes)
        switch verdict(budget, pick) {
        case .ok:
            let why: String
            switch budget.kind {
            case .discrete: why = "Full GPU, \(need) of \(b) GB VRAM"
            case .cpu: why = "CPU only"
            case .unified: why = "\(need) of \(b) GB for models"
            }
            return FitResult(k: .ok, label: "Fits", why: why, quant: pick)
        case .slow:
            switch budget.kind {
            case .discrete: return FitResult(k: .slow, label: "May be slow", why: "Part runs on the CPU", quant: pick)
            case .cpu: return FitResult(k: .slow, label: "May be slow", why: "CPU only, about 5 tok/s", quant: pick)
            case .unified: return FitResult(k: .slow, label: "Tight fit", why: "Close other apps first", quant: pick)
            }
        case .no:
            let why = budget.kind == .unified
                ? "Needs \(need) GB, \(b) GB free for models"
                : "Needs about \(need) GB"
            return FitResult(k: .no, label: "Won't fit", why: why, quant: pick)
        }
    }
}
