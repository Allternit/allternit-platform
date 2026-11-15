// OnDeviceLab: a debug-only app (bundle com.allternit.dev.ondevicelab,
// target OnDeviceLab in project.yml) that runs the real Core/OnDevice
// runtime on a phone without replacing the TestFlight Allternit app.
// Never archived or shipped. Excluded from the Allternit target.
//
// Launch with arguments to run a scripted check and print results to stdout
// (read with `xcrun devicectl device process launch --console`):
//   --mlx      fit -> download -> verify -> load -> reply (largest MLX quant that fits)
//   --llama    same with a small GGUF on llama.cpp
//   --warn     after the reply, send a memory warning and show the unload
//   --fm       Apple Foundation Models reply (iOS 26+ with Apple Intelligence on)
//   --tts      Kokoro synthesis smoke (KokoroKit on the shared mlx-swift)
// Background the app afterwards (e.g. launch Settings) to see the unload.

import Combine
import KokoroSwift
import MisakiSwift
import MLX
import OnDeviceLLM
import SwiftUI
import UIKit

@main
struct OnDeviceLabApp: App {
    @StateObject private var lab = Lab()
    var body: some Scene {
        WindowGroup {
            ScrollView {
                Text(lab.log).font(.system(.footnote, design: .monospaced)).frame(maxWidth: .infinity, alignment: .leading).padding()
            }
            .task { await lab.run(CommandLine.arguments) }
        }
    }
}

/// Candidate models with their real device.json quants (freeweights
/// catalog/overlay/device.json, C2), as the catalog route would return them.
private enum LabModels {
    static let qwen35_2B = ("lmstudio-community/Qwen3.5-2B-MLX-4bit",
        [Quant(name: "4bit", format: "mlx", engine: .mlx, size: 1_722_271_715,
               files: ["chat_template.jinja", "config.json", "model.safetensors", "model.safetensors.index.json",
                       "preprocessor_config.json", "processor_config.json", "tokenizer.json", "tokenizer_config.json",
                       "video_preprocessor_config.json", "vocab.json"])])
    /// Smallest Qwen3.5 MLX build: the one that fits a 2.24 GB phone budget
    /// (need 1.15 GB). Qwen3.5-2B 4bit above needs 2.47 GB, so it must be
    /// refused on an iPhone 15 without the increased-memory entitlement.
    static let qwen35_0_8B = ("mlx-community/Qwen3.5-0.8B-4bit",
        [Quant(name: "4bit", format: "mlx", engine: .mlx, size: 625_229_487,
               files: ["chat_template.jinja", "config.json", "model.safetensors", "model.safetensors.index.json",
                       "preprocessor_config.json", "processor_config.json", "tokenizer.json", "tokenizer_config.json",
                       "video_preprocessor_config.json", "vocab.json"])])
    static let qwen3_1_7B = ("mlx-community/Qwen3-1.7B-4bit",
        [Quant(name: "4bit", format: "mlx", engine: .mlx, size: 968_080_210,
               files: ["added_tokens.json", "config.json", "merges.txt", "model.safetensors", "model.safetensors.index.json",
                       "special_tokens_map.json", "tokenizer.json", "tokenizer_config.json", "vocab.json"])])
    static let qwen35_0_8B_gguf = ("ggml-org/Qwen3.5-0.8B-GGUF",
        [Quant(name: "Q4_0", format: "gguf", engine: .llamaCpp, size: 563_036_064, files: ["Qwen3.5-0.8B-Q4_0.gguf"]),
         Quant(name: "Q8_0", format: "gguf", engine: .llamaCpp, size: 833_592_096, files: ["Qwen3.5-0.8B-Q8_0.gguf"])])
}

@MainActor
final class Lab: ObservableObject {
    @Published var log = ""
    private var lastPct = -1

    private func say(_ s: String) {
        let line = "[lab] \(s)"
        print(line)
        fflush(stdout)
        log += line + "\n"
    }

