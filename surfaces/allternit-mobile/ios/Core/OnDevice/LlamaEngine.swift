import Foundation
import OnDeviceLLM
#if canImport(llama)
import llama
#endif

/// llama.cpp engine for GGUF quants (llama.xcframework b12414, fetched by
/// Packages/OnDeviceLLM/Frameworks/fetch-llama.sh, same release as the
/// Desktop Metal engine pack). Metal offload of every layer; one sequence;
/// all llama calls on one serial queue (a llama_context is not thread safe).
/// App-side rather than in OnDeviceLLM: the Mac runs GGUF through the signed
/// llama-server engine pack instead.
final class LlamaEngine: LLMEngine, @unchecked Sendable {
    let kind = EngineKind.llamaCpp
    private let queue = DispatchQueue(label: "com.allternit.ondevice.llama", qos: .userInitiated)

    #if canImport(llama)
    private var model: OpaquePointer?
    private var ctx: OpaquePointer?
    private var contextLength: UInt32 = 4096
    private static let backendOnce: Void = { llama_backend_init() }()
    #endif

    static var isLinked: Bool {
        #if canImport(llama)
        return true
        #else
        return false
        #endif
    }

    var isLoaded: Bool {
        get async {
            #if canImport(llama)
            return await onQueue { self.model != nil }
            #else
            return false
            #endif
        }
    }

