import Foundation

// -----------------------------------------------------------------------------
// Cowork session REST models — base path /api/v1/cowork/sessions.
//
// Mirrors the Rust producers in cmd/allternit-api/src/cowork_routes.rs
// (`SessionRow`). The API emits snake_case keys on the wire; Swift properties
// stay camelCase via explicit CodingKeys.
// -----------------------------------------------------------------------------

/// One cowork session (`GET /api/v1/cowork/sessions` → `{sessions: [...]}`).
struct CoworkSession: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let userId: String
    let projectId: String?
    let title: String?
    let status: String
    let mode: String
    let checkpoint: String?
    let metadata: String?
    let startedAt: String?
    let completedAt: String?
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id, title, status, mode, checkpoint, metadata
        case userId = "user_id"
        case projectId = "project_id"
        case startedAt = "started_at"
        case completedAt = "completed_at"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    internal init(id: String, userId: String, projectId: String?, title: String?, status: String, mode: String, checkpoint: String?, metadata: String?, startedAt: String?, completedAt: String?, createdAt: String, updatedAt: String) {
        self.id = id
        self.userId = userId
        self.projectId = projectId
        self.title = title
        self.status = status
        self.mode = mode
        self.checkpoint = checkpoint
        self.metadata = metadata
        self.startedAt = startedAt
        self.completedAt = completedAt
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }

    /// Display title fallback: stored title, then a short timestamp.
    var displayTitle: String {
        if !AgentSession.isRawDefaultName(title) { return (title ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let date = (try? Date(createdAt, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)))
            ?? (try? Date(createdAt, strategy: Date.ISO8601FormatStyle()))
        guard let date else { return "New session" }
        return "New session · " + date.formatted(.dateTime.month(.abbreviated).day())
    }
}

/// `GET /api/v1/cowork/sessions` envelope (`{ sessions: [...] }`).
struct CoworkSessionListResponse: Decodable, Sendable {
    let sessions: [CoworkSession]
}

/// `POST /api/v1/cowork/sessions` body (`CreateSessionBody`).
struct CreateCoworkSessionBody: Encodable, Sendable {
    let name: String
    let sessionMode: String

    enum CodingKeys: String, CodingKey {
        case name
        case sessionMode = "session_mode"
    }
}

/// `POST /api/v1/cowork/sessions` response (`{ session: { id } }`).
struct CreateCoworkSessionResponse: Decodable, Sendable {
    struct Created: Decodable, Sendable {
        let id: String
    }
    let session: Created
}

/// `PATCH /api/v1/cowork/sessions/:id` body.
struct PatchCoworkSessionBody: Encodable, Sendable {
    let checkpoint: String?
    let status: String?
}
