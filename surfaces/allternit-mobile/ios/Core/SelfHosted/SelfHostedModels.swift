import Foundation

// Wire types for the Self-Hosted Agents Server (cmd/allternit-api/openapi/selfhost-v1.yaml).
// Only the fields the phone shows; unknown fields are ignored.

struct SHServerInfo: Decodable, Equatable {
    var version: String
    var provider: String
    var authModes: [String]?
    var publicURL: String?

    enum CodingKeys: String, CodingKey {
        case version, provider
        case authModes = "auth_modes"
        case publicURL = "public_url"
    }
}

struct SHList<T: Decodable>: Decodable {
    var data: [T]
    var hasMore: Bool?
    var nextCursor: String?

    enum CodingKeys: String, CodingKey {
        case data
        case hasMore = "has_more"
        case nextCursor = "next_cursor"
    }
}

struct SHAgent: Decodable, Identifiable, Equatable, Hashable {
    var id: String
    var name: String
    var model: String
    var instructions: String?
}

struct SHEnvironment: Decodable, Identifiable, Equatable, Hashable {
    var id: String
    var name: String
    var network: String?
    var workerCount: Int?
    var queueDepth: Int?

    enum CodingKeys: String, CodingKey {
        case id, name, network
        case workerCount = "worker_count"
        case queueDepth = "queue_depth"
    }
}

struct SHWorker: Decodable, Identifiable, Equatable {
    var workerID: String
    var online: Bool
    var hostname: String?
    var version: String?
    var currentWorkID: String?
    var id: String { workerID }

    enum CodingKeys: String, CodingKey {
        case online, hostname, version
        case workerID = "worker_id"
        case currentWorkID = "current_work_id"
    }
}

enum SHSessionStatus: String, Decodable {
    case idle, running, interrupted, error, archived
    case waitingForWorker = "waiting_for_worker"
}

struct SHSession: Decodable, Identifiable, Equatable {
    var id: String
    var agentID: String
    var environmentID: String
    var status: SHSessionStatus
    var statusReason: String?
    var lastSeq: Int?

    enum CodingKeys: String, CodingKey {
        case id, status
        case agentID = "agent_id"
        case environmentID = "environment_id"
        case statusReason = "status_reason"
        case lastSeq = "last_seq"
    }
}

/// One session event. `data` is decoded by `type`.
struct SHEvent: Decodable, Identifiable, Equatable {
    enum Payload: Equatable {
        case message(role: String, content: String)
        case toolCall(id: String, name: String, input: String)
        case toolResult(id: String, output: String, isError: Bool)
        case status(SHSessionStatus?, reason: String?)
        case error(code: String, message: String)
        case unknown
    }

    var seq: Int
    var type: String
    var payload: Payload
    var id: Int { seq }

    enum CodingKeys: String, CodingKey { case seq, type, data }

    init(seq: Int, type: String, payload: Payload) {
        self.seq = seq; self.type = type; self.payload = payload
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        seq = try c.decode(Int.self, forKey: .seq)
        type = try c.decode(String.self, forKey: .type)
        let raw = (try? c.decode(SHJSON.self, forKey: .data))?.object ?? [:]
        func str(_ k: String) -> String { raw[k]?.string ?? "" }
        switch type {
        case "message":
            payload = .message(role: str("role"), content: str("content"))
        case "tool_call":
            payload = .toolCall(id: str("tool_call_id"), name: str("name"),
                                input: raw["input"]?.compactString ?? "")
        case "tool_result":
            payload = .toolResult(id: str("tool_call_id"), output: str("output"),
                                  isError: raw["is_error"]?.bool ?? false)
        case "status":
            payload = .status(SHSessionStatus(rawValue: str("status")),
                              reason: raw["reason"]?.string)
        case "error":
            payload = .error(code: str("code"), message: str("message"))
        default:
            payload = .unknown
        }
    }
}

/// Minimal JSON value, enough to read event `data` without a schema per type.
indirect enum SHJSON: Decodable, Equatable {
    case string(String), number(Double), bool(Bool), null
    case array([SHJSON]), object([String: SHJSON])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([SHJSON].self) { self = .array(v) }
        else { self = .object(try c.decode([String: SHJSON].self)) }
    }

    var string: String? { if case .string(let s) = self { return s }; return nil }
    var bool: Bool? { if case .bool(let b) = self { return b }; return nil }
    var object: [String: SHJSON]? { if case .object(let o) = self { return o }; return nil }

    /// Compact one-line rendering for tool inputs.
    var compactString: String {
        switch self {
        case .string(let s): return "\"\(s)\""
        case .number(let n): return n == n.rounded() ? String(Int(n)) : String(n)
        case .bool(let b): return String(b)
        case .null: return "null"
        case .array(let a): return "[" + a.map(\.compactString).joined(separator: ", ") + "]"
        case .object(let o):
            return "{" + o.keys.sorted().map { "\($0): \(o[$0]!.compactString)" }.joined(separator: ", ") + "}"
        }
    }
}
