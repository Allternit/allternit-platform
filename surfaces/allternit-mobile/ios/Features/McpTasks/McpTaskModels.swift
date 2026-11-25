import Foundation

/// Long-running MCP tasks and server questions (elicitation), shown inline in
/// chat. Same events and routes as the web (allternit-ai McpTaskCard /
/// McpElicitationForm, behind `mcp.tasks`):
///   bus events `mcp.task` {sessionID, part}, `mcp.elicitation` {sessionID, part},
///   `mcp.elicitation.resolved` {sessionID, requestID, action};
///   POST /api/v1/mcp/elicitation/:id/reply {action, content?}
///   POST /api/v1/mcp/task/:id/cancel   (ids contain ':' — URL-encode them).

/// Feature flag `mcp.tasks`, off by default. Turn on with the launch argument
/// `-allternit.flag.mcp.tasks YES` or the `allternit.flag.mcp.tasks` default.
enum McpTasksFlag {
    static let key = "allternit.flag.mcp.tasks"
    static var isOn: Bool { UserDefaults.standard.bool(forKey: key) }
}

// MARK: - Task part

struct McpTaskPart: Decodable, Sendable, Equatable, Identifiable {
    struct Progress: Decodable, Sendable, Equatable {
        let done: Double?
        let total: Double?
        let label: String?

        /// 0...1 when a total is known.
        var fraction: Double? {
            guard let done, let total, total > 0 else { return nil }
            return min(1, max(0, done / total))
        }
    }

    var sessionID: String? = nil
    let taskId: String
    let status: String
    let toolName: String?
    /// The server the task runs on.
    let source: String?
    let title: String?
    let statusMessage: String?
    let progress: Progress?
    let toolCallID: String?
    /// Epoch milliseconds.
    let startedAt: Double?
    let updatedAt: Double?
    let errorText: String?

    var id: String { taskId }

    var isInputNeeded: Bool { status == "input_required" }
    var isRunning: Bool { status == "working" || status == "submitted" || status == "running" }
    var isDone: Bool { status == "completed" }
    var isFailed: Bool { status == "failed" }
    var isCancelled: Bool { status == "cancelled" || status == "canceled" }
    var isFinished: Bool { isDone || isFailed || isCancelled }

    var displayTitle: String {
        if let title, !title.isEmpty { return title }
        if let toolName, !toolName.isEmpty { return toolName }
        return "Task"
    }

    private enum CodingKeys: String, CodingKey {
        case sessionID, taskId, status, toolName, source, title, statusMessage
        case progress, toolCallID, startedAt, updatedAt, error
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionID = try? c.decodeIfPresent(String.self, forKey: .sessionID)
        taskId = try c.decode(String.self, forKey: .taskId)
        status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? "working"
        toolName = try? c.decodeIfPresent(String.self, forKey: .toolName)
        source = try? c.decodeIfPresent(String.self, forKey: .source)
        title = try? c.decodeIfPresent(String.self, forKey: .title)
        statusMessage = try? c.decodeIfPresent(String.self, forKey: .statusMessage)
        progress = try? c.decodeIfPresent(Progress.self, forKey: .progress)
        toolCallID = try? c.decodeIfPresent(String.self, forKey: .toolCallID)
        startedAt = try? c.decodeIfPresent(Double.self, forKey: .startedAt)
        updatedAt = try? c.decodeIfPresent(Double.self, forKey: .updatedAt)
        if let text = try? c.decodeIfPresent(String.self, forKey: .error) {
            errorText = text
        } else if let nested = try? c.decodeIfPresent([String: McpJSON].self, forKey: .error),
                  case .string(let message)? = nested["message"] {
            errorText = message
        } else {
            errorText = nil
        }
    }
}

// MARK: - Elicitation part

struct McpFormField: Equatable, Identifiable, Sendable {
    enum Kind: Equatable, Sendable { case text, number, integer, boolean, choice }

    let key: String
    let title: String
    let detail: String?
    let kind: Kind
    let options: [Option]
    let required: Bool
    let defaultText: String?
    let multiline: Bool