    func run(_ args: [String]) async {
        let rt = OnDeviceRuntime.shared
        say("device \(UIDevice.current.model) iOS \(UIDevice.current.systemVersion), physical \(ProcessInfo.processInfo.physicalMemory / 1_000_000) MB")
        say("cold available \(OnDeviceBudget.coldAvailableBytes() / 1_000_000) MB, budget \(Int64(rt.budget.fastBytes) / 1_000_000) MB (0.75x)")
        for r in rt.runtimes { say("runtime \(r.id.rawValue): \(r.state.rawValue) \(r.version ?? "") \(r.note ?? "")") }
        say("storage \(rt.storageBytes / 1_000_000) MB used, \(rt.freeDiskBytes / 1_000_000) MB free, installed \(rt.installs.map(\.id))")

        if args.contains("--mlx") { await chat(models: [LabModels.qwen35_2B, LabModels.qwen35_0_8B, LabModels.qwen3_1_7B], args: args) }
        if args.contains("--llama") { await chat(models: [LabModels.qwen35_0_8B_gguf], args: args) }
        if args.contains("--fm") { await appleFM() }
        if args.contains("--tts") { await tts() }
        if args.contains("--delete") {
            for i in rt.installs { await rt.delete(i); say("deleted \(i.id)") }
        }
        say("done; background the app to see the unload")
    }

    private func chat(models: [(String, [Quant])], args: [String]) async {
        let rt = OnDeviceRuntime.shared
        var chosen: (String, Quant)?
        for (id, quants) in models {
            let r = rt.fit(quants)
            say("fit \(id): \(r.label) (\(r.why)) quant \(r.quant?.name ?? "-")")
            if chosen == nil, let q = r.quant, r.k == .ok || r.k == .slow { chosen = (id, q) }
            if r.k == .no, let q = r.quant {
                do { try await rt.install(modelId: id, quant: q); say("ERROR: install of a Won't-fit model went through") }
                catch { say("install refused as expected: \(error)") }
            }
        }
        guard let (id, quant) = chosen else { say("nothing fits"); return }
        rt.$progress.sink { [weak self] p in
            guard let self, let pr = p["\(id)#\(quant.name)"] else { return }
            let pct = Int(pr.fraction * 100)
            if pct / 10 != self.lastPct / 10 || pr.phase != .downloading {
                self.lastPct = pct
                print("[lab] progress \(pr.phase) \(pct)% \(pr.receivedBytes / 1_000_000)/\(pr.totalBytes / 1_000_000) MB")
            }
        }.store(in: &bag)
        do {
            let t0 = Date()
            let install: OnDeviceInstall
            if let i = rt.installs.first(where: { $0.modelId == id && $0.quant == quant.name }) { install = i }
            else { install = try await rt.install(modelId: id, quant: quant) }
            say("installed \(install.id) verified=\(install.verified) \(install.sizeBytes / 1_000_000) MB in \(Int(Date().timeIntervalSince(t0)))s; on disk \(rt.sizeOnDisk(install) / 1_000_000) MB")
            try await rt.load(install)
            say("loaded, footprint \(OnDeviceBudget.footprintBytes() / 1_000_000) MB")
            var reply = ""
            for try await t in rt.generate([.init(.user, "In one short sentence, what is an iPhone? /no_think")], maxTokens: 96, temperature: 0) {
                reply += t
            }
            say("reply: \(reply.replacingOccurrences(of: "\n", with: " "))")
            if let s = rt.lastStats { say(String(format: "tok/s %.1f (%d tokens, prompt %d in %.2fs)", s.tokensPerSecond, s.generatedTokens, s.promptTokens, s.promptSeconds)) }
            if args.contains("--warn") {
                say("sending a memory warning")
                UIApplication.shared.perform(Selector(("_performMemoryWarning")))
                try? await Task.sleep(for: .seconds(2))
                say("after warning: loaded=\(rt.loaded != nil), footprint \(OnDeviceBudget.footprintBytes() / 1_000_000) MB")
                try await rt.load(install)
                say("reloaded for the background check, footprint \(OnDeviceBudget.footprintBytes() / 1_000_000) MB")
            }
        } catch {
            say("ERROR \(error)")
        }
    }

