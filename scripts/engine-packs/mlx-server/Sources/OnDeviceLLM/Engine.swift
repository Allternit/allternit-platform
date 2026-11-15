// OnDeviceLLM: the MLX engine core shared by the Mac sidecar and iOS.
//
// Loads a model from a local directory (config.json, *.safetensors,
// tokenizer files) that the caller already downloaded and verified; it never
// fetches from Hugging Face itself. Streams a chat reply with token stats.
// No networking, no UI: iOS (W7) wraps it in its own runtime, the Mac wraps
// it in allternit-mlx-server.

import Foundation
import MLX
import MLXLLM
import MLXLMCommon

public struct ChatTurn: Sendable, Equatable {
    public enum Role: String, Sendable { case system, user, assistant }
    public var role: Role
    public var content: String
    public init(role: Role, content: String) {
        self.role = role
        self.content = content
    }
}

public struct GenerationOptions: Sendable {
    public var maxTokens: Int?
    public var temperature: Float
    public var topP: Float
    /// Upper bound on the KV cache (context) in tokens; nil = model default.
    public var maxContextTokens: Int?
    public init(maxTokens: Int? = nil, temperature: Float = 0.7, topP: Float = 1.0, maxContextTokens: Int? = nil) {
        self.maxTokens = maxTokens
        self.temperature = temperature
        self.topP = topP
        self.maxContextTokens = maxContextTokens
    }
}

public struct GenerationStats: Sendable, Equatable {
    public var promptTokens: Int
    public var completionTokens: Int
    public var promptSeconds: Double
    public var generateSeconds: Double
    /// "stop" (end of text) or "length" (hit maxTokens).
    public var finishReason: String
    public var tokensPerSecond: Double {
        generateSeconds > 0 ? Double(completionTokens) / generateSeconds : 0
    }
    public var promptTokensPerSecond: Double {
        promptSeconds > 0 ? Double(promptTokens) / promptSeconds : 0
    }
}

public enum GenerationEvent: Sendable {
    case text(String)
    case done(GenerationStats)
}

/// One generation at a time per engine: MLX evaluates on one GPU stream, and
/// a second concurrent request would only slow both down.
actor SerialGate {
    private var busy = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if !busy {
            busy = true
            return
        }
        await withCheckedContinuation { waiters.append($0) }
    }

    func release() {
        if waiters.isEmpty {
            busy = false
        } else {
            waiters.removeFirst().resume()
        }
    }
}

public final class OnDeviceLLMEngine: Sendable {
    public let modelId: String
    public let directory: URL
    private let container: ModelContainer
    private let gate = SerialGate()

    private init(modelId: String, directory: URL, container: ModelContainer) {
        self.modelId = modelId
        self.directory = directory
        self.container = container
    }

    /// Load a model from a local directory.
    /// - cacheLimitBytes: MLX buffer cache cap; keeps idle memory near the weights.
    public static func load(directory: URL, modelId: String, cacheLimitBytes: Int = 256 << 20) async throws -> OnDeviceLLMEngine {
        Memory.cacheLimit = cacheLimitBytes
        let config = ModelConfiguration(directory: directory)
        let container = try await LLMModelFactory.shared.loadContainer(configuration: config)
        return OnDeviceLLMEngine(modelId: modelId, directory: directory, container: container)
    }

    /// Bytes MLX holds right now (weights + KV cache + buffer cache).
    public static var memoryBytes: Int {
        Memory.activeMemory + Memory.cacheMemory
    }

    /// Stream a reply. Cancelling the consumer cancels generation.
    public func stream(_ turns: [ChatTurn], options: GenerationOptions = .init()) -> AsyncThrowingStream<GenerationEvent, Error> {
        let (stream, continuation) = AsyncThrowingStream<GenerationEvent, Error>.makeStream()
        let container = self.container
        let gate = self.gate
        let task = Task {
            await gate.acquire()
            do {
                let chat: [Chat.Message] = turns.map { turn in
                    switch turn.role {
                    case .system: return .system(turn.content)
                    case .assistant: return .assistant(turn.content)
                    case .user: return .user(turn.content)
                    }
                }
                let input = try await container.prepare(input: UserInput(chat: chat))
                let parameters = GenerateParameters(
                    maxTokens: options.maxTokens,
                    maxKVSize: options.maxContextTokens,
                    temperature: options.temperature,
                    topP: options.topP
                )
                let generation = try await container.generate(input: input, parameters: parameters)
                for await event in generation {
                    if Task.isCancelled { break }
                    switch event {
                    case .chunk(let text):
                        continuation.yield(.text(text))
                    case .info(let info):
                        continuation.yield(.done(GenerationStats(
                            promptTokens: info.promptTokenCount,
                            completionTokens: info.generationTokenCount,
                            promptSeconds: info.promptTime,
                            generateSeconds: info.generateTime,
                            finishReason: info.stopReason == .length ? "length" : "stop"
                        )))
                    case .toolCall:
                        break
                    }
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
            await gate.release()
        }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }
}
