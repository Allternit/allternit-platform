import Foundation

// MARK: - Wire models (cmd/allternit-cloud-api/src/routes/artifacts_v2.rs)
//
// One account-level store of typed artifacts every surface reads and writes
// (Desktop, ai.allternit.com, the PWA, this app). Shapes follow
// `summary_json` / `artifact_json` / `version_json` in artifacts_v2.rs and
// the web client's types (allternit-ai src/lib/artifacts/types.ts).

/// The v2 kinds (artifacts/kinds.rs KINDS). Unknown kinds decode as raw
/// strings on the models, so a newer server never breaks the list.
enum ArtifactV2Kind: String, CaseIterable, Sendable, Identifiable {
    case doc, sheet, slides, design, dashboard, ops, motion, page, card, diagram, image, code, pdf, video

    var id: String { rawValue }

    /// Kinds offered as library filters (the web's FILTER_KINDS order).
    static let filterKinds: [ArtifactV2Kind] = [
        .doc, .slides, .sheet, .design, .dashboard, .motion, .page, .card, .diagram, .image, .code, .pdf,
    ]

    var label: String {
        switch self {
        case .doc: return "Docs"
        case .sheet: return "Sheets"
        case .slides: return "Slides"
        case .design: return "Designs"
        case .dashboard: return "Dashboards"
        case .ops: return "Ops sheets"
        case .motion: return "Motion"
        case .page: return "Pages"
        case .card: return "Cards"
        case .diagram: return "Diagrams"
        case .image: return "Images"
        case .code: return "Code"
        case .pdf: return "PDFs"
        case .video: return "Videos"
        }
    }

    var noun: String {
        switch self {
        case .doc: return "Doc"
        case .sheet: return "Sheet"
        case .slides: return "Deck"
        case .design: return "Design"
        case .dashboard: return "Dashboard"
        case .ops: return "Ops sheet"
        case .motion: return "Motion"
        case .page: return "Page"
        case .card: return "Card"
        case .diagram: return "Diagram"
        case .image: return "Image"
        case .code: return "Code"
        case .pdf: return "PDF"
        case .video: return "Video"
        }
    }

    var systemImage: String {
        switch self {
        case .doc: return "doc.text"
        case .sheet: return "tablecells"
        case .slides: return "rectangle.on.rectangle"
        case .design: return "paintbrush.pointed"
        case .dashboard: return "chart.bar.xaxis"
        case .ops: return "gauge.with.dots.needle.33percent"
        case .motion: return "film.stack"
        case .page: return "globe"
        case .card: return "square.text.square"
        case .diagram: return "point.3.connected.trianglepath.dotted"
        case .image: return "photo"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .pdf: return "doc.richtext"
        case .video: return "video"
        }
    }

    /// SF Symbol for any kind string, known or not.
    static func systemImage(for kind: String) -> String {
        ArtifactV2Kind(rawValue: kind)?.systemImage ?? "doc"
    }
}

struct ArtifactV2Owner: Decodable, Hashable, Sendable {
    let id: String
    let name: String?
    let imageURL: String?

    enum CodingKeys: String, CodingKey {
        case id, name
        case imageURL = "image_url"
    }
}

/// `origin` — the owner sees every field; others see `surface` /
/// `legacy_source` only (origin_json).
struct ArtifactV2Origin: Codable, Hashable, Sendable {
    var surface: String?
    var sessionId: String?
    var messageId: String?
    var computerId: String?
    var legacySource: String?
    var legacyId: String?

    init(surface: String? = nil, sessionId: String? = nil) {
        self.surface = surface
        self.sessionId = sessionId
    }

    enum CodingKeys: String, CodingKey {
        case surface
        case sessionId = "session_id"
        case messageId = "message_id"
        case computerId = "computer_id"
        case legacySource = "legacy_source"
        case legacyId = "legacy_id"
    }
}

/// A list row (`summary_json`).
struct ArtifactV2Summary: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let kind: String
    let title: String
    let icon: String?
    let owner: ArtifactV2Owner
    let updatedAt: String
    let currentVersion: Int
    /// private | people | org | link
    let visibility: String
    /// owner | edit | comment | view
    let myAccess: String
    let origin: ArtifactV2Origin?
    let runtimeVersion: Int?
    let thumbnailURL: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, title, icon, owner, visibility, origin
        case updatedAt = "updated_at"
        case currentVersion = "current_version"
        case myAccess = "my_access"
        case runtimeVersion = "runtime_version"
        case thumbnailURL = "thumbnail_url"
    }

    var knownKind: ArtifactV2Kind? { ArtifactV2Kind(rawValue: kind) }
    var isMine: Bool { myAccess == "owner" }
    /// `runtime_version: 1` records were copied from a pre-v2 store.
    var isLegacy: Bool { runtimeVersion == 1 }
    var updatedDate: Date? { ArtifactV2Dates.parse(updatedAt) }
}