    private func onQueueThrowing<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { cont in
            queue.async { cont.resume(with: Result { try work() }) }
        }
    }

    private func onQueue<T: Sendable>(_ work: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { cont in queue.async { cont.resume(returning: work()) } }
    }

    func load(from location: URL?) async throws {
        #if canImport(llama)
        guard let location else { throw EngineError.loadFailed("no file") }
        try await onQueueThrowing {
            _ = Self.backendOnce
            self.freeAll()
            var mp = llama_model_default_params()
            #if targetEnvironment(simulator)
            mp.n_gpu_layers = 0
            #else
            mp.n_gpu_layers = 999
            #endif
            guard let m = llama_model_load_from_file(location.path, mp) else {
                throw EngineError.loadFailed("llama.cpp could not read \(location.lastPathComponent)")
            }
            var cp = llama_context_default_params()
            cp.n_ctx = self.contextLength
            cp.n_batch = 512
            let threads = Int32(max(1, min(4, ProcessInfo.processInfo.activeProcessorCount - 2)))
            cp.n_threads = threads
            cp.n_threads_batch = threads
            guard let c = llama_init_from_model(m, cp) else {
                llama_model_free(m)
                throw EngineError.loadFailed("llama.cpp could not create a context")
            }
            self.model = m
            self.ctx = c
        }
        #else
        throw EngineError.unavailable("llama.cpp is not in this build")
        #endif
    }

    func unload() async {
        #if canImport(llama)
        await onQueue { self.freeAll() }
        #endif
    }

    #if canImport(llama)
    /// Queue only.
    private func freeAll() {
        if let ctx { llama_free(ctx) }
        if let model { llama_model_free(model) }
        ctx = nil
        model = nil
    }

    /// Formats the chat with the model's own template (GGUF metadata).
    /// Queue only.
    private func prompt(_ turns: [ChatTurn], model: OpaquePointer) -> String {
        let tmpl = llama_model_chat_template(model, nil)
        var cStrings: [UnsafeMutablePointer<CChar>] = []
        defer { cStrings.forEach { free($0) } }
        var msgs: [llama_chat_message] = []
        for t in turns {
            let r = strdup(t.role.rawValue)!, c = strdup(t.content)!
            cStrings += [r, c]
            msgs.append(llama_chat_message(role: UnsafePointer(r), content: UnsafePointer(c)))
        }
        var buf = [CChar](repeating: 0, count: max(1024, turns.reduce(0) { $0 + $1.content.utf8.count } * 2 + 512))
        var n = llama_chat_apply_template(tmpl, msgs, msgs.count, true, &buf, Int32(buf.count))
        if n > buf.count {
            buf = [CChar](repeating: 0, count: Int(n) + 1)
            n = llama_chat_apply_template(tmpl, msgs, msgs.count, true, &buf, Int32(buf.count))
        }
        if n < 0 {
            // Template not supported by the built-in list: plain fallback.
            return turns.map { "\($0.role.rawValue): \($0.content)" }.joined(separator: "\n") + "\nassistant:"
        }
        return String(decoding: buf.prefix(Int(n)).map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }
    #endif

    func generate(_ turns: [ChatTurn], options: GenerationOptions) -> AsyncThrowingStream<GenerationEvent, Error> {
        AsyncThrowingStream { continuation in
            #if canImport(llama)
            let cancelled = ManagedAtomicFlag()
            continuation.onTermination = { _ in cancelled.set() }
            queue.async { [self] in
                guard let model, let ctx else { continuation.finish(throwing: EngineError.notLoaded); return }
                let vocab = llama_model_get_vocab(model)
                llama_memory_clear(llama_get_memory(ctx), true)

                let text = prompt(turns, model: model)
                let maxPrompt = Int32(llama_n_ctx(ctx)) - Int32(min(options.maxTokens, 1024))
                var tokens = [llama_token](repeating: 0, count: text.utf8.count + 16)
                let nTok = llama_tokenize(vocab, text, Int32(text.utf8.count), &tokens, Int32(tokens.count), true, true)
                guard nTok > 0 else { continuation.finish(throwing: EngineError.generationFailed("could not tokenize")); return }
                guard nTok <= maxPrompt else {
                    continuation.finish(throwing: EngineError.generationFailed("the chat is longer than the model's context"))
                    return
                }
                tokens = Array(tokens.prefix(Int(nTok)))

                let chain = llama_sampler_chain_init(llama_sampler_chain_default_params())
                defer { llama_sampler_free(chain) }
                if options.temperature <= 0 {
                    llama_sampler_chain_add(chain, llama_sampler_init_greedy())
                } else {
                    llama_sampler_chain_add(chain, llama_sampler_init_top_p(options.topP, 1))
                    llama_sampler_chain_add(chain, llama_sampler_init_temp(options.temperature))
                    llama_sampler_chain_add(chain, llama_sampler_init_dist(UInt32.random(in: 0...UInt32.max)))
                }

                let start = Date()
                var rc = tokens.withUnsafeMutableBufferPointer { buf in
                    // Feed the prompt in n_batch-sized pieces.
                    var off = 0
                    var result: Int32 = 0
                    while off < buf.count && result == 0 {
                        let n = min(512, buf.count - off)
                        result = llama_decode(ctx, llama_batch_get_one(buf.baseAddress! + off, Int32(n)))
                        off += n
                    }
                    return result
                }
                guard rc == 0 else { continuation.finish(throwing: EngineError.generationFailed("prompt decode \(rc)")); return }
                let promptDone = Date()

                var generated = 0
                var pending: [UInt8] = []
                var piece = [CChar](repeating: 0, count: 256)
                while generated < options.maxTokens, !cancelled.isSet {
                    var tok = llama_sampler_sample(chain, ctx, -1)
                    if llama_vocab_is_eog(vocab, tok) { break }
                    generated += 1
                    let n = llama_token_to_piece(vocab, tok, &piece, Int32(piece.count), 0, false)
                    if n > 0 {
                        pending += piece.prefix(Int(n)).map { UInt8(bitPattern: $0) }
                        // Emit only complete UTF-8; a character can span tokens.
                        let valid = Self.validUTF8Prefix(pending)
                        if valid > 0 {
                            continuation.yield(.text(String(decoding: pending.prefix(valid), as: UTF8.self)))
                            pending.removeFirst(valid)
                        }
                    }
                    rc = llama_decode(ctx, llama_batch_get_one(&tok, 1))
                    if rc != 0 { break }
                }
                if !pending.isEmpty { continuation.yield(.text(String(decoding: pending, as: UTF8.self))) }
                let end = Date()
                continuation.yield(.done(GenerationStats(
                    promptTokens: tokens.count, generatedTokens: generated,
                    promptSeconds: promptDone.timeIntervalSince(start), generateSeconds: end.timeIntervalSince(promptDone))))
                continuation.finish()
            }
            #else
            continuation.finish(throwing: EngineError.unavailable("llama.cpp is not in this build"))
            #endif
        }
    }

    /// Length of the longest prefix of `bytes` that ends on a UTF-8 boundary.
    static func validUTF8Prefix(_ bytes: [UInt8]) -> Int {
        let n = bytes.count
        var i = n - 1
        while i >= 0 && n - i <= 4 {
            let b = bytes[i]
            if b & 0x80 == 0 { return n }
            if b & 0xC0 == 0xC0 {
                let need = b >= 0xF0 ? 4 : b >= 0xE0 ? 3 : 2
                return n - i >= need ? n : i
            }
            i -= 1
        }
        return n
    }
}

/// A set-once flag readable from any thread.
private final class ManagedAtomicFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    func set() { lock.withLock { value = true } }
    var isSet: Bool { lock.withLock { value } }
}
