import Combine
import Foundation
import OnDeviceLLM
import os
#if canImport(UIKit)
import UIKit
#endif

private let rtLog = Logger(subsystem: "com.allternit.app", category: "ondevice")

enum OnDeviceRuntimeError: Error, CustomStringConvertible, Equatable {
    /// The fit says no: never load it (the screens show "Won't fit").
    case wontFit(String)
    case noBuild
    case backgrounded
    case notInstalled

    var description: String {
        switch self {
        case let .wontFit(why): return "Won't fit. \(why)"
        case .noBuild: return "No build for this device"
        case .backgrounded: return "Models run only while Allternit is open"
        case .notInstalled: return "This model is not downloaded"
        }
    }
}

/// The iPhone's local model runtime: three engines (MLX, llama.cpp, Apple
/// Foundation Models), the measured budget, downloads, and the lifecycle
/// rules. One model in memory at a time.
///
/// Lifecycle: unloads when the app goes to the background (iOS forbids GPU
/// work there and jetsam would kill a big process), on a memory warning, and
/// after 5 idle minutes. A model whose need exceeds the budget is never
/// loaded.
@MainActor
final class OnDeviceRuntime: ObservableObject {
    static let shared = OnDeviceRuntime()

    struct Loaded: Equatable {
        var install: OnDeviceInstall?   // nil for Apple Foundation Models
        var engine: EngineKind
        var memBytes: Int64
        var loadedAt: Date
    }

    static let idleUnloadSeconds = 300
    static let mlxVersion = "mlx-swift 0.30.6, mlx-swift-lm 2.30.6"
    static let llamaVersion = "llama.cpp b12414"

    @Published private(set) var budget: DeviceBudget
    @Published private(set) var installs: [OnDeviceInstall] = []
    @Published private(set) var progress: [String: InstallProgress] = [:]
    @Published private(set) var loaded: Loaded?
    @Published private(set) var lastStats: GenerationStats?
    @Published private(set) var ctxUsed = 0

    let contextLength = 4096
    private let mlx = MLXEngine()
    private let llama = LlamaEngine()
    private let appleFM = AppleFMEngine()
    private var generation: Task<Void, Never>?
    private var idleTimer: Timer?
    private var observers: [NSObjectProtocol] = []

