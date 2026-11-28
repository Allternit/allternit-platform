import Foundation

/// Client-driven tool loop for the browser computer (CONTRACT-colibri-unkernel
/// section 7), the Swift twin of allternit-ai `src/lib/browser-computer/tool-loop.ts`.
///
/// The phone sends `tools`, receives a tool call, runs it through the box
/// RPC `exec`, and sends the result on the next request. No server holds the
/// loop, so a closed app cannot leave a hanging run. The wire call and the
/// executor are injected, so the loop is tested with fakes.

// MARK: - Caps and tools (contract 7.1, 7.3)

enum BrowserToolCaps {
    static let streamBytes = 16 * 1024
    static let streamKeepBytes = 8 * 1024
    static let resultMessageBytes = 32 * 1024
    static let defaultTimeoutSeconds = 60
    static let maxTimeoutSeconds = 300
    static let runCodeMaxChars = 200_000
    static let writeFileMaxChars = 1_048_576
    static let readFileDefaultBytes = 65_536
    static let readFileMaxBytes = 262_144
    static let maxRoundsPerMessage = 8
}

/// Contract 7.4: the exact line a vendor plan brain gets in place of tools.
let browserPlanNoToolsLine = "Plan models cannot run commands. Open the Terminal to use your computer yourself."

enum BrowserToolDefinitions {
    /// OpenAI function format, copied from contract 7.1.
    static let all: [[String: Any]] = {
        let json = #"""
        [
          { "type": "function", "function": {
              "name": "bash",
              "description": "Run a shell command in the user's Linux computer. Returns stdout, stderr and the exit code.",
              "parameters": { "type": "object", "additionalProperties": false, "required": ["command"],
                "properties": {
                  "command": { "type": "string", "description": "Shell text, run as sh -c." },
                  "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 60 } } } } },
          { "type": "function", "function": {
              "name": "run_code",
              "description": "Run a short program in the user's Linux computer and return its output.",
              "parameters": { "type": "object", "additionalProperties": false, "required": ["language", "code"],
                "properties": {
                  "language": { "type": "string", "enum": ["python", "node"] },
                  "code": { "type": "string", "maxLength": 200000 },
                  "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 60 } } } } },
          { "type": "function", "function": {
              "name": "read_file",
              "description": "Read a text file from the user's Linux computer.",
              "parameters": { "type": "object", "additionalProperties": false, "required": ["path"],
                "properties": {
                  "path": { "type": "string", "description": "Absolute, or relative to /root." },
                  "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144, "default": 65536 } } } } },
          { "type": "function", "function": {
              "name": "write_file",
              "description": "Write a text file in the user's Linux computer. Creates parent folders.",
              "parameters": { "type": "object", "additionalProperties": false, "required": ["path", "content"],
                "properties": {
                  "path": { "type": "string" },
                  "content": { "type": "string", "maxLength": 1048576 },
                  "append": { "type": "boolean", "default": false } } } } }
        ]
        """#
        let parsed = try? JSONSerialization.jsonObject(with: Data(json.utf8))
        return (parsed as? [[String: Any]]) ?? []
    }()
}

// MARK: - Executor

struct BoxExecParams {
    var cmd: String
    var args: [String] = []
    var cwd: String? = nil
    /// Base64 text, as the TS loop sends it.
    var stdin: String? = nil
    var timeoutMs: Int = 60_000
}

struct BoxExecResult: Equatable {
    var stdout: String
    var stderr: String
    var exitCode: Int
    var timedOut: Bool = false
}

@MainActor
protocol BrowserComputerExecutor: AnyObject {
    func exec(_ params: BoxExecParams) async throws -> BoxExecResult
}

extension BrowserComputerModel: BrowserComputerExecutor {
    /// Contract 6.2 `exec`: `{cmd, args, cwd, stdin, timeout_ms}`.
    func exec(_ params: BoxExecParams) async throws -> BoxExecResult {
        let r = try await call("exec", [
            "cmd": params.cmd,
            "args": params.args,
            "cwd": params.cwd ?? NSNull(),
            "stdin": params.stdin ?? NSNull(),
            "timeout_ms": params.timeoutMs,
        ], timeout: Double(params.timeoutMs) / 1000 + 10)
        return BoxExecResult(
            stdout: r["stdout"] as? String ?? "",
            stderr: r["stderr"] as? String ?? "",
            exitCode: r["exit_code"] as? Int ?? -1,
            timedOut: r["timed_out"] as? Bool ?? false)
    }
}

/// The running browser computer, for the chat to find. The computer's cover
/// sets it when the VM is ready and clears it when it stops.
@MainActor
final class BrowserComputerRegistry {
    static let shared = BrowserComputerRegistry()
    weak var model: BrowserComputerModel?

    /// A computer is attached to the chat only while it is booted and ready.
    var attached: BrowserComputerModel? {
        guard let model, model.phase == .running, model.status.state == "ready" else { return nil }
        return model
    }
}

// MARK: - Wire shapes

struct BrowserToolCall: Equatable {
    let id: String
    let name: String
    /// JSON text, as the model wrote it.
    let arguments: String
}

enum BrowserWireMessage {
    case text(role: String, content: String)
    case assistantToolCalls([BrowserToolCall])
    case toolResult(callId: String, content: String)

    var json: [String: Any] {
        switch self {
        case .text(let role, let content):
            return ["role": role, "content": content]
        case .assistantToolCalls(let calls):
            return ["role": "assistant", "content": NSNull(),
                    "tool_calls": calls.map { ["id": $0.id, "type": "function",
                                               "function": ["name": $0.name, "arguments": $0.arguments]] as [String: Any] }]
        case .toolResult(let callId, let content):
            return ["role": "tool", "tool_call_id": callId, "content": content]
        }
    }
}

struct BrowserToolRequest {
    var messages: [BrowserWireMessage]
    /// Nil for a brain that cannot take tools.
    var tools: [[String: Any]]?
    /// True on the last round: `tool_choice: "none"`.
    var toolChoiceNone = false
    var onDelta: (String) -> Void
}

struct BrowserToolResponse {
    var toolCalls: [BrowserToolCall] = []
}

typealias BrowserStreamCall = @MainActor (BrowserToolRequest) async throws -> BrowserToolResponse

enum BrowserToolBrain { case tools, plan }

struct BrowserToolLoopCallbacks {
    var onChunk: (String) -> Void = { _ in }
    var onToolCall: (_ id: String, _ name: String, _ input: [String: String]) -> Void = { _, _, _ in }
    var onToolResult: (_ id: String, _ name: String, _ result: String) -> Void = { _, _, _ in }
    var onToolError: (_ id: String, _ name: String, _ error: String) -> Void = { _, _, _ in }
}

struct BrowserToolLoopResult: Equatable {
    var rounds: Int
    /// "Used N of 8 tool rounds", or nil when no tool ran.
    var roundsLine: String?
}

// MARK: - Loop

@MainActor
enum BrowserToolLoop {
    // MARK: caps

    /// Keeps the first and last `keep` bytes; the cut is named in the middle.
    static func cutMiddle(_ text: String, total: Int, keep: Int) -> String {
        let bytes = Array(text.utf8)
        if bytes.count <= total { return text }
        let cut = bytes.count - 2 * keep
        return "\(String(decoding: bytes.prefix(keep), as: UTF8.self))\n[... \(cut) bytes cut ...]\n\(String(decoding: bytes.suffix(keep), as: UTF8.self))"
    }

    /// A command stream returned to the model: over 16 KiB keeps 8 KiB from each end.
    static func capStream(_ text: String) -> (text: String, truncated: Bool) {
        if text.utf8.count <= BrowserToolCaps.streamBytes { return (text, false) }
        return (cutMiddle(text, total: BrowserToolCaps.streamBytes, keep: BrowserToolCaps.streamKeepBytes), true)
    }

    private static func jsonText(_ object: [String: Any]) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    /// The whole result message stays under 32 KiB: shrink the biggest text field until it fits.
    static func fitResultMessage(_ result: [String: Any]) -> String {
        var out = result
        var json = jsonText(out)
        var i = 0
        while i < 24, json.utf8.count > BrowserToolCaps.resultMessageBytes {
            i += 1
            let field = ["stdout", "stderr", "content"]
                .filter { out[$0] is String }
                .max { ((out[$0] as? String)?.utf8.count ?? 0) < ((out[$1] as? String)?.utf8.count ?? 0) }
            guard let field, let current = out[field] as? String else { break }
            let keep = max(16, current.utf8.count / 4)
            out[field] = cutMiddle(current, total: 0, keep: keep)
            out["truncated"] = true
            json = jsonText(out)
        }
        return json
    }

    // MARK: tools

    private static func timeoutMs(_ v: Any?) -> Int {
        guard let n = v as? Int ?? (v as? Double).map({ Int($0.rounded()) }) else {
            return BrowserToolCaps.defaultTimeoutSeconds * 1000
        }
        return min(max(n, 1), BrowserToolCaps.maxTimeoutSeconds) * 1000
    }

    private static func fail(_ error: String) -> [String: Any] { ["error": error] }

    private static func commandResult(_ r: BoxExecResult) -> [String: Any] {
        let out = capStream(r.stdout)
        let err = capStream(r.stderr)
        return ["exit_code": r.exitCode, "stdout": out.text, "stderr": err.text,
                "truncated": out.truncated || err.truncated, "timed_out": r.timedOut]
    }

    /// Content goes in stdin as base64 and the path in args, never in shell text.
    private static func writeText(_ ex: BrowserComputerExecutor, path: String, content: String, append: Bool) async throws -> [String: Any] {
        let script = append
            ? #"mkdir -p "$(dirname -- "$1")" && base64 -d >> "$1""#
            : #"mkdir -p "$(dirname -- "$1")" && base64 -d > "$1""#
        let r = try await ex.exec(BoxExecParams(cmd: "sh", args: ["-c", script, "sh", path], cwd: "/root",
                                                stdin: Data(content.utf8).base64EncodedString(), timeoutMs: 60_000))
        if r.exitCode != 0 {
            let message = r.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            return fail(message.isEmpty ? "Could not write the file." : message)
        }
        return ["bytes_written": content.utf8.count]
    }

    private static var tempCounter = 0

    /// Runs one tool call. Never throws: a failure is `{error}` and the loop continues.
    static func runTool(_ ex: BrowserComputerExecutor, name: String, rawArgs: String) async -> [String: Any] {
        guard let parsed = try? JSONSerialization.jsonObject(with: Data((rawArgs.isEmpty ? "{}" : rawArgs).utf8)),
              let args = parsed as? [String: Any] else {
            return fail("The tool arguments were not valid JSON.")
        }
        do {
            switch name {
            case "bash":
                guard let command = args["command"] as? String, !command.isEmpty else { return fail("bash needs a command.") }
                return commandResult(try await ex.exec(BoxExecParams(cmd: "sh", args: ["-c", command], cwd: "/root",
                                                                     timeoutMs: timeoutMs(args["timeout_seconds"]))))
            case "run_code":
                let language = args["language"] as? String
                guard language == "python" || language == "node" else { return fail("run_code language must be python or node.") }
                guard let code = args["code"] as? String, !code.isEmpty else { return fail("run_code needs code.") }
                if code.count > BrowserToolCaps.runCodeMaxChars { return fail("The code is too long to run.") }
                tempCounter += 1
                let file = "/tmp/run-\(String(Int(Date().timeIntervalSince1970 * 1000), radix: 36))-\(tempCounter).\(language == "python" ? "py" : "js")"
                let wrote = try await writeText(ex, path: file, content: code, append: false)
                if wrote["error"] != nil { return wrote }
                let result: [String: Any]
                do {
                    result = commandResult(try await ex.exec(BoxExecParams(cmd: language == "python" ? "python3" : "node",
                                                                           args: [file], cwd: "/root",
                                                                           timeoutMs: timeoutMs(args["timeout_seconds"]))))
                } catch {
                    _ = try? await ex.exec(BoxExecParams(cmd: "rm", args: ["-f", file], timeoutMs: 10_000))
                    throw error
                }
                _ = try? await ex.exec(BoxExecParams(cmd: "rm", args: ["-f", file], timeoutMs: 10_000))
                return result
            case "read_file":
                guard let path = args["path"] as? String, !path.isEmpty else { return fail("read_file needs a path.") }
                let requested = args["max_bytes"] as? Int
                let max = requested.map { min(Swift.max($0, 1), BrowserToolCaps.readFileMaxBytes) } ?? BrowserToolCaps.readFileDefaultBytes
                // One byte past the limit tells us whether the file was longer.
                let r = try await ex.exec(BoxExecParams(cmd: "head", args: ["-c", String(max + 1), "--", path], cwd: "/root", timeoutMs: 30_000))
                if r.exitCode != 0 {
                    let message = r.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
                    return fail(message.isEmpty ? "Could not read the file." : message)
                }
                let bytes = Array(r.stdout.utf8)
                let truncated = bytes.count > max
                return ["content": truncated ? String(decoding: bytes.prefix(max), as: UTF8.self) : r.stdout, "truncated": truncated]
            case "write_file":
                guard let path = args["path"] as? String, !path.isEmpty else { return fail("write_file needs a path.") }
                guard let content = args["content"] as? String else { return fail("write_file needs content.") }
                if content.count > BrowserToolCaps.writeFileMaxChars { return fail("The file content is too long to write.") }
                return try await writeText(ex, path: path, content: content, append: args["append"] as? Bool == true)
            default:
                return fail("There is no tool called \(name).")
            }
        } catch {
            let message = error.localizedDescription
            return fail(message.isEmpty ? "The computer did not answer." : message)
        }
    }

    // MARK: loop

    /// `plan`: the server refuses tools, so none are sent and the contract's plain line is shown.
    static func run(brain: BrowserToolBrain,
                    messages: [BrowserWireMessage],
                    executor: BrowserComputerExecutor,
                    streamCall: BrowserStreamCall,
                    callbacks: BrowserToolLoopCallbacks = BrowserToolLoopCallbacks()) async throws -> BrowserToolLoopResult {
        if brain == .plan {
            _ = try await streamCall(BrowserToolRequest(messages: messages, tools: nil, onDelta: callbacks.onChunk))
            callbacks.onChunk("\n\n\(browserPlanNoToolsLine)")
            return BrowserToolLoopResult(rounds: 0, roundsLine: nil)
        }
        var thread = messages
        var rounds = 0
        while true {
            try Task.checkCancellation()
            let last = rounds >= BrowserToolCaps.maxRoundsPerMessage
            let response = try await streamCall(BrowserToolRequest(messages: thread, tools: BrowserToolDefinitions.all,
                                                                   toolChoiceNone: last, onDelta: callbacks.onChunk))
            // The last request is "no more tools": any call in it is ignored.
            if last || response.toolCalls.isEmpty { break }
            rounds += 1
            thread.append(.assistantToolCalls(response.toolCalls))
            // Parallel calls in one assistant message run in order, one at a time.
            for call in response.toolCalls {
                try Task.checkCancellation()
                let input = ((try? JSONSerialization.jsonObject(with: Data(call.arguments.utf8))) as? [String: Any] ?? [:])
                    .compactMapValues { $0 as? String }
                callbacks.onToolCall(call.id, call.name, input)
                let result = await runTool(executor, name: call.name, rawArgs: call.arguments)
                let content = fitResultMessage(result)
                if let error = result["error"] as? String { callbacks.onToolError(call.id, call.name, error) }
                else { callbacks.onToolResult(call.id, call.name, content) }
                thread.append(.toolResult(callId: call.id, content: content))
            }
        }
        let line = rounds > 0 ? "Used \(rounds) of \(BrowserToolCaps.maxRoundsPerMessage) tool rounds" : nil
        if let line { callbacks.onChunk("\n\n\(line)") }
        return BrowserToolLoopResult(rounds: rounds, roundsLine: line)
    }
}
