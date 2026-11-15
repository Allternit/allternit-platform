// allternit-mlx-server: OpenAI-compatible loopback server over OnDeviceLLM.
//
// Spawned by local-engine (services/local-engine, MLX recipe) exactly like
// llama-server, and answers the same way so the engine's proxy and stats code
// treat both alike:
//   GET  /health                 503 {"status":"loading"} until the model is in memory, then 200
//   GET  /v1/models
//   POST /v1/chat/completions    stream and non-stream; `usage` (+ llama.cpp-style
//                                `timings`) on the last chunk when
//                                stream_options.include_usage is set, always on
//                                non-stream replies.
//
// Usage: allternit-mlx-server --model <dir> --port <n> [--host 127.0.0.1] [--model-id <id>] [--ctx <tokens>]

import Foundation
import Hummingbird
import OnDeviceLLM

struct Args {
    var model = ""
    var port = 0
    var host = "127.0.0.1"
    var modelId = ""
    var ctx: Int?

    static func parse(_ argv: [String]) -> Args {
        var a = Args()
        var it = argv.dropFirst().makeIterator()
        while let flag = it.next() {
            let value = it.next() ?? ""
            switch flag {
            case "--model", "-m": a.model = value
            case "--port": a.port = Int(value) ?? 0
            case "--host": a.host = value
            case "--model-id": a.modelId = value
            case "--ctx", "-c": a.ctx = Int(value)
            default:
                FileHandle.standardError.write("unknown flag \(flag)\n".data(using: .utf8)!)
                exit(2)
            }
        }
        if a.model.isEmpty || a.port == 0 {
            FileHandle.standardError.write("usage: allternit-mlx-server --model <dir> --port <n> [--host h] [--model-id id] [--ctx n]\n".data(using: .utf8)!)
            exit(2)
        }
        if a.modelId.isEmpty { a.modelId = URL(fileURLWithPath: a.model).lastPathComponent }
        return a
    }
}

/// The loaded engine, or the load error. Filled once, read by every request.
actor EngineSlot {
    private(set) var engine: OnDeviceLLMEngine?
    private(set) var error: String?
    func set(_ e: OnDeviceLLMEngine) { engine = e }
    func fail(_ message: String) { error = message }
}

let args = Args.parse(CommandLine.arguments)
let slot = EngineSlot()

Task {
    do {
        let engine = try await OnDeviceLLMEngine.load(directory: URL(fileURLWithPath: args.model), modelId: args.modelId)
        await slot.set(engine)
        print("model loaded: \(args.modelId) (\(OnDeviceLLMEngine.memoryBytes) bytes)")
    } catch {
        await slot.fail("\(error)")
        FileHandle.standardError.write("model load failed: \(error)\n".data(using: .utf8)!)
        exit(1)
    }
}

func json(_ object: Any, status: HTTPResponse.Status = .ok) -> Response {
    let data = (try? JSONSerialization.data(withJSONObject: object)) ?? Data("{}".utf8)
    return Response(
        status: status,
        headers: [.contentType: "application/json"],
        body: .init(byteBuffer: ByteBuffer(bytes: [UInt8](data)))
    )
}

func sse(_ object: Any) -> ByteBuffer {
    let data = (try? JSONSerialization.data(withJSONObject: object)) ?? Data("{}".utf8)
    return ByteBuffer(string: "data: \(String(decoding: data, as: UTF8.self))\n\n")
}

/// OpenAI message content: a string, or an array of {type:"text", text}.
func text(of content: Any?) -> String {
    if let s = content as? String { return s }
    if let parts = content as? [[String: Any]] {
        return parts.compactMap { $0["type"] as? String == "text" ? $0["text"] as? String : nil }.joined()
    }
    return ""
}

func usage(_ s: GenerationStats) -> [String: Any] {
    ["prompt_tokens": s.promptTokens, "completion_tokens": s.completionTokens, "total_tokens": s.promptTokens + s.completionTokens]
}

/// Same field names as llama-server's `timings`, so local-engine maps both one way.
func timings(_ s: GenerationStats) -> [String: Any] {
    [
        "prompt_n": s.promptTokens, "prompt_ms": s.promptSeconds * 1000, "prompt_per_second": s.promptTokensPerSecond,
        "predicted_n": s.completionTokens, "predicted_ms": s.generateSeconds * 1000, "predicted_per_second": s.tokensPerSecond,
    ]
}

