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

    enum CodingKeys: String, CodingKey {
        case kind, apple, browser, webgpu
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

enum FitKind: String, Codable, Sendable {
    case ok, slow, no, na, unknown
}

struct FitResult: Sendable, Equatable {
    var k: FitKind
    var label: String
    var why: String
    var quant: Quant?
}

enum Fit {
    private static let gb = 1e9

    /// Memory a quant needs: ram_est, else size x 1.2 + 0.4 GB.
    static func needBytes(_ q: Quant) -> Double {
        q.ramEst ?? q.size * 1.2 + 0.4 * gb
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
    static func fit(_ budget: DeviceBudget, _ quants: [Quant]?, ctx: Int? = nil) -> FitResult {
        guard let quants else {
            return FitResult(k: .unknown, label: "Size not checked", why: "No device data for this model yet", quant: nil)
        }
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