    struct Option: Equatable, Sendable { let value: String; let label: String }

    var id: String { key }

    /// Form mode never collects secrets: the server must use URL mode or a
    /// connector. Matches password-like names and `format: password`.
    let isSecret: Bool

    static func looksSecret(_ text: String, format: String?) -> Bool {
        if format?.lowercased() == "password" { return true }
        let lowered = text.lowercased().replacingOccurrences(of: "_", with: " ").replacingOccurrences(of: "-", with: " ")
        return ["password", "passcode", "secret", "token", "api key", "apikey", "private key", "credential", "ssn", "card number", "cvv"]
            .contains { lowered.contains($0) }
    }
}

struct McpElicitationPart: Decodable, Sendable, Equatable, Identifiable {
    enum Mode: String, Sendable { case form, url }

    var sessionID: String? = nil
    /// "<taskId>:<key>".
    let id: String
    let mode: Mode
    let source: String?
    let message: String
    let taskId: String?
    let toolCallID: String?
    let url: URL?
    let fields: [McpFormField]

    var hasSecretField: Bool { fields.contains(where: \.isSecret) }

    private enum CodingKeys: String, CodingKey {
        case sessionID, id, mode, source, message, taskId, toolCallID, url, requestedSchema
    }

    private struct Schema: Decodable {
        struct Prop: Decodable {
            let type: String?
            let title: String?
            let description: String?
            let format: String?
            let enumValues: [String]?
            let enumNames: [String]?
            let oneOf: [OneOf]?
            let defaultValue: McpJSON?
            let maxLength: Int?
            struct OneOf: Decodable { let const: String?; let title: String? }
            private enum CodingKeys: String, CodingKey {
                case type, title, description, format, enumNames, oneOf, maxLength
                case enumValues = "enum"
                case defaultValue = "default"
            }
        }
        let properties: [String: Prop]?
        let required: [String]?
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionID = try? c.decodeIfPresent(String.self, forKey: .sessionID)
        id = try c.decode(String.self, forKey: .id)
        let rawMode = (try? c.decodeIfPresent(String.self, forKey: .mode)) ?? "form"
        mode = Mode(rawValue: rawMode) ?? .form
        source = try? c.decodeIfPresent(String.self, forKey: .source)
        message = (try? c.decodeIfPresent(String.self, forKey: .message)) ?? ""
        taskId = try? c.decodeIfPresent(String.self, forKey: .taskId)
        toolCallID = try? c.decodeIfPresent(String.self, forKey: .toolCallID)
        if let raw = try? c.decodeIfPresent(String.self, forKey: .url), let parsed = URL(string: raw),
           ["https", "http"].contains(parsed.scheme?.lowercased() ?? "") {
            url = parsed
        } else {
            url = nil
        }
        let schema = try? c.decodeIfPresent(Schema.self, forKey: .requestedSchema)
        let required = Set(schema?.required ?? [])
        var built: [McpFormField] = []
        for (key, prop) in schema?.properties ?? [:] {
            let title = prop.title ?? key
            var options: [McpFormField.Option] = []
            if let one = prop.oneOf {
                options = one.compactMap { o in o.const.map { .init(value: $0, label: o.title ?? $0) } }
            } else if let values = prop.enumValues {
                options = values.enumerated().map { i, v in
                    .init(value: v, label: (prop.enumNames?.indices.contains(i) ?? false) ? prop.enumNames![i] : v)
                }
            }
            let kind: McpFormField.Kind
            if !options.isEmpty { kind = .choice }
            else {
                switch prop.type {
                case "boolean": kind = .boolean
                case "integer": kind = .integer
                case "number": kind = .number
                default: kind = .text
                }
            }
            built.append(McpFormField(
                key: key, title: title, detail: prop.description, kind: kind, options: options,
                required: required.contains(key), defaultText: prop.defaultValue?.text,
                multiline: (prop.maxLength ?? 0) > 200,
                isSecret: McpFormField.looksSecret(key + " " + title, format: prop.format)
            ))
        }
        // Required first, then by name (JSON objects carry no order).
        fields = built.sorted { ($0.required ? 0 : 1, $0.key) < ($1.required ? 0 : 1, $1.key) }
    }
}

