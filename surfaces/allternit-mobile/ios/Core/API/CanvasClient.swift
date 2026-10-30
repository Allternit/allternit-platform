import Foundation

/// One entry of a canvas's `components` array. The web persists artifacts as
/// `{ type: 'artifact', artifactId, kind, title, content, url }`
/// (mode-session-store.ts persistArtifactToCanvas); every field except `type`
/// is optional on the wire, and non-artifact component shapes decode with
/// whatever subset matches.
struct CanvasComponent: Codable, Sendable {
    let type: String?
    let artifactId: String?
    let kind: String?
    let title: String?
    let content: String?
    let url: String?
}

/// A persisted canvas row (canvas_routes.rs CanvasResponse). `components` and
/// `metadata` are arbitrary JSON server-side; non-matching shapes decode as
/// empty/default rather than failing the whole record.
struct CanvasRecord: Decodable, Sendable {
    let id: String
    let sessionId: String
    let title: String?
    let components: [CanvasComponent]
    let artifactKey: String?
    let version: Int
    let updatedAt: String
    /// Metadata JSON from the backend; may hold a canvas protocol spec.
    let metadata: JSONValue?

    enum CodingKeys: String, CodingKey {
        case id, title, components, metadata
        case version
        case artifactKey = "artifact_key"
        case sessionId = "session_id"
        case updatedAt = "updated_at"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        sessionId = try container.decode(String.self, forKey: .sessionId)
        title = try container.decodeIfPresent(String.self, forKey: .title)
        components = (try? container.decode([CanvasComponent].self, forKey: .components)) ?? []
        artifactKey = try container.decodeIfPresent(String.self, forKey: .artifactKey)
        version = try container.decodeIfPresent(Int.self, forKey: .version) ?? 1
        updatedAt = try container.decode(String.self, forKey: .updatedAt)
        metadata = try? container.decode(JSONValue.self, forKey: .metadata)
    }

    /// Decodes a `CanvasSpec` from `metadata` when one was stored.
    var spec: CanvasSpec? {
        guard let metadata else { return nil }
        guard let data = try? JSONEncoder().encode(metadata) else { return nil }
        return try? JSONDecoder().decode(CanvasSpec.self, from: data)
    }
}

/// Canvas CRUD over the routes in canvas_routes.rs (Canvas Protocol specs).
/// Artifacts no longer round-trip through canvases: the library is Artifacts
/// v2 (`ArtifactsV2Client`, `/api/v2/artifacts`).
struct CanvasClient: Sendable {
    private struct ListResponse: Decodable {
        let canvases: [CanvasRecord]
    }

    /// Payload that includes a Canvas Protocol spec in `metadata`.
    private struct SpecPayload: Encodable {
        let title: String
        let components: [CanvasComponent]
        let metadata: CanvasSpec
    }

    /// Minimal PATCH payload that only updates `metadata`.
    private struct SpecUpdatePayload: Encodable {
        let metadata: CanvasSpec
    }

    /// POST returns a slim `{ id, session_id }`, not the full row
    /// (canvas_routes.rs create_canvas).
    private struct CreateResponse: Decodable {
        let id: String
    }

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// GET /api/v1/canvases → the authenticated user's newest canvases,
    /// including artifacts published under non-chat synthetic session ids.
    func listCanvases() async throws -> [CanvasRecord] {
        let envelope: ListResponse = try await client.get(path: "canvases")
        return envelope.canvases
    }

    /// GET /api/v1/agent-sessions/:id/canvases → { canvases: [...] }
    func listCanvases(sessionId: String) async throws -> [CanvasRecord] {
        let escaped = sessionId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionId
        let envelope: ListResponse = try await client.get(path: "agent-sessions/\(escaped)/canvases")
        return envelope.canvases
    }

    // MARK: - Canvas Protocol (item #56)

    /// POST /api/v1/agent-sessions/:id/canvases — create a canvas carrying a
    /// Canvas Protocol spec in its metadata. Components default to empty.
    func createCanvas(sessionId: String, spec: CanvasSpec, components: [CanvasComponent] = []) async throws -> String {
        let escaped = sessionId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionId
        let created: CreateResponse = try await client.post(
            path: "agent-sessions/\(escaped)/canvases",
            body: SpecPayload(
                title: spec.title ?? "Canvas",
                components: components,
                metadata: spec
            )
        )
        return created.id
    }

    /// PATCH /api/v1/canvases/:id — update the canvas metadata to store a new
    /// or revised Canvas Protocol spec.
    func updateCanvasSpec(canvasId: String, spec: CanvasSpec) async throws {
        let escaped = canvasId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? canvasId
        try await client.patch(
            path: "canvases/\(escaped)",
            body: SpecUpdatePayload(metadata: spec)
        )
    }

    /// GET /api/v1/canvases/:id — returns the canvas row; decode `spec` from
    /// `CanvasRecord.metadata`.
    func getCanvas(canvasId: String) async throws -> CanvasRecord {
        let escaped = canvasId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? canvasId
        return try await client.get(path: "canvases/\(escaped)")
    }
}