/// `version_json` with the body.
struct ArtifactV2Version: Decodable, Hashable, Sendable {
    let version: Int
    let body: String
    let bodyFormat: String
    let meta: [String: JSONValue]?
    let authorId: String?
    let createdAt: String?

    enum CodingKeys: String, CodingKey {
        case version, body, meta
        case bodyFormat = "body_format"
        case authorId = "author_id"
        case createdAt = "created_at"
    }

    func metaString(_ key: String) -> String? {
        if case .string(let value)? = meta?[key] { return value }
        return nil
    }

    // JSONValue is only Equatable, so hash on the fields that identify a version.
    func hash(into hasher: inout Hasher) {
        hasher.combine(version)
        hasher.combine(body)
    }
}

/// The full artifact (`artifact_json`): the summary plus the version the
/// caller sees (viewers see `shared_version`, editors the latest).
struct ArtifactV2: Decodable, Identifiable, Sendable {
    let id: String
    let kind: String
    let title: String
    let icon: String?
    let owner: ArtifactV2Owner
    let updatedAt: String
    let currentVersion: Int
    let visibility: String
    let myAccess: String
    let origin: ArtifactV2Origin?
    let runtimeVersion: Int?
    let thumbnailURL: String?
    let templateId: String?
    let capabilities: JSONValue?
    let sharedVersion: Int?
    let createdAt: String?
    let version: ArtifactV2Version

    enum CodingKeys: String, CodingKey {
        case id, kind, title, icon, owner, visibility, origin, capabilities, version
        case updatedAt = "updated_at"
        case currentVersion = "current_version"
        case myAccess = "my_access"
        case runtimeVersion = "runtime_version"
        case thumbnailURL = "thumbnail_url"
        case templateId = "template_id"
        case sharedVersion = "shared_version"
        case createdAt = "created_at"
    }

    var knownKind: ArtifactV2Kind? { ArtifactV2Kind(rawValue: kind) }
    var canEdit: Bool { myAccess == "owner" || myAccess == "edit" }
    var isMine: Bool { myAccess == "owner" }
}

/// `GET /api/v2/artifacts` page.
struct ArtifactV2ListPage: Decodable, Sendable {
    let items: [ArtifactV2Summary]
    let nextCursor: String?
    let archiveSupported: Bool?

    enum CodingKeys: String, CodingKey {
        case items
        case nextCursor = "next_cursor"
        case archiveSupported = "archive_supported"
    }
}

/// `GET /api/v2/org/artifact-settings` (OrgSettings). Signed-in users with
/// no org get the defaults (all on).
struct ArtifactV2OrgSettings: Decodable, Sendable {
    let enabled: Bool
    let templates: [String: Bool]?
    let externalSharing: Bool?
    let outsideInvites: Bool?
    let presence: Bool?
    let connectors: Bool?

    enum CodingKeys: String, CodingKey {
        case enabled, templates, presence, connectors
        case externalSharing = "external_sharing"
        case outsideInvites = "outside_invites"
    }

    /// A template is off only when the org turned it off explicitly.
    func allows(kind: String) -> Bool {
        guard enabled else { return false }
        return templates?[kind] ?? true
    }
}

/// `POST /api/v2/artifacts` body (CreateRequest).
struct ArtifactV2CreateInput: Encodable, Sendable {
    /// Client id (`art_` + [A-Za-z0-9_-], ≤128): create is idempotent for an
    /// id the caller already owns (returns that artifact, 200).
    var id: String?
    var kind: String
    var title: String
    var body: String
    var bodyFormat: String
    var meta: [String: JSONValue]?
    var origin: ArtifactV2Origin?

    enum CodingKeys: String, CodingKey {
        case id, kind, title, body, meta, origin
        case bodyFormat = "body_format"
    }
}

/// `GET /api/v2/artifacts` query.
struct ArtifactV2ListQuery: Sendable, Equatable {
    /// mine | shared | all
    var scope: String = "all"
    var kind: String? = nil
    var query: String? = nil
    var cursor: String? = nil
    var limit: Int = 50
    var archived: Bool = false
}

enum ArtifactV2Dates {
    /// Server timestamps are RFC 3339 with microseconds (`ts`).
    static func parse(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
    }
}

// MARK: - Client

/// Artifacts v2 client — `/api/v2/artifacts*` on allternit-cloud-api.
///
/// Requests go DIRECT to `AppConfig.cloudAPIBaseURL` (never the runtime
/// relay), with the Clerk session token first: the server resolves Clerk
/// users with their org and email, which "Shared with me" (org + email
/// invites) needs. A paired runtime device token is accepted as a fallback
/// (artifacts_v2.rs `caller`) but acts as the owner with no org or email.
final class ArtifactsV2Client: @unchecked Sendable {
    static let shared = ArtifactsV2Client()

    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = AppConfig.cloudAPIBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// The bearer v2 should see: Clerk first, then the runtime device token.
    static func bearerToken() async -> String? {
        if let clerk = try? await AuthManager.shared.getToken(), !clerk.isEmpty {
            return clerk
        }
        if let device = try? await AuthManager.shared.effectiveToken(), !device.isEmpty {
            return device
        }
        return nil
    }

