import Foundation
#if canImport(FoundationModels)
import FoundationModels
#endif

/// Apple Foundation Models (the system model on iOS/macOS 26+ with Apple
/// Intelligence on). Zero download, nothing to fit: the OS owns the memory.
public final class AppleFMEngine: LLMEngine, @unchecked Sendable {
    public let kind = EngineKind.appleFM

    public init() {}

    public enum Availability: Equatable, Sendable {
        case available
        /// OS older than 26, or an SDK without FoundationModels.
        case unsupportedOS
        case deviceNotEligible
        case appleIntelligenceOff
        case modelNotReady
        case other(String)

        /// Short reason for a settings row.
        public var reason: String? {
            switch self {
            case .available: return nil
            case .unsupportedOS: return "Needs iOS 26 or later"
            case .deviceNotEligible: return "This device cannot run Apple Intelligence"
            case .appleIntelligenceOff: return "Turn on Apple Intelligence in Settings"
            case .modelNotReady: return "Apple Intelligence is still downloading"
            case let .other(why): return why
            }
        }
    }

    public static var availability: Availability {
        #if canImport(FoundationModels)
        if #available(iOS 26.0, macOS 26.0, *) {
            switch SystemLanguageModel.default.availability {
            case .available: return .available
            case let .unavailable(reason):
                switch reason {
                case .deviceNotEligible: return .deviceNotEligible
                case .appleIntelligenceNotEnabled: return .appleIntelligenceOff
                case .modelNotReady: return .modelNotReady
                @unknown default: return .other(String(describing: reason))
                }
            @unknown default: return .other("unknown")
            }
        }
        #endif
        return .unsupportedOS
    }

    private let lock = NSLock()
    private var loaded = false

    public var isLoaded: Bool { get async { lock.withLock { loaded } } }

    public func load(from location: URL?) async throws {
        guard Self.availability == .available else {
            throw EngineError.unavailable(Self.availability.reason ?? "Apple Intelligence is not available")
        }
        lock.withLock { loaded = true }
    }

    public func unload() async { lock.withLock { loaded = false } }

    public func generate(_ turns: [ChatTurn], options: GenerationOptions) -> AsyncThrowingStream<GenerationEvent, Error> {
        AsyncThrowingStream { continuation in
            #if canImport(FoundationModels)
            if #available(iOS 26.0, macOS 26.0, *) {
                let task = Task {
                    do {
                        let system = turns.filter { $0.role == .system }.map(\.content).joined(separator: "\n")
                        let convo = turns.filter { $0.role != .system }
                        // Earlier turns go in the prompt as a transcript; the
                        // last user turn is the request.
                        let history = convo.dropLast().map { "\($0.role == .user ? "User" : "Assistant"): \($0.content)" }
                        let last = convo.last?.content ?? ""
                        let prompt = history.isEmpty ? last : history.joined(separator: "\n") + "\nUser: \(last)"
                        let session = system.isEmpty ? LanguageModelSession() : LanguageModelSession(instructions: system)
                        let fmOptions = FoundationModels.GenerationOptions(
                            temperature: Double(options.temperature), maximumResponseTokens: options.maxTokens)
                        let start = Date()
                        var first: Date?
                        var sent = ""
                        for try await snapshot in session.streamResponse(to: prompt, options: fmOptions) {
                            if Task.isCancelled { break }
                            let full = snapshot.content
                            if first == nil { first = Date() }
                            if full.hasPrefix(sent) {
                                let delta = String(full.dropFirst(sent.count))
                                if !delta.isEmpty { continuation.yield(.text(delta)) }
                            }
                            sent = full
                        }
                        let end = Date()
                        // The framework reports no token counts; ~4 chars per token.
                        continuation.yield(.done(GenerationStats(
                            promptTokens: prompt.count / 4, generatedTokens: max(1, sent.count / 4),
                            promptSeconds: (first ?? end).timeIntervalSince(start),
                            generateSeconds: end.timeIntervalSince(first ?? start))))
                        continuation.finish()
                    } catch {
                        continuation.finish(throwing: EngineError.generationFailed(String(describing: error)))
                    }
                }
                continuation.onTermination = { _ in task.cancel() }
                return
            }
            #endif
            continuation.finish(throwing: EngineError.unavailable(Availability.unsupportedOS.reason!))
        }
    }
}
