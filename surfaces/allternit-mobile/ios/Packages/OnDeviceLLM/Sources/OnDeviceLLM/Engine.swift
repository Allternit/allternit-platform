import Foundation

/// The three on-device engines. Raw values match the device.json `engine`
/// field (`mlx`, `llama.cpp`) and the DeviceRuntime ids (`apple-fm`).
public enum EngineKind: String, Sendable, Codable, CaseIterable {
    case mlx
    case llamaCpp = "llama.cpp"
    case appleFM = "apple-fm"
}

public struct ChatTurn: Sendable, Equatable {
    public enum Role: String, Sendable { case system, user, assistant }
    public let role: Role
    public let content: String
    public init(_ role: Role, _ content: String) { self.role = role; self.content = content }
}

public struct GenerationOptions: Sendable {
    public var maxTokens: Int
    public var temperature: Float
    public var topP: Float
    /// KV / context window in tokens. Phones keep it small: KV cache is the
    /// part of the memory need that grows.
    public var contextLength: Int
    public init(maxTokens: Int = 512, temperature: Float = 0.6, topP: Float = 0.95, contextLength: Int = 4096) {
        self.maxTokens = maxTokens
        self.temperature = temperature
        self.topP = topP
        self.contextLength = contextLength
    }
}

public struct GenerationStats: Sendable, Equatable {
    public let promptTokens: Int
    public let generatedTokens: Int
    /// Time to first token (prompt processing), seconds.
    public let promptSeconds: Double
    public let generateSeconds: Double
    public var tokensPerSecond: Double { generateSeconds > 0 ? Double(generatedTokens) / generateSeconds : 0 }
    public init(promptTokens: Int, generatedTokens: Int, promptSeconds: Double, generateSeconds: Double) {
        self.promptTokens = promptTokens
        self.generatedTokens = generatedTokens
        self.promptSeconds = promptSeconds
        self.generateSeconds = generateSeconds
    }
}

public enum GenerationEvent: Sendable {
    case text(String)
    case done(GenerationStats)
}

public enum EngineError: Error, CustomStringConvertible, Equatable {
    case notLoaded
    case unavailable(String)
    case loadFailed(String)
    case generationFailed(String)

    public var description: String {
        switch self {
        case .notLoaded: return "No model is loaded"
        case let .unavailable(why): return why
        case let .loadFailed(why): return "Could not load the model: \(why)"
        case let .generationFailed(why): return "Generation failed: \(why)"
        }
    }
}

/// One local engine holding at most one model. Engines do no downloading
/// and no lifecycle handling: the caller (the iPhone runtime, or the Mac
/// sidecar) decides what fits, when to load and when to unload.
public protocol LLMEngine: AnyObject, Sendable {
    var kind: EngineKind { get }
    /// Loads model files from `location` (an MLX folder or a .gguf file).
    /// Apple Foundation Models ignores it. Replaces any loaded model.
    func load(from location: URL?) async throws
    /// Streams the reply to `turns`. Cancelling the consuming task stops
    /// generation at the next token.
    func generate(_ turns: [ChatTurn], options: GenerationOptions) -> AsyncThrowingStream<GenerationEvent, Error>
    /// Frees the model and the engine's GPU caches. Safe to call when empty.
    func unload() async
    var isLoaded: Bool { get async }
}