    private var bag = Set<AnyCancellable>()

    private func appleFM() async {
        let rt = OnDeviceRuntime.shared
        let a = AppleFMEngine.availability
        guard case .available = a else { say("apple-fm: not available (\(a.reason ?? "unknown"))"); return }
        do {
            try await rt.loadAppleFoundationModel()
            var reply = ""
            for try await t in rt.generate([.init(.user, "In one short sentence, what is an iPhone?")], maxTokens: 96, temperature: 0) {
                reply += t
            }
            say("apple-fm reply: \(reply.replacingOccurrences(of: "\n", with: " "))")
            if let s = rt.lastStats { say(String(format: "apple-fm tok/s %.1f (%d tokens)", s.tokensPerSecond, s.generatedTokens)) }
            await rt.unload(reason: "lab done")
        } catch {
            say("apple-fm: ERROR \(error)")
        }
    }

    // MARK: Kokoro smoke

    private func tts() async {
        guard #available(iOS 18.0, *) else { say("tts: Kokoro needs iOS 18"); return }
        let base = "https://runtime.allternit.com/voice-models/ios/v1/"
        let files = ["kokoro/kokoro-v1_0-fp16.safetensors", "kokoro/voices-v1_0-fp16.safetensors",
                     "misaki/us_gold.json", "misaki/us_silver.json", "misaki/us_bart.safetensors", "misaki/us_bart_config.json"]
        let root = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("lab-kokoro")
        do {
            for f in files {
                let dest = root.appendingPathComponent(f)
                if FileManager.default.fileExists(atPath: dest.path) { continue }
                try FileManager.default.createDirectory(at: dest.deletingLastPathComponent(), withIntermediateDirectories: true)
                let (tmp, resp) = try await URLSession.shared.download(from: URL(string: base + f)!)
                guard (resp as? HTTPURLResponse)?.statusCode == 200 else { say("tts: HTTP error for \(f)"); return }
                try FileManager.default.moveItem(at: tmp, to: dest)
            }
            say("tts: pack ready")
            let text = "Hello from Allternit. This voice runs on the phone."
            let samples: [Float] = try await Task.detached {
                MisakiResources.directory = root.appendingPathComponent("misaki")
                MLX.Memory.cacheLimit = 64 << 20
                return try withError {
                    let voices = try MLX.loadArrays(url: root.appendingPathComponent("kokoro/voices-v1_0-fp16.safetensors")).mapValues { $0.asType(.float32) }
                    let tts = KokoroTTS(modelPath: root.appendingPathComponent("kokoro/kokoro-v1_0-fp16.safetensors"), g2p: .misaki)
                    return try tts.generateAudio(voice: voices["af_heart"]!, language: .enUS, text: text, speed: 1).0
                }
            }.value
            let rms = sqrt(samples.reduce(0) { $0 + $1 * $1 } / Float(max(1, samples.count)))
            let peak = samples.map(abs).max() ?? 0
            let secs = Double(samples.count) / Double(KokoroTTS.Constants.samplingRate)
            say(String(format: "tts: %d samples (%.2fs at %d Hz), rms %.4f, peak %.3f", samples.count, secs, KokoroTTS.Constants.samplingRate, rms, peak))
            let wav = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("kokoro-smoke.wav")
            try Self.wav(samples, rate: KokoroTTS.Constants.samplingRate).write(to: wav)
            say("tts: wrote \(wav.path)")
            MLX.Memory.clearCache()
        } catch {
            say("tts: ERROR \(error)")
        }
    }

    private static func wav(_ samples: [Float], rate: Int) -> Data {
        var d = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        let pcm = samples.map { Int16(max(-1, min(1, $0)) * 32767) }
        d.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count * 2)); d.append(contentsOf: Array("WAVEfmt ".utf8))
        u32(16); u16(1); u16(1); u32(UInt32(rate)); u32(UInt32(rate * 2)); u16(2); u16(16)
        d.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count * 2))
        pcm.forEach { withUnsafeBytes(of: $0.littleEndian) { d.append(contentsOf: $0) } }
        return d
    }
}