let router = Router()

router.get("health") { _, _ -> Response in
    if await slot.engine != nil {
        return json(["status": "ok", "model": args.modelId, "mem_bytes": OnDeviceLLMEngine.memoryBytes])
    }
    if let error = await slot.error {
        return json(["status": "error", "error": error], status: .internalServerError)
    }
    return json(["status": "loading"], status: .serviceUnavailable)
}

router.get("v1/models") { _, _ -> Response in
    json(["object": "list", "data": [["id": args.modelId, "object": "model", "owned_by": "allternit"]]])
}

router.post("v1/chat/completions") { request, _ -> Response in
    guard let engine = await slot.engine else {
        return json(["error": ["message": "model is loading", "type": "unavailable"]], status: .serviceUnavailable)
    }
    let buffer = try await request.body.collect(upTo: 32 << 20)
    guard let body = try? JSONSerialization.jsonObject(with: Data(buffer.readableBytesView)) as? [String: Any],
          let messages = body["messages"] as? [[String: Any]]
    else {
        return json(["error": ["message": "body must be a chat completion request", "type": "invalid_request_error"]], status: .badRequest)
    }
    let turns = messages.map { m in
        ChatTurn(role: ChatTurn.Role(rawValue: m["role"] as? String ?? "user") ?? .user, content: text(of: m["content"]))
    }
    let options = GenerationOptions(
        maxTokens: (body["max_completion_tokens"] as? Int) ?? (body["max_tokens"] as? Int),
        temperature: Float((body["temperature"] as? Double) ?? 0.7),
        topP: Float((body["top_p"] as? Double) ?? 1.0),
        maxContextTokens: args.ctx
    )
    let id = "chatcmpl-\(UUID().uuidString.lowercased())"
    let created = Int(Date().timeIntervalSince1970)
    let model = args.modelId

    if body["stream"] as? Bool != true {
        var content = ""
        var stats: GenerationStats?
        for try await event in engine.stream(turns, options: options) {
            switch event {
            case .text(let t): content += t
            case .done(let s): stats = s
            }
        }
        var reply: [String: Any] = [
            "id": id, "object": "chat.completion", "created": created, "model": model,
            "choices": [["index": 0, "message": ["role": "assistant", "content": content], "finish_reason": stats?.finishReason ?? "stop"]],
        ]
        if let s = stats {
            reply["usage"] = usage(s)
            reply["timings"] = timings(s)
        }
        return json(reply)
    }

    let includeUsage = ((body["stream_options"] as? [String: Any])?["include_usage"] as? Bool) == true
    let events = engine.stream(turns, options: options)
    let responseBody = ResponseBody { writer in
        func chunk(_ delta: [String: Any], finish: String? = nil) -> [String: Any] {
            ["id": id, "object": "chat.completion.chunk", "created": created, "model": model,
             "choices": [["index": 0, "delta": delta, "finish_reason": finish as Any? ?? NSNull()]]]
        }
        try await writer.write(sse(chunk(["role": "assistant", "content": ""])))
        var stats: GenerationStats?
        do {
            for try await event in events {
                switch event {
                case .text(let t): try await writer.write(sse(chunk(["content": t])))
                case .done(let s): stats = s
                }
            }
        } catch {
            try await writer.write(sse(["error": ["message": "\(error)", "type": "server_error"]]))
        }
        try await writer.write(sse(chunk([:], finish: stats?.finishReason ?? "stop")))
        if includeUsage, let s = stats {
            try await writer.write(sse([
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [], "usage": usage(s), "timings": timings(s),
            ]))
        }
        try await writer.write(ByteBuffer(string: "data: [DONE]\n\n"))
        try await writer.finish(nil)
    }
    return Response(
        status: .ok,
        headers: [.contentType: "text/event-stream", .cacheControl: "no-cache"],
        body: responseBody
    )
}

let app = Application(
    router: router,
    configuration: .init(address: .hostname(args.host, port: args.port), serverName: "allternit-mlx-server")
)
try await app.runService()
