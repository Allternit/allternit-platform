import Foundation

/// Client for gizzi-code's cron routes (`cmd/gizzi-code/src/runtime/server/
/// routes/cron.ts`, mounted at `v1/cron` — `server.ts:463`).
///
/// Like `PtyClient`/`PermissionClient` this talks to gizzi-code
/// (resolved per call by `InstanceConnection`: explicit URL → mesh → relay) — cron jobs live on the gizzi-code server
/// itself, not on `allternit-api` (unlike `ProjectsClient`, which goes
/// through `APIClient.shared`'s relay-aware routing to a different backend).
final class CronClient: @unchecked Sendable {
    static let shared = CronClient()

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

    /// `GET v1/cron/jobs` (routes/cron.ts:41-62) — bare `CronJob[]`, no
    /// envelope.
    func listJobs() async throws -> [CronJob] {
        try await client().get(path: "v1/cron/jobs")
    }

    /// `GET v1/cron/jobs/:id` (routes/cron.ts:88-120).
    func getJob(id: String) async throws -> CronJob {
        try await client().get(path: "v1/cron/jobs/\(Self.escape(id))")
    }

    /// `POST v1/cron/jobs` (routes/cron.ts:63-87) with `type: "agent"` and
    /// `config: { prompt }` (`AgentJob`, cron/types.ts:118-128). `schedule`
    /// is sent as the raw string the user typed — the server's
    /// `parseScheduleToType` (cron/service.ts:220-222) accepts both cron
    /// expressions and natural language, so no client-side validation
    /// happens here; an invalid schedule surfaces as the server's own error.
    func createAgentJob(
        name: String,
        prompt: String,
        schedule: String,
        description: String? = nil,
        agentId: String? = nil,
        model: String? = nil,
        status: String? = nil,
        maxRetries: Int? = nil,
        timeoutSeconds: Int? = nil
    ) async throws -> CronJob {
        try await client().post(
            path: "v1/cron/jobs",
            body: CreateAgentJobRequest(
                name: name,
                type: "agent",
                schedule: schedule,
                config: .init(prompt: prompt, agentId: agentId, model: model),
                description: description,
                status: status,
                maxRetries: maxRetries,
                timeoutSeconds: timeoutSeconds
            )
        )
    }

    /// `PUT v1/cron/jobs/:id` (routes/cron.ts `cron.update`): the Edit form's save.
    func updateJob(id: String, _ request: UpdateCronJobRequest) async throws {
        try await client().put(path: "v1/cron/jobs/\(Self.escape(id))", body: request)
    }

    /// `PUT v1/cron/jobs/:id` (routes/cron.ts `cron.update`): a dragged
    /// heartbeat's new schedule, parsed on the server like create's.
    func updateSchedule(id: String, schedule: String) async throws {
        try await client().put(path: "v1/cron/jobs/\(Self.escape(id))", body: ["schedule": schedule])
    }

    /// `POST v1/cron/jobs/:id/pause` (routes/cron.ts:178-208).
    func pause(id: String) async throws {
        try await client().post(path: "v1/cron/jobs/\(Self.escape(id))/pause")
    }

    /// `POST v1/cron/jobs/:id/resume` (routes/cron.ts:209-239).
    func resume(id: String) async throws {
        try await client().post(path: "v1/cron/jobs/\(Self.escape(id))/resume")
    }

    /// `POST v1/cron/jobs/:id/run` (routes/cron.ts:240-269) — manual trigger.
    /// The response is the newly created `CronRun`, which callers refresh
    /// separately via `listRuns`, so it's discarded here.
    func runNow(id: String) async throws {
        try await client().post(path: "v1/cron/jobs/\(Self.escape(id))/run")
    }

    /// `DELETE v1/cron/jobs/:id` (routes/cron.ts:153-177).
    func deleteJob(id: String) async throws {
        try await client().delete(path: "v1/cron/jobs/\(Self.escape(id))")
    }

    /// `GET v1/cron/jobs/:id/runs` (routes/cron.ts:270-298) — bare
    /// `CronRun[]`, no envelope.
    func listRuns(jobId: String) async throws -> [CronRun] {
        try await client().get(path: "v1/cron/jobs/\(Self.escape(jobId))/runs")
    }

    /// Web uses `encodeURIComponent` on path ids (parity with
    /// AgentChatClient/PtyClient).
    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
