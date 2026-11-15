import Foundation

/// Gateway-API client for Cowork sessions — `/api/v1/cowork/sessions`.
///
/// The cowork session routes live on the user's own allternit-api (the PWA
/// relays them; they are not cloud control-plane paths), so requests are
/// built with `APIClient.shared` (base URL + Bearer) and sent through
/// `InstanceConnection.runtimeAPIData`: the runtime relay to the active
/// computer in Release, direct to a local allternit-api in DEBUG. Distinct
/// from `CoworkTasksClient`, whose `/api/v1/tasks` is cloud-api's own.
final class CoworkSessionsClient: @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// `GET /api/v1/cowork/sessions?limit=<n>`.
    func listSessions(limit: Int = 30) async throws -> [CoworkSession] {
        let request = try await client.authorizedRequest(path: "cowork/sessions?limit=\(limit)")
        let response: CoworkSessionListResponse = try await send(request)
        return response.sessions
    }

    /// `POST /api/v1/cowork/sessions` — returns the new session id.
    @discardableResult
    func createSession(name: String, sessionMode: String = "regular") async throws -> String {
        var request = try await client.authorizedRequest(path: "cowork/sessions", method: "POST")
        request.httpBody = try JSONEncoder().encode(CreateCoworkSessionBody(name: name, sessionMode: sessionMode))
        let response: CreateCoworkSessionResponse = try await send(request)
        return response.session.id
    }

    /// `DELETE /api/v1/cowork/sessions/:id`.
    func deleteSession(id: String) async throws {
        let request = try await client.authorizedRequest(path: "cowork/sessions/\(id)", method: "DELETE")
        try await sendNoContent(request)
    }

    /// `PATCH /api/v1/cowork/sessions/:id`.
    func patchSession(id: String, checkpoint: String? = nil, status: String? = nil) async throws {
        var request = try await client.authorizedRequest(path: "cowork/sessions/\(id)", method: "PATCH")
        request.httpBody = try JSONEncoder().encode(PatchCoworkSessionBody(checkpoint: checkpoint, status: status))
        try await sendNoContent(request)
    }

    private func send<T: Decodable>(_ request: URLRequest) async throws -> T {
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    private func sendNoContent(_ request: URLRequest) async throws {
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
    }
}
