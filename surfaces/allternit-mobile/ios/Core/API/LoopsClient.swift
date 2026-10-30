import Foundation

/// Client for gizzi-code's loops routes (`cmd/gizzi-code/src/runtime/server/
/// routes/automations.ts:228-344`, mounted at `v1/automations` on
/// gizzi-code's own server — `server.ts:367,455`).
///
/// Like `RoutinesClient`/`CronClient`/`PtyClient`/`PermissionClient` this
/// talks to gizzi-code (resolved per call by `InstanceConnection`) — loops live on
/// the gizzi-code server itself, not on `allternit-api` (unlike
/// `ProjectsClient`, which goes through `APIClient.shared`'s relay-aware
/// routing to a different backend).
///
/// There is no `GET /loops/:id` or a runs/history endpoint (same as
/// Routines) — the detail view works from the already-fetched list item and
/// refetches the whole list after mutations.
final class LoopsClient: @unchecked Sendable {
    static let shared = LoopsClient()

    private let endpointBaseURL: URL

    init(baseURL: URL = AppConfig.gizziCodeBaseURL) {
        self.endpointBaseURL = baseURL
    }

    /// Per-call target: the given base URL, or — for the static default —
    /// whatever `InstanceConnection.resolve()` picks now (explicit URL →
    /// mesh → the relay to the user's computer).
    private func client() async throws -> APIClient {
        try await InstanceConnection.gizziClient(for: endpointBaseURL)
    }

    /// `GET v1/automations/loops` (automations.ts:228-242) — bare `Loop[]`,
    /// no envelope.
    func listLoops() async throws -> [Loop] {
        try await client().get(path: "v1/automations/loops")
    }

    /// `POST v1/automations/loops` (automations.ts:243-272), body validated
    /// against `LoopCreateSchema` (automations.ts:33-39). Unlike Routines,
    /// the server starts the loop immediately on creation
    /// (`LoopEngine.startLoop`, fire-and-forget) — the returned row already
    /// has `state: "running"`. Returns the created row, 201.
    func createLoop(
        command: String,
        exitCondition: String?,
        maxIterations: Int
    ) async throws -> Loop {
        try await client().post(
            path: "v1/automations/loops",
            body: CreateLoopRequest(command: command, exitCondition: exitCondition, maxIterations: maxIterations)
        )
    }

    /// `POST v1/automations/loops/:id/run` (automations.ts:320-344) — no
    /// body. Restarts a stopped/finished loop: sets `state: "running"` and
    /// calls `LoopEngine.startLoop(id)` again, fire-and-forget. Response is
    /// `{success: true, state: "running"}`; discarded here since callers
    /// refresh the list to pick up the new `state`.
    func runLoop(id: String) async throws {
        try await client().post(path: "v1/automations/loops/\(Self.escape(id))/run")
    }

    /// `DELETE v1/automations/loops/:id` (automations.ts:302-319).
    func deleteLoop(id: String) async throws {
        try await client().delete(path: "v1/automations/loops/\(Self.escape(id))")
    }

    /// Web uses `encodeURIComponent` on path ids (parity with
    /// AgentChatClient/PtyClient/CronClient/RoutinesClient).
    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
