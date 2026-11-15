import Foundation

// MARK: - Models (services/studio-spec/openapi/studio-api.yaml)

struct StudioJob: Decodable, Identifiable, Equatable, Sendable {
    struct Spec: Decodable, Equatable, Sendable {
        struct Base: Decodable, Equatable, Sendable { let repo: String }
        let engine: String
        let base: Base
        let method: String
    }

    let id: String
    let state: String
    let spec: Spec
    let createdAt: String
    let error: String?
    let lastSeq: Int?

    enum CodingKeys: String, CodingKey {
        case id, state, spec, error
        case createdAt = "created_at"
        case lastSeq = "last_seq"
    }

    var isFinished: Bool { ["succeeded", "failed", "cancelled"].contains(state) }
}

struct StudioJobEvent: Decodable, Equatable, Identifiable, Sendable {
    let seq: Int
    let at: String
    let kind: String
    let message: String
    let progress: Double?
    var id: Int { seq }
}

struct StudioEventsPage: Decodable, Equatable, Sendable {
    let state: String
    let events: [StudioJobEvent]
}

/// Accumulates polled events: pages with `after=<lastSeq>`, keeps a bounded
/// log tail and the latest progress. Pure so it can be unit tested.
struct StudioLogTail: Equatable, Sendable {
    static let maxLines = 50
    private(set) var lines: [StudioJobEvent] = []
    private(set) var lastSeq = 0
    private(set) var progress: Double?
    private(set) var state: String?

    mutating func apply(_ page: StudioEventsPage) {
        state = page.state
        for event in page.events where event.seq > lastSeq {
            lastSeq = event.seq
            if let p = event.progress { progress = p }
            if event.kind == "progress" && event.message.isEmpty { continue }
            lines.append(event)
        }
        if lines.count > Self.maxLines { lines.removeFirst(lines.count - Self.maxLines) }
    }
}

// MARK: - Client

/// Reads and cancels Freeweights Studio jobs on the user's computer:
/// allternit-api `/api/v1/studio`, reached through
/// `InstanceConnection.runtimeAPIData` (cloud runtime relay in Release).
/// The relay times out at 90 s, so callers poll; nothing streams.
/// Jobs are created on Desktop or web, never here.
final class StudioJobsClient: @unchecked Sendable {
    static let shared = StudioJobsClient()

    private let client: APIClient
    private let baseURL: URL

    init(client: APIClient = .shared, baseURL: URL = StudioJobsClient.defaultBaseURL) {
        self.client = client
        self.baseURL = baseURL
    }

    /// `{apiBase}/api/v1/studio`.
    static let defaultBaseURL: URL = {
        let api = AppConfig.apiBaseURL
        let apiBase = api.lastPathComponent == "v1" ? api.deletingLastPathComponent() : api
        return apiBase.appendingPathComponent("v1").appendingPathComponent("studio")
    }()

    private func request<T: Decodable>(_ path: String, query: [URLQueryItem] = [], method: String = "GET") async throws -> T {
        var url = baseURL.appendingPathComponent(path)
        if !query.isEmpty, var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            parts.queryItems = query
            url = parts.url ?? url
        }
        let req = try await client.authorizedRequest(url: url, method: method)
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: req, session: client.session)
        try client.validate(response, data: data, method: method)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    func listJobs() async throws -> [StudioJob] {
        struct Envelope: Decodable { let jobs: [StudioJob] }
        let envelope: Envelope = try await request("jobs")
        return envelope.jobs
    }

    func job(id: String) async throws -> StudioJob {
        try await request("jobs/\(id)")
    }

    func events(id: String, after seq: Int) async throws -> StudioEventsPage {
        try await request("jobs/\(id)/events", query: [URLQueryItem(name: "after", value: String(seq))])
    }

    @discardableResult
    func cancel(id: String) async throws -> StudioJob {
        try await request("jobs/\(id)/cancel", method: "POST")
    }
}
