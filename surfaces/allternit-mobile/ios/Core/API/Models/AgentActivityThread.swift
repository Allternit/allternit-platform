import Foundation

// -----------------------------------------------------------------------------
// Agent Activity (Factory mail) REST models — allternit-api's
// `factory_core_router()`, mounted at `/api/factory` (AppConfig.factoryBaseURL). Verified against
// the real handlers directly, not against the web phase's own client types.
// -----------------------------------------------------------------------------

/// One row of `GET /mail/threads`'s `threads[]` (`list_mail_threads`) — `{ thread_id, messages: <count>, last_ts }`.
/// `messages` is a message *count*, not the message list — there is no
/// separate human-readable title yet, `threadId` literally IS the topic
/// string (the web phase's own finding, still true; displayed as-is).
struct AgentActivityThreadSummary: Decodable, Sendable, Identifiable, Hashable {
    let threadId: String
    let messageCount: Int
    let lastActivityAt: String

    var id: String { threadId }

    private enum CodingKeys: String, CodingKey {
        case threadId = "thread_id"
        case messageCount = "messages"
        case lastActivityAt = "last_ts"
    }
}

/// `GET /mail/threads`'s envelope — `{ threads: [...] }`.
struct AgentActivityThreadListResponse: Decodable, Sendable {
    let threads: [AgentActivityThreadSummary]
}

/// One row of `GET /mail/thread/:id`'s `messages[]` (`read_mail_thread`) — `{ message_id, thread_id, from_agent, body,
/// event_type, timestamp }`. `body` is pulled server-side from a ledger
/// event's `payload.body_ref` key, so it can be any JSON value (a plain
/// string, `null`, or something structured) depending on what produced the
/// event — decoded leniently here: a `String` when present, `nil` for
/// anything else, rather than failing the whole thread decode.
struct AgentActivityMessage: Decodable, Sendable, Identifiable {
    let messageId: String
    let threadId: String
    let fromAgent: String
    let body: String?
    let eventType: String
    let timestamp: String

    var id: String { messageId }

    private enum CodingKeys: String, CodingKey {
        case messageId = "message_id"
        case threadId = "thread_id"
        case fromAgent = "from_agent"
        case body
        case eventType = "event_type"
        case timestamp
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        messageId = try container.decode(String.self, forKey: .messageId)
        threadId = try container.decode(String.self, forKey: .threadId)
        fromAgent = try container.decode(String.self, forKey: .fromAgent)
        body = try? container.decode(String.self, forKey: .body)
        eventType = try container.decode(String.self, forKey: .eventType)
        timestamp = try container.decode(String.self, forKey: .timestamp)
    }
}

/// `GET /mail/thread/:id`'s envelope — `{ messages: [...] }`.
struct AgentActivityMessageListResponse: Decodable, Sendable {
    let messages: [AgentActivityMessage]
}

/// One event from `POST /ledger/tail` (`tail_ledger` / `UiLedgerEvent`) — the response is a bare `UiLedgerEvent[]`, no
/// envelope. Wire keys, verified against the real `UiLedgerEvent` struct, are
/// `event_id` / `event_type` / `timestamp` / `payload` (the internal Rust
/// field is named `ts`, but it's serialized out as `timestamp` — the map
/// doc's guess of `ts` on the wire was wrong).
///
/// No `AnyCodable`/`AnyDecodable` type exists anywhere in this codebase —
/// `PermissionMetadata`'s own doc comment (Core/API/PermissionClient.swift)
/// confirms that was checked and deliberately rejected in favor of narrow,
/// per-caller decodes of only the keys actually read. `payload` here is an
/// untyped `serde_json::Value` on the wire, and the only thing this phase
/// needs from it is whether it carries a `thread_id` or `mail_thread_id`
/// string, so it decodes narrowly into exactly those two optional fields
/// (`LedgerEventPayload`) instead of a general-purpose JSON value type.
struct LedgerEvent: Decodable, Sendable, Identifiable {
    let eventId: String
    let eventType: String
    let timestamp: String
    let payload: LedgerEventPayload?

    var id: String { eventId }

    private enum CodingKeys: String, CodingKey {
        case eventId = "event_id"
        case eventType = "event_type"
        case timestamp
        case payload
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        eventId = try container.decode(String.self, forKey: .eventId)
        eventType = try container.decode(String.self, forKey: .eventType)
        timestamp = try container.decode(String.self, forKey: .timestamp)
        // `payload` is an arbitrary JSON value server-side — if it's ever
        // something LedgerEventPayload can't parse as an object (or is
        // missing), surface no thread linkage rather than failing the whole
        // event (and with it, the rest of the batch — JSONDecoder aborts an
        // array decode on the first element that throws).
        payload = try? container.decodeIfPresent(LedgerEventPayload.self, forKey: .payload)
    }

    /// The thread id a ledger event is "about", if any — the only signal
    /// `AgentActivityStore` uses to associate a ledger event with a thread.
    var relatedThreadId: String? {
        payload?.threadId ?? payload?.mailThreadId
    }
}

/// Narrow decode of `UiLedgerEvent.payload` (`serde_json::Value` on the
/// wire) — only the two keys `AgentActivityStore`'s reservation/guard/review
/// heuristic reads. See `LedgerEvent`'s doc comment for why this isn't a
/// general JSON value type.
struct LedgerEventPayload: Decodable, Sendable {
    let threadId: String?
    let mailThreadId: String?

    private enum CodingKeys: String, CodingKey {
        case threadId = "thread_id"
        case mailThreadId = "mail_thread_id"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        threadId = try? container.decode(String.self, forKey: .threadId)
        mailThreadId = try? container.decode(String.self, forKey: .mailThreadId)
    }
}
