import Foundation

/// A routine run, newest first in `recentRuns` (`?recent_runs=N`).
struct AutomationRunTick: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let status: String
    let startedAt: String?
    let finishedAt: String?
    let durationMs: Double?
    let reason: String?
    let apps: [String]?
    let model: String?
}

/// `/api/v1/automation/routines` row (allternit-api automation_routes.rs) —
/// the routines bots run (the PWA's automation-api.ts `Routine`).
struct AutomationRoutine: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let agentId: String?
    let goalId: String?
    let name: String
    let description: String?
    let status: String
    let scheduleType: String
    let scheduleExpression: String
    let timezone: String?
    let nextRunAt: String?
    let lastRunAt: String?
    let createdAt: String
    let recentRuns: [AutomationRunTick]?

    enum CodingKeys: String, CodingKey {
        case id, name, description, status, timezone, recentRuns
        case agentId = "agent_id"
        case goalId = "goal_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case nextRunAt = "next_run_at"
        case lastRunAt = "last_run_at"
        case createdAt = "created_at"
    }

    var isEnabled: Bool { status == "active" }
}

/// `GET …/routines/:id/live`: the run in progress.
struct AutomationLiveRun: Decodable, Equatable, Sendable {
    struct Step: Decodable, Equatable, Sendable {
        let content: String?
        let status: String?
    }
    let running: Bool
    let runId: String?
    let startedAt: String?
    let sessionId: String?
    let steps: [Step]?
    let apps: [String]?
    let model: String?
    let text: String?
    let canStop: Bool?
}

/// `/api/v1/automation/goals` row.
struct AutomationGoal: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let agentId: String?
    let title: String
    let description: String?
    let status: String
    let priority: String
    let targetDate: String?
    let progress: Double
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id, title, description, status, priority, progress
        case agentId = "agent_id"
        case targetDate = "target_date"
        case createdAt = "created_at"
    }
}

struct CreateAutomationGoalRequest: Encodable, Sendable {
    let title: String
    let description: String?
    let priority: String
    let agentId: String?
    let targetDate: String?

    enum CodingKeys: String, CodingKey {
        case title, description, priority
        case agentId = "agent_id"
        case targetDate = "target_date"
    }
}

struct UpdateAutomationGoalRequest: Encodable, Sendable {
    let status: String?
    let progress: Double?
}

struct CreateAutomationRoutineRequest: Encodable, Sendable {
    let name: String
    let description: String?
    let agentId: String
    let scheduleType: String
    let scheduleExpression: String
    let timezone: String
    let executionDomain: String
    let config: [String: String]
    /// Optional row metadata (the web's `bot-routine` source / botName /
    /// title / fromTemplate). Omitted from the body when nil.
    var metadata: [String: JSONValue]? = nil

    enum CodingKeys: String, CodingKey {
        case name, description, timezone, config, metadata
        case agentId = "agent_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case executionDomain = "execution_domain"
    }
}

private struct UpdateRoutineStatusRequest: Encodable, Sendable {
    let status: String
}

private struct UpdateRoutineScheduleRequest: Encodable, Sendable {
    let scheduleType: String
    let scheduleExpression: String
    let timezone: String

    enum CodingKeys: String, CodingKey {
        case timezone
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
    }
}

private struct RunsEnvelope: Decodable, Sendable {
    let runs: [AutomationRunTick]?
}

/// The `/api/v1/automation/*` routes on allternit-api, through
/// `APIClient.shared` (so they follow its direct / relay routing to the
/// user's computer). Separate from `RoutinesClient`, which talks to
/// gizzi-code's own `v1/automations` server.
final class AutomationClient: @unchecked Sendable {
    static let shared = AutomationClient()
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    func listRoutines(recentRuns: Int = 14) async throws -> [AutomationRoutine] {
        try await client.get(path: "automation/routines?recent_runs=\(recentRuns)")
    }

    func createRoutine(_ request: CreateAutomationRoutineRequest) async throws -> AutomationRoutine {
        try await client.post(path: "automation/routines", body: request)
    }

    func setEnabled(id: String, enabled: Bool) async throws {
        try await client.put(path: "automation/routines/\(Self.escape(id))",
                             body: UpdateRoutineStatusRequest(status: enabled ? "active" : "paused"))
    }

    func updateSchedule(id: String, cron: String, timezone: String) async throws {
        try await client.put(path: "automation/routines/\(Self.escape(id))",
                             body: UpdateRoutineScheduleRequest(scheduleType: "cron", scheduleExpression: cron, timezone: timezone))
    }

    func deleteRoutine(id: String) async throws {
        try await client.delete(path: "automation/routines/\(Self.escape(id))")
    }

    func runNow(id: String) async throws {
        try await client.post(path: "automation/routines/\(Self.escape(id))/run")
    }

    func live(id: String) async throws -> AutomationLiveRun {
        try await client.get(path: "automation/routines/\(Self.escape(id))/live")
    }

    func cancelRun(routineId: String, runId: String) async throws {
        try await client.post(path: "automation/routines/\(Self.escape(routineId))/runs/\(Self.escape(runId))/cancel")
    }

    func listGoals() async throws -> [AutomationGoal] {
        try await client.get(path: "automation/goals")
    }

    func createGoal(_ request: CreateAutomationGoalRequest) async throws -> AutomationGoal {
        try await client.post(path: "automation/goals", body: request)
    }

    func updateGoal(id: String, _ request: UpdateAutomationGoalRequest) async throws {
        try await client.put(path: "automation/goals/\(Self.escape(id))", body: request)
    }

    func deleteGoal(id: String) async throws {
        try await client.delete(path: "automation/goals/\(Self.escape(id))")
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