    private init() {
        budget = OnDeviceBudget.budget()
        installs = OnDeviceStorage.installed()
        let downloader = OnDeviceDownloader.shared
        downloader.onProgress = { p in
            Task { @MainActor in OnDeviceRuntime.shared.progress["\(p.modelId)#\(p.quant)"] = p }
        }
        downloader.activate()
        rtLog.info("budget \(Int64(self.budget.fastBytes)) bytes (0.75 x cold available \(OnDeviceBudget.coldAvailableBytes())), \(self.installs.count) installed")
        #if canImport(UIKit)
        let c = NotificationCenter.default
        observers.append(c.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in await OnDeviceRuntime.shared.handleBackground() }
        })
        observers.append(c.addObserver(forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in await OnDeviceRuntime.shared.unload(reason: "memory warning") }
        })
        #endif
    }

    // MARK: Fit

    var isSimulator: Bool {
        #if targetEnvironment(simulator)
        return true
        #else
        return false
        #endif
    }

    /// The fit for a model's device.json quants on this iPhone. Quants for an
    /// engine this build cannot run are dropped first, so MLX-only models on
    /// a build without MLX show "No build for this device".
    func fit(_ quants: [Quant]?) -> FitResult {
        Fit.fit(budget, quants.map { $0.filter(engineAvailable) })
    }

    private func engineAvailable(_ q: Quant) -> Bool {
        switch q.engine {
        case .mlx: return !isSimulator
        case .llamaCpp: return LlamaEngine.isLinked
        case .webgpu: return false
        }
    }

    // MARK: Installs

    /// Downloads the quant the fit picks. Refuses "Won't fit" and "No build".
    @discardableResult
    func install(modelId: String, quants: [Quant]?) async throws -> OnDeviceInstall {
        let r = fit(quants)
        guard let q = r.quant else { throw OnDeviceRuntimeError.noBuild }
        guard r.k == .ok || r.k == .slow else { throw OnDeviceRuntimeError.wontFit(r.why) }
        return try await install(modelId: modelId, quant: q)
    }

    /// Downloads one specific quant (after the caller checked its fit).
    @discardableResult
    func install(modelId: String, quant: Quant) async throws -> OnDeviceInstall {
        guard Fit.needBytes(quant) <= budget.fastBytes else {
            throw OnDeviceRuntimeError.wontFit(Fit.fit(budget, [quant]).why)
        }
        let rec = try await OnDeviceDownloader.shared.install(modelId: modelId, quant: quant)
        refreshInstalls()
        return rec
    }

    func cancelInstall(modelId: String, quant: String) {
        OnDeviceDownloader.shared.cancel(modelId: modelId, quant: quant)
        progress["\(modelId)#\(quant)"] = nil
    }

    func delete(_ install: OnDeviceInstall) async {
        if loaded?.install?.id == install.id { await unload(reason: "deleted") }
        OnDeviceStorage.delete(modelId: install.modelId, quant: install.quant)
        progress[install.id] = nil
        refreshInstalls()
    }

    func refreshInstalls() { installs = OnDeviceStorage.installed() }

    var storageBytes: Int64 { OnDeviceStorage.totalBytes }
    var freeDiskBytes: Int64 { OnDeviceStorage.freeDiskBytes }
    func sizeOnDisk(_ install: OnDeviceInstall) -> Int64 {
        OnDeviceStorage.bytes(at: OnDeviceStorage.folder(modelId: install.modelId, quant: install.quant))
    }

    // MARK: Load / unload

    private func engine(_ kind: EngineKind) -> LLMEngine {
        switch kind {
        case .mlx: return mlx
        case .llamaCpp: return llama
        case .appleFM: return appleFM
        }
    }

    private var isActive: Bool {
        #if canImport(UIKit)
        return UIApplication.shared.applicationState != .background
        #else
        return true
        #endif
    }

    /// Loads an installed, verified quant. Never loads one whose need
    /// exceeds the budget.
    func load(_ install: OnDeviceInstall) async throws {
        guard install.verified else { throw OnDeviceRuntimeError.notInstalled }
        guard isActive else { throw OnDeviceRuntimeError.backgrounded }
        let need = Double(install.sizeBytes) * 1.2 + 0.4e9
        guard need <= budget.fastBytes else {
            let q = Quant(name: install.quant, format: install.engine == .mlx ? "mlx" : "gguf", engine: install.engine,
                          size: Double(install.sizeBytes))
            throw OnDeviceRuntimeError.wontFit(Fit.fit(budget, [q]).why)
        }
        if loaded?.install?.id == install.id { touch(); return }
        await unload(reason: "switching model")
        let kind: EngineKind = install.engine == .mlx ? .mlx : .llamaCpp
        let before = OnDeviceBudget.footprintBytes()
        let t0 = Date()
        try await engine(kind).load(from: OnDeviceStorage.loadLocation(install))
        let mem = max(0, OnDeviceBudget.footprintBytes() - before)
        loaded = Loaded(install: install, engine: kind, memBytes: mem, loadedAt: Date())
        rtLog.info("loaded \(install.modelId, privacy: .public) \(install.quant, privacy: .public) on \(kind.rawValue, privacy: .public) in \(String(format: "%.1f", Date().timeIntervalSince(t0)), privacy: .public)s, +\(mem / 1_000_000) MB, available now \(OnDeviceBudget.availableNow() / 1_000_000) MB")
        touch()
    }

    /// Apple Foundation Models: nothing to download or fit.
    func loadAppleFoundationModel() async throws {
        guard isActive else { throw OnDeviceRuntimeError.backgrounded }
        await unload(reason: "switching model")
        try await appleFM.load(from: nil)
        loaded = Loaded(install: nil, engine: .appleFM, memBytes: 0, loadedAt: Date())
        rtLog.info("loaded Apple Foundation Model")
        touch()
    }

    func unload(reason: String) async {
        generation?.cancel()
        generation = nil
        idleTimer?.invalidate()
        idleTimer = nil
        guard let current = loaded else { return }
        await engine(current.engine).unload()
        loaded = nil
        ctxUsed = 0
        rtLog.info("unloaded \(current.install?.modelId ?? "apple-fm", privacy: .public) (\(reason, privacy: .public)), footprint now \(OnDeviceBudget.footprintBytes() / 1_000_000) MB")
    }

    private func handleBackground() async {
        guard loaded != nil else { return }
        #if canImport(UIKit)
        // Finish the unload before iOS suspends the app.
        var taskId = UIBackgroundTaskIdentifier.invalid
        taskId = UIApplication.shared.beginBackgroundTask(withName: "ondevice-unload") {
            UIApplication.shared.endBackgroundTask(taskId)
        }
        await unload(reason: "app in background")
        UIApplication.shared.endBackgroundTask(taskId)
        #else
        await unload(reason: "app in background")
        #endif
    }

    /// Restarts the idle clock (5 minutes).
    private func touch() {
        idleTimer?.invalidate()
        idleTimer = Timer.scheduledTimer(withTimeInterval: TimeInterval(Self.idleUnloadSeconds), repeats: false) { _ in
            Task { @MainActor in await OnDeviceRuntime.shared.unload(reason: "idle \(OnDeviceRuntime.idleUnloadSeconds)s") }
        }
    }

    // MARK: Generate

    /// Streams a reply from the loaded model. tok/s is logged and kept in
    /// `lastStats`. A new call cancels the previous one.
    func generate(_ turns: [ChatTurn], maxTokens: Int = 512, temperature: Float = 0.6) -> AsyncThrowingStream<String, Error> {
        AsyncThrowingStream { continuation in
            guard let current = loaded else { continuation.finish(throwing: EngineError.notLoaded); return }
            guard isActive else { continuation.finish(throwing: OnDeviceRuntimeError.backgrounded); return }
            generation?.cancel()
            idleTimer?.invalidate()
            let eng = engine(current.engine)
            let opts = GenerationOptions(maxTokens: maxTokens, temperature: temperature, contextLength: contextLength)
            let name = current.install.map { "\($0.modelId) \($0.quant)" } ?? "apple-fm"
            let task = Task { @MainActor [weak self] in
                do {
                    for try await event in eng.generate(turns, options: opts) {
                        if Task.isCancelled { break }
                        switch event {
                        case let .text(t): continuation.yield(t)
                        case let .done(stats):
                            self?.lastStats = stats
                            self?.ctxUsed = stats.promptTokens + stats.generatedTokens
                            rtLog.info("generated \(stats.generatedTokens) tokens at \(String(format: "%.1f", stats.tokensPerSecond), privacy: .public) tok/s (prompt \(stats.promptTokens) tokens in \(String(format: "%.2f", stats.promptSeconds), privacy: .public)s) on \(current.engine.rawValue, privacy: .public) \(name, privacy: .public)")
                        }
                    }
                    continuation.finish()
                } catch {
                    rtLog.error("generation failed: \(String(describing: error), privacy: .public)")
                    continuation.finish(throwing: error)
                }
                self?.touch()
            }
            generation = task
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: DeviceRuntime mirror

    private var deviceName: String {
        #if canImport(UIKit)
        return UIDevice.current.name
        #else
        return Host.current().localizedName ?? "This Mac"
        #endif
    }

    private func models(_ engine: QuantEngine) -> [DeviceRuntimeModel] {
        installs.filter { $0.engine == engine }.map {
            DeviceRuntimeModel(id: $0.modelId, providerId: "on-device", runtimeModelId: "\($0.modelId)#\($0.quant)",
                               name: $0.modelId.split(separator: "/").last.map(String.init) ?? $0.modelId,
                               sizeBytes: $0.sizeBytes, digest: nil, quant: $0.quant, engine: $0.engine.rawValue,
                               verified: $0.verified)
        }
    }

    private func running(_ kind: EngineKind) -> RunningModel? {
        guard let l = loaded, l.engine == kind else { return nil }
        return RunningModel(modelId: l.install?.modelId ?? "apple/foundation-model", quant: l.install?.quant ?? "system",
                            memBytes: l.memBytes, tokPerSec: lastStats?.tokensPerSecond ?? 0,
                            ctxUsed: ctxUsed, ctxMax: contextLength, idleUnloadSec: Self.idleUnloadSeconds)
    }

    /// One entry per engine, for the Models screens and the chat picker
    /// ("On this iPhone").
    var runtimes: [DeviceRuntime] {
        let fm = AppleFMEngine.availability
        let fmState: DeviceRuntimeState
        switch fm {
        case .available: fmState = .ready
        case .appleIntelligenceOff: fmState = .off
        case .modelNotReady: fmState = .missing
        default: fmState = .unsupported
        }
        return [
            DeviceRuntime(id: .iosMLX, place: .thisDevice, deviceName: deviceName, engine: "mlx", backend: "metal",
                          version: Self.mlxVersion, state: isSimulator ? .unsupported : .ready, budget: budget,
                          offline: true, models: models(.mlx), running: running(.mlx),
                          note: isSimulator ? "MLX needs a real iPhone" : nil),
            DeviceRuntime(id: .iosLlama, place: .thisDevice, deviceName: deviceName, engine: "llama.cpp", backend: "metal",
                          version: Self.llamaVersion, state: LlamaEngine.isLinked ? .ready : .missing, budget: budget,
                          offline: true, models: models(.llamaCpp), running: running(.llamaCpp),
                          note: LlamaEngine.isLinked ? nil : "llama.cpp is not in this build"),
            DeviceRuntime(id: .appleFM, place: .thisDevice, deviceName: deviceName, engine: "apple-fm", backend: nil,
                          version: nil, state: fmState, budget: budget, offline: true,
                          models: fmState == .ready ? [DeviceRuntimeModel(
                              id: "apple/foundation-model", providerId: "on-device", runtimeModelId: "apple-fm",
                              name: "Apple Foundation Model", sizeBytes: nil, digest: nil, quant: "system",
                              engine: "apple-fm", verified: true)] : [],
                          running: running(.appleFM), note: fm.reason),
        ]
    }
}
