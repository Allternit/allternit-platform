import Foundation
import MLX
import MLXLLM
import MLXLMCommon

/// MLX engine (mlx-swift-lm). Loads an mlx-community style folder
/// (config.json, *.safetensors, tokenizer files) from local disk only: it
/// never calls the Hugging Face hub, downloads are the caller's job.
public final class MLXEngine: LLMEngine, @unchecked Sendable {
    public let kind = EngineKind.mlx
    private let state = State()

    /// GPU buffer cache MLX may keep between calls. Small on phones: the
    /// cache counts against the app's memory limit.
    private let cacheLimitBytes: Int

    public init(cacheLimitBytes: Int = 64 << 20) {
        self.cacheLimitBytes = cacheLimitBytes
    }

    /// `model_type` values this mlx-swift-lm build can run. A model whose
    /// config.json names another type gets "No build for this device"
    /// instead of a failed load.
    /// Copied from mlx-swift-lm 2.30.6 `LLMTypeRegistry` (its table is not
    /// public). Update together with the mlx-swift-lm pin. Note: no
    /// `qwen3_5` before mlx-swift-lm 2.31.3.
    public static let supportedModelTypes: Set<String> = [
        "mistral", "llama", "phi", "phi3", "phimoe", "gemma", "gemma2", "gemma3", "gemma3_text", "gemma3n",
        "qwen2", "qwen3", "qwen3_moe", "qwen3_next", "minicpm", "starcoder2", "cohere", "openelm", "internlm2",
        "deepseek_v3", "granite", "granitemoehybrid", "mimo", "mimo_v2_flash", "minimax", "glm4", "glm4_moe",
        "glm4_moe_lite", "acereason", "falcon_h1", "bitnet", "smollm3", "ernie4_5", "lfm2", "baichuan_m1",
        "exaone4", "gpt_oss", "lille-130m", "olmoe", "olmo2", "olmo3", "bailing_moe", "lfm2_moe", "nanochat",
        "nemotron_h", "afmoe", "jamba_3b", "mistral3", "apertus",
    ]

    public static func supports(modelType: String) -> Bool { supportedModelTypes.contains(modelType) }

    /// Reads `model_type` from a downloaded folder's config.json.
    public static func modelType(in folder: URL) -> String? {
        guard let data = try? Data(contentsOf: folder.appendingPathComponent("config.json")),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return obj["model_type"] as? String
    }

    public var isLoaded: Bool { get async { await state.container != nil } }

    public func load(from location: URL?) async throws {
        guard let location else { throw EngineError.loadFailed("no folder") }
        await unload()
        if let type = Self.modelType(in: location), !Self.supports(modelType: type) {
            throw EngineError.unavailable("This MLX build cannot run \(type) models")
        }
        MLX.Memory.cacheLimit = cacheLimitBytes
        do {
            let container = try await LLMModelFactory.shared.loadContainer(configuration: ModelConfiguration(directory: location))
            await state.set(container)
        } catch {
            throw EngineError.loadFailed(String(describing: error))
        }
    }

    public func unload() async {
        await state.set(nil)
        MLX.Memory.clearCache()
    }

    public func generate(_ turns: [ChatTurn], options: GenerationOptions) -> AsyncThrowingStream<GenerationEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                guard let container = await state.container else {
                    continuation.finish(throwing: EngineError.notLoaded); return
                }
                do {
                    let chat: [Chat.Message] = turns.map {
                        switch $0.role {
                        case .system: return .system($0.content)
                        case .user: return .user($0.content)
                        case .assistant: return .assistant($0.content)
                        }
                    }
                    let input = try await container.prepare(input: UserInput(chat: chat))
                    let params = GenerateParameters(
                        maxTokens: options.maxTokens, maxKVSize: options.contextLength,
                        temperature: options.temperature, topP: options.topP)
                    let stream = try await container.generate(input: input, parameters: params)
                    for await event in stream {
                        if Task.isCancelled { break }
                        switch event {
                        case let .chunk(text): continuation.yield(.text(text))
                        case let .info(info):
                            continuation.yield(.done(GenerationStats(
                                promptTokens: info.promptTokenCount, generatedTokens: info.generationTokenCount,
                                promptSeconds: info.promptTime, generateSeconds: info.generateTime)))
                        default: break
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: EngineError.generationFailed(String(describing: error)))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private actor State {
        var container: ModelContainer?
        func set(_ c: ModelContainer?) { container = c }
    }
}
