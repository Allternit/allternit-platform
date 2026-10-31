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

/// A JSON object that never fails its row's decode (a routine's `config`).
struct AutomationLooseObject: Decodable, Equatable, Sendable {
    let value: [String: JSONValue]

    init(from decoder: Decoder) throws {
        value = (try? decoder.singleValueContainer().decode([String: JSONValue].self)) ?? [:]
    }
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
    let config: AutomationLooseObject?

    enum CodingKeys: String, CodingKey {
        case id, name, description, status, timezone, recentRuns, config
        case agentId = "agent_id"
        case goalId = "goal_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case nextRunAt = "next_run_at"
        case lastRunAt = "last_run_at"
        case createdAt = "created_at"
    }

    var isEnabled: Bool { status == "active" }

    /// What the bot is told to do: `config.prompt`, else the description.
    var instruction: String {
        if case .string(let prompt)? = config?.value["prompt"], !prompt.isEmpty { return prompt }
        return description ?? ""
    }
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

/// `/api/v1/automation/loops` row (automation.types.ts `Loop`): the
/// continuous / monitoring tasks on Automation Tasks.
struct AutomationLoop: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let agentId: String?
    let goalId: String?
    let name: String
    let description: String?
    let status: String
    let scheduleType: String
    let scheduleExpression: String
    let createdAt: String?
    let updatedAt: String?
    let config: AutomationLooseObject?
    /// When it next runs (the local scheduler's cursor).
    let nextRunAt: String?
    let lastRunAt: String?
    /// Newest first, with `?recent_runs=N`.
    let recentRuns: [AutomationRunTick]?

    enum CodingKeys: String, CodingKey {
        case id, name, description, status, config, recentRuns
        case nextRunAt = "next_run_at"
        case lastRunAt = "last_run_at"
        case agentId = "agent_id"
        case goalId = "goal_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    /// What the executor is told to do: `config.prompt`, else the description.
    var instruction: String {
        if case .string(let prompt)? = config?.value["prompt"], !prompt.isEmpty { return prompt }
        return description ?? ""
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
    var title: String? = nil
    var description: String? = nil
    var priority: String? = nil
    /// "yyyy-MM-dd"; an empty string clears it.
    var targetDate: String? = nil

    enum CodingKeys: String, CodingKey {
        case status, progress, title, description, priority
        case targetDate = "target_date"
    }
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
    /// The goal this routine supports (the wizard's Associated Goal).
    var goalId: String? = nil

    enum CodingKeys: String, CodingKey {
        case name, description, timezone, config, metadata
        case goalId = "goal_id"
        case agentId = "agent_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case executionDomain = "execution_domain"
    }
}

/// `POST /api/v1/automation/loops` (automation-api.ts createLoop).
struct CreateAutomationLoopRequest: Encodable, Sendable {
    let name: String
    let description: String?
    let agentId: String?
    let scheduleType: String
    let scheduleExpression: String
    let executionDomain: String
    let config: [String: String]
    /// The goal this loop supports (the wizard's Associated Goal).
    var goalId: String? = nil

    enum CodingKeys: String, CodingKey {
        case name, description, config
        case goalId = "goal_id"
        case agentId = "agent_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
        case executionDomain = "execution_domain"
    }
}

/// `PUT /api/v1/automation/routines/:id` (UpdateRoutineRequest): only the
/// fields that changed are sent. `config` replaces the whole column, so it
/// carries the routine's other keys too. An empty `goalId` clears the goal.
struct UpdateAutomationRoutineRequest: Encodable, Sendable {
    var name: String? = nil
    var description: String? = nil
    var agentId: String? = nil
    var goalId: String? = nil
    var scheduleType: String? = nil
    var scheduleExpression: String? = nil
    var timezone: String? = nil
    var config: [String: JSONValue]? = nil

    enum CodingKeys: String, CodingKey {
        case name, description, timezone, config
        case agentId = "agent_id"
        case goalId = "goal_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
    }
}

/// `PUT /api/v1/automation/loops/:id` (UpdateLoopRequest): only the fields that
/// changed are sent. An empty `agentId` or `goalId` clears it.
struct UpdateAutomationLoopRequest: Encodable, Sendable {
    var name: String? = nil
    var description: String? = nil
    var agentId: String? = nil
    var goalId: String? = nil
    var scheduleType: String? = nil
    var scheduleExpression: String? = nil
    var config: [String: JSONValue]? = nil

    enum CodingKeys: String, CodingKey {
        case name, description, config
        case agentId = "agent_id"
        case goalId = "goal_id"
        case scheduleType = "schedule_type"
        case scheduleExpression = "schedule_expression"
    }
}

private struct UpdateLoopScheduleRequest: Encodable, Sendable {
    let scheduleExpression: String

    enum CodingKeys: String, CodingKey {
        case scheduleExpression = "schedule_expression"
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

/// A row of `GET …/loops/:id/runs` (the routine run shape).
struct AutomationRunRecord: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let status: String
    let scheduledAt: String
    let startedAt: String?
    let finishedAt: String?
    let durationMs: Double?
    let output: String?
    let error: String?
    let triggeredBy: String?

    enum CodingKeys: String, CodingKey {
        case id, status, output, error
        case scheduledAt = "scheduled_at"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
        case durationMs = "duration_ms"
        case triggeredBy = "triggered_by"
    }
}

private struct RunRecordsEnvelope: Decodable, Sendable {
    let runs: [AutomationRunRecord]?
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

    func updateRoutine(id: String, _ request: UpdateAutomationRoutineRequest) async throws {
        try await client.put(path: "automation/routines/\(Self.escape(id))", body: request)
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

    func listLoops(recentRuns: Int = 14) async throws -> [AutomationLoop] {
        try await client.get(path: "automation/loops?recent_runs=\(recentRuns)")
    }

    func cancelLoopRun(loopId: String, runId: String) async throws {
        try await client.post(path: "automation/loops/\(Self.escape(loopId))/runs/\(Self.escape(runId))/cancel")
    }

    func listLoopRuns(id: String) async throws -> [AutomationRunRecord] {
        let envelope: RunRecordsEnvelope = try await client.get(path: "automation/loops/\(Self.escape(id))/runs")
        return envelope.runs ?? []
    }

    func setLoopEnabled(id: String, enabled: Bool) async throws {
        try await client.put(path: "automation/loops/\(Self.escape(id))",
                             body: UpdateRoutineStatusRequest(status: enabled ? "active" : "paused"))
    }

    /// A dragged loop's new cron (`updateLoop`, same route as the web).
    func updateLoopSchedule(id: String, cron: String) async throws {
        try await client.put(path: "automation/loops/\(Self.escape(id))",
                             body: UpdateLoopScheduleRequest(scheduleExpression: cron))
    }

    func updateLoop(id: String, _ request: UpdateAutomationLoopRequest) async throws {
        try await client.put(path: "automation/loops/\(Self.escape(id))", body: request)
    }

    func createLoop(_ request: CreateAutomationLoopRequest) async throws {
        try await client.post(path: "automation/loops", body: request)
    }

    func runLoop(id: String) async throws {
        try await client.post(path: "automation/loops/\(Self.escape(id))/run")
    }

    func deleteLoop(id: String) async throws {
        try await client.delete(path: "automation/loops/\(Self.escape(id))")
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