    // MARK: Library

    func list(_ query: ArtifactV2ListQuery) async throws -> ArtifactV2ListPage {
        var items: [URLQueryItem] = [
            URLQueryItem(name: "scope", value: query.scope),
            URLQueryItem(name: "limit", value: String(query.limit)),
        ]
        if let kind = query.kind, !kind.isEmpty { items.append(URLQueryItem(name: "kind", value: kind)) }
        if let q = query.query?.trimmingCharacters(in: .whitespacesAndNewlines), !q.isEmpty {
            items.append(URLQueryItem(name: "q", value: q))
        }
        if let cursor = query.cursor, !cursor.isEmpty { items.append(URLQueryItem(name: "cursor", value: cursor)) }
        if query.archived { items.append(URLQueryItem(name: "archived", value: "true")) }
        return try await send(path: "api/v2/artifacts", query: items)
    }

    func get(id: String) async throws -> ArtifactV2 {
        try await send(path: "api/v2/artifacts/\(Self.escape(id))")
    }

    func create(_ input: ArtifactV2CreateInput) async throws -> ArtifactV2 {
        try await send(path: "api/v2/artifacts", method: "POST", body: try JSONEncoder().encode(input))
    }

    /// `POST /api/v2/artifacts/:id/versions` (AppendRequest) — saves an edit
    /// as a new version. `baseVersion` is the version the edit started from;
    /// the server answers 409 when someone saved after it. `meta` replaces
    /// the version meta (the server defaults it to `{}`), so pass the
    /// current version's meta to keep it.
    func appendVersion(id: String, baseVersion: Int, body: String, bodyFormat: String, meta: [String: JSONValue]?) async throws -> ArtifactV2 {
        struct Body: Encodable {
            let baseVersion: Int
            let body: String
            let bodyFormat: String
            let meta: [String: JSONValue]?
            let author: String
            enum CodingKeys: String, CodingKey {
                case body, meta, author
                case baseVersion = "base_version"
                case bodyFormat = "body_format"
            }
        }
        let payload = Body(baseVersion: baseVersion, body: body, bodyFormat: bodyFormat, meta: meta, author: "user")
        return try await send(path: "api/v2/artifacts/\(Self.escape(id))/versions", method: "POST", body: try JSONEncoder().encode(payload))
    }

    func delete(id: String) async throws {
        try await sendNoContent(path: "api/v2/artifacts/\(Self.escape(id))", method: "DELETE")
    }

    /// `PUT /api/v2/artifacts/:id/library` — per-viewer archive (any viewer).
    func setArchived(id: String, archived: Bool) async throws {
        struct Body: Encodable { let archived: Bool }
        try await sendNoContent(
            path: "api/v2/artifacts/\(Self.escape(id))/library",
            method: "PUT",
            body: try JSONEncoder().encode(Body(archived: archived))
        )
    }

    func orgSettings() async throws -> ArtifactV2OrgSettings {
        try await send(path: "api/v2/org/artifact-settings")
    }

    // MARK: Transport

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? id
    }

    private func request(path: String, query: [URLQueryItem] = [], method: String, body: Data?) async throws -> URLRequest {
        guard var components = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false) else {
            throw APIError.invalidResponse
        }
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url else { throw APIError.invalidResponse }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        guard let token = await Self.bearerToken() else {
            throw APIError.httpError(statusCode: 401, message: "Sign in to see your artifacts.")
        }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw APIError.transport(error)
        }
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw APIError.httpError(statusCode: http.statusCode, message: Self.message(status: http.statusCode, data: data))
        }
        return data
    }

    private func send<T: Decodable>(path: String, query: [URLQueryItem] = [], method: String = "GET", body: Data? = nil) async throws -> T {
        let data = try await perform(try await request(path: path, query: query, method: method, body: body))
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    private func sendNoContent(path: String, method: String, body: Data? = nil) async throws {
        _ = try await perform(try await request(path: path, method: method, body: body))
    }

    /// Server `{message|error}` first, then the web client's wording
    /// (lib/artifacts/api.ts errorMessage).
    private static func message(status: Int, data: Data) -> String {
        struct Body: Decodable { let message: String?; let error: String? }
        if let body = try? JSONDecoder().decode(Body.self, from: data) {
            if let m = body.message, !m.isEmpty { return m }
            if let e = body.error, !e.isEmpty { return e }
        }
        switch status {
        case 401: return "Sign in to see your artifacts."
        case 403: return "You don’t have access to do that."
        case 404: return "This artifact doesn’t exist or isn’t shared with you."
        case 409: return "Someone saved a newer version."
        case 413: return "This artifact is too large (16 MB max)."
        default: return "Request failed (\(status))."
        }
    }
}

extension APIError {
    /// HTTP status for `.httpError`, nil otherwise.
    var httpStatus: Int? {
        if case .httpError(let status, _) = self { return status }
        return nil
    }
}
