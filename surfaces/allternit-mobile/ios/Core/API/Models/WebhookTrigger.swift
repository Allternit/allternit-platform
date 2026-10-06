import Foundation

// -----------------------------------------------------------------------------
// Webhook trigger REST models — base path /api/v1/webhook-triggers on
// allternit-api.
//
// Mirrors the Rust producers in
// cmd/allternit-api/src/webhook_trigger_routes.rs (`TriggerRow`,
// `DeliveryRow`) and the web client shapes in
// surfaces/ai.allternit.com/src/lib/webhook-api.ts. The API emits snake_case
// keys on the wire; Swift properties stay camelCase via explicit CodingKeys.
// Timestamps stay Strings (SQLite `CURRENT_TIMESTAMP`, "yyyy-MM-dd HH:mm:ss"
// UTC); views parse them at render time, never via a global date strategy.
//
// The trigger `secret` is generated server-side and NEVER returned by any
// route (webhook_trigger_routes.rs `create_trigger` — the HMAC key is
// write-only), so no model field for it exists here by design: the inbound
// URL alone is what the UI displays/copies.
// -----------------------------------------------------------------------------

/// One inbound webhook trigger (`TriggerRow`,
/// webhook_trigger_routes.rs:67-77) — an org-scoped route from a public
/// inbound URL to a bot: a signed POST to `/webhooks/inbound/:id` creates a
/// Factory ticket assigned to `targetBotId`.
struct WebhookTrigger: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let orgId: String
    let name: String
    let targetBotId: String
    let active: Bool
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id, name, active
        case orgId = "org_id"
        case targetBotId = "target_bot_id"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    /// Tolerant decoding per repo model convention: a missing field degrades
    /// to a default rather than failing the whole list.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decodeIfPresent(String.self, forKey: .id) ?? ""
        orgId = try container.decodeIfPresent(String.self, forKey: .orgId) ?? ""
        name = try container.decodeIfPresent(String.self, forKey: .name) ?? ""
        targetBotId = try container.decodeIfPresent(String.self, forKey: .targetBotId) ?? ""
        active = try container.decodeIfPresent(Bool.self, forKey: .active) ?? false
        createdAt = try container.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
        updatedAt = try container.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
    }

    /// Memberwise init for fixtures and local edits.
    init(id: String, orgId: String, name: String, targetBotId: String,
         active: Bool, createdAt: String, updatedAt: String) {
        self.id = id
        self.orgId = orgId
        self.name = name
        self.targetBotId = targetBotId
        self.active = active
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }

    static func == (lhs: WebhookTrigger, rhs: WebhookTrigger) -> Bool { lhs.id == rhs.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

/// One recorded delivery attempt (`DeliveryRow`,
/// webhook_trigger_routes.rs:79-89). `status` is one of
/// "pending" | "delivered" | "failed" (webhook-api.ts).
struct WebhookTriggerDelivery: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let triggerId: String
    /// `X-Webhook-Event` header of the inbound request; nil decodes to the
    /// receiver's own default at display time ("webhook.received",
    /// webhook_trigger_routes.rs:432-436).
    let event: String?
    let status: String
    /// HTTP status the receiver answered the inbound POST with.
    let responseStatus: Int?
    let error: String?
    let attempts: Int
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id, event, status, error, attempts
        case triggerId = "trigger_id"
        case responseStatus = "response_status"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decodeIfPresent(String.self, forKey: .id) ?? ""
        triggerId = try container.decodeIfPresent(String.self, forKey: .triggerId) ?? ""
        event = try container.decodeIfPresent(String.self, forKey: .event)
        status = try container.decodeIfPresent(String.self, forKey: .status) ?? "pending"
        responseStatus = try container.decodeIfPresent(Int.self, forKey: .responseStatus)
        error = try container.decodeIfPresent(String.self, forKey: .error)
        attempts = try container.decodeIfPresent(Int.self, forKey: .attempts) ?? 1
        createdAt = try container.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
        updatedAt = try container.decodeIfPresent(String.self, forKey: .updatedAt) ?? ""
    }
}

/// `GET /api/v1/webhook-triggers` envelope (`{ triggers, total }`).
struct WebhookTriggerListResponse: Decodable, Sendable {
    let triggers: [WebhookTrigger]
    let total: Int
}

/// Single-trigger envelope of `POST`/`GET`/`PATCH /api/v1/webhook-triggers[/:id]`
/// (`{ trigger }`).
struct WebhookTriggerResponse: Decodable, Sendable {
    let trigger: WebhookTrigger
}

/// `GET /api/v1/webhook-triggers/:id/deliveries` envelope
/// (`{ deliveries, total }`, last 100, newest first).
struct WebhookTriggerDeliveryListResponse: Decodable, Sendable {
    let deliveries: [WebhookTriggerDelivery]
    let total: Int
}

/// Body of `POST /api/v1/webhook-triggers` (`CreateTriggerBody`,
/// webhook_trigger_routes.rs:55-58). The server rejects empty/blank fields.
struct CreateWebhookTriggerBody: Encodable, Sendable {
    let name: String
    let targetBotId: String

    enum CodingKeys: String, CodingKey {
        case name
        case targetBotId = "target_bot_id"
    }
}

/// Body of `PATCH /api/v1/webhook-triggers/:id` (`UpdateTriggerBody`,
/// webhook_trigger_routes.rs:60-65). Optionals encode only when set
/// (synthesized `encodeIfPresent`) — the server 400s an empty body
/// ("no fields to update"), so callers must set at least one field.
struct UpdateWebhookTriggerBody: Encodable, Sendable {
    var name: String? = nil
    var targetBotId: String? = nil
    var active: Bool? = nil

    enum CodingKeys: String, CodingKey {
        case name
        case targetBotId = "target_bot_id"
        case active
    }
}