// MARK: - Tolerant JSON value

enum McpJSON: Codable, Sendable, Equatable {
    case string(String), number(Double), bool(Bool), null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let n = try? c.decode(Double.self) { self = .number(n) }
        else if let s = try? c.decode(String.self) { self = .string(s) }
        else { self = .null }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let s): try c.encode(s)
        case .number(let n): try c.encode(n)
        case .bool(let b): try c.encode(b)
        case .null: try c.encodeNil()
        }
    }

    var text: String? {
        switch self {
        case .string(let s): return s
        case .number(let n): return n == n.rounded() ? String(Int(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return nil
        }
    }
}

// MARK: - Bus envelope

/// `{type, properties:{sessionID, part}}` (or the same fields flat).
struct McpBusEnvelope<Part: Decodable>: Decodable {
    let sessionID: String?
    let part: Part?

    private struct Inner: Decodable { let sessionID: String?; let part: Part? }
    private enum CodingKeys: String, CodingKey { case properties }

    init(from decoder: Decoder) throws {
        let nested = try? decoder.container(keyedBy: CodingKeys.self).decodeIfPresent(Inner.self, forKey: .properties)
        let inner = nested ?? (try? Inner(from: decoder))
        sessionID = inner?.sessionID
        part = inner?.part
    }
}

struct McpElicitationResolved: Decodable {
    let requestID: String?
    let action: String?

    private struct Inner: Decodable { let requestID: String?; let action: String? }
    private enum CodingKeys: String, CodingKey { case properties }

    init(from decoder: Decoder) throws {
        let nested = try? decoder.container(keyedBy: CodingKeys.self).decodeIfPresent(Inner.self, forKey: .properties)
        let inner = nested ?? (try? Inner(from: decoder))
        requestID = inner?.requestID
        action = inner?.action
    }
}

// MARK: - Client

final class McpTasksClient: @unchecked Sendable {
    static let shared = McpTasksClient()
    private let client: APIClient
    init(client: APIClient = .shared) { self.client = client }

    private struct ReplyBody: Encodable {
        let action: String
        let content: [String: McpJSON]?
    }

    /// action: accept | decline | cancel. `content` only with accept.
    func replyElicitation(id: String, action: String, content: [String: McpJSON]? = nil) async throws {
        try await client.post(
            path: "mcp/elicitation/\(Self.escape(id))/reply",
            body: ReplyBody(action: action, content: content)
        )
    }

    func cancelTask(id: String) async throws {
        try await client.post(path: "mcp/task/\(Self.escape(id))/cancel")
    }

    /// Ids carry ':' — encode everything but unreserved characters.
    static func escape(_ id: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return id.addingPercentEncoding(withAllowedCharacters: allowed) ?? id
    }
}

// MARK: - Store

/// Latest state of every task and open question, by id. Chat messages keep the
/// ids; the cards read here so a status update redraws the same card.
@MainActor
final class McpTasksStore: ObservableObject {
    static let shared = McpTasksStore()

    @Published private(set) var tasks: [String: McpTaskPart] = [:]
    @Published private(set) var elicitations: [String: McpElicitationPart] = [:]
    @Published private(set) var resolved: [String: String] = [:]

    func apply(_ part: McpTaskPart, sessionID: String?) {
        var next = part
        next.sessionID = sessionID ?? part.sessionID
        let finishedNow = next.isFinished && !(tasks[part.taskId]?.isFinished ?? false)
        tasks[part.taskId] = next
        if finishedNow { next.isDone ? Haptics.success() : Haptics.warning() }
    }

    func apply(_ part: McpElicitationPart, sessionID: String?) {
        var next = part
        next.sessionID = sessionID ?? part.sessionID
        if elicitations[part.id] == nil && resolved[part.id] == nil { Haptics.medium() }
        elicitations[part.id] = next
    }

    func markResolved(id: String, action: String) { resolved[id] = action }

    func isResolved(_ id: String) -> Bool { resolved[id] != nil }
}
