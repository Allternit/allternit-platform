import Foundation

/// Client for gizzi-code's routines routes (`cmd/gizzi-code/src/runtime/
/// server/routes/automations.ts:104-225`, mounted at `v1/automations` on
/// gizzi-code's own server — `server.ts:367,455`).
///
/// Like `CronClient`/`PtyClient`/`PermissionClient` this always connects
/// to gizzi-code, resolved per call by `InstanceConnection`, — routines live on the gizzi-code
/// server itself, not on `allternit-api` (unlike `ProjectsClient`, which goes
/// through `APIClient.shared`'s relay-aware routing to a different backend).
///
/// There is no `GET /routines/:id` or a runs/history endpoint (unlike cron's
/// `/jobs/:id` and `/jobs/:id/runs`) — the detail view works from the
/// already-fetched list item and refetches the whole list after mutations.
final class RoutinesClient: @unchecked Sendable {
    static let shared = RoutinesClient()

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

    /// `GET v1/automations/routines` (automations.ts:104-121) — bare
    /// `Routine[]`, no envelope.
    func listRoutines() async throws -> [Routine] {
        try await client().get(path: "v1/automations/routines")
    }

    /// `POST v1/automations/routines` (automations.ts:122-153), body
    /// validated against `RoutineCreateSchema` (automations.ts:15-22).
    /// Returns the created row, 201.
    func createRoutine(
        name: String,
        steps: [RoutineStep],
        trigger: String?,
        schedule: String?
    ) async throws -> Routine {
        try await client().post(
            path: "v1/automations/routines",
            body: CreateRoutineRequest(name: name, steps: steps, trigger: trigger, schedule: schedule)
        )
    }

    /// `POST v1/automations/routines/:id/run` (automations.ts:201-225) — no
    /// body. Triggers `RoutineEngine.startRoutine(id)` server-side, async/
    /// fire-and-forget. Response is `{success: true, state: "running"}`;
    /// discarded here since callers refresh the list to pick up the new
    /// `state`.
    func runRoutine(id: String) async throws {
        try await client().post(path: "v1/automations/routines/\(Self.escape(id))/run")
    }

    /// `DELETE v1/automations/routines/:id` (automations.ts:183-200).
    func deleteRoutine(id: String) async throws {
        try await client().delete(path: "v1/automations/routines/\(Self.escape(id))")
    }

    /// Web uses `encodeURIComponent` on path ids (parity with
    /// AgentChatClient/PtyClient/CronClient).
    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
