import Foundation

/// Gateway-API client for Cowork sessions — `/api/v1/cowork/sessions`.
///
/// Routes through `APIClient.shared` / `AppConfig.apiBaseURL` (the allternit-api
/// Axum gateway), NOT the cloud API used by `CoworkTasksClient`.
final class CoworkSessionsClient: @unchecked Sendable {
    /// `GET /api/v1/cowork/sessions?limit=<n>`.
    func listSessions(limit: Int = 30) async throws -> [CoworkSession] {
        let response: CoworkSessionListResponse = try await APIClient.shared.get(
            path: "cowork/sessions?limit=\(limit)"
        )
        return response.sessions
    }

    /// `POST /api/v1/cowork/sessions` — returns the new session id.
    @discardableResult
    func createSession(name: String, sessionMode: String = "regular") async throws -> String {
        let response: CreateCoworkSessionResponse = try await APIClient.shared.post(
            path: "cowork/sessions",
            body: CreateCoworkSessionBody(name: name, sessionMode: sessionMode)
        )
        return response.session.id
    }

    /// `DELETE /api/v1/cowork/sessions/:id`.
    func deleteSession(id: String) async throws {
        try await APIClient.shared.delete(path: "cowork/sessions/\(id)")
    }

    /// `PATCH /api/v1/cowork/sessions/:id`.
    func patchSession(id: String, checkpoint: String? = nil, status: String? = nil) async throws {
        try await APIClient.shared.patch(
            path: "cowork/sessions/\(id)",
            body: PatchCoworkSessionBody(checkpoint: checkpoint, status: status)
        )
    }
}
