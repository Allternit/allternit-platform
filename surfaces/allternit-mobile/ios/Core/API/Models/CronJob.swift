import Foundation

// -----------------------------------------------------------------------------
// Cron job REST models — base path `v1/cron` on gizzi-code's own server
// (`cmd/gizzi-code/src/runtime/server/server.ts:463`), same host as
// `PtyClient`/`PermissionClient`, NOT `allternit-api`.
//
// Mirrors the TS producers in
// `cmd/gizzi-code/src/runtime/automation/cron/types.ts:43-191`. Unlike the
// Rust-backed models (CoworkProject, AgentSession) this backend is a plain
// TS/Hono server that serializes its objects as-is, so the wire is camelCase
// throughout — no snake_case `CodingKeys` needed.
//
// Timestamps stay Strings (ISO-8601 `Date.toISOString()`); views parse them
// with `ISO8601FormatStyle` at render time (CodeModeView's convention),
// never via a global `Decodable` date strategy.
// -----------------------------------------------------------------------------

// MARK: - Schedule

/// `Schedule` union (types.ts:43-55): a cron expression or a fixed interval.
/// Phase 1 only ever displays these — creation always submits a plain string
/// (see `CreateJobInput.schedule`), which the server parses server-side.
enum CronJobSchedule: Decodable, Sendable {
    case cron(expression: String, timezone: String?)
    case interval(seconds: Int, startAt: String?)
    /// A schedule shape iOS doesn't recognize — rendered as "Unknown schedule"
    /// rather than failing to decode the whole job.
    case unknown

    private enum CodingKeys: String, CodingKey {
        case type, expression, timezone, seconds, startAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decodeIfPresent(String.self, forKey: .type)
        switch type {
        case "cron":
            let expression = try container.decode(String.self, forKey: .expression)
            let timezone = try container.decodeIfPresent(String.self, forKey: .timezone)
            self = .cron(expression: expression, timezone: timezone)
        case "interval":
            let seconds = try container.decode(Int.self, forKey: .seconds)
            let startAt = try container.decodeIfPresent(String.self, forKey: .startAt)
            self = .interval(seconds: seconds, startAt: startAt)
        default:
            self = .unknown
        }
    }

    /// Display text for list/detail rows: the raw cron expression, or
    /// "every N seconds" for interval schedules.
    var displayText: String {
        switch self {
        case .cron(let expression, let timezone):
            if let timezone {
                return "\(expression) (\(timezone))"
            }
            return expression
        case .interval(let seconds, _):
            return "every \(seconds)s"
        case .unknown:
            return "Unknown schedule"
        }
    }
}

// MARK: - Job record

/// `config.prompt` etc. for `type: "agent"` jobs (types.ts:118-128) — the
/// only job type Phase 1 fully models. Other types decode with `config`
/// left nil; the UI falls back to "type: shell" style labels for them.
struct CronAgentConfig: Decodable, Sendable {
    let prompt: String
    let agentId: String?
    let model: String?
    let context: String?
    let maxTokens: Int?
    let temperature: Double?
    /// Bot Mode: delivers into this bot's own chat (`[bot:<name>] <label>` jobs).
    let bot: String?
}

/// A cron job (`BaseJob`, types.ts:61-94, plus `AgentJob.config` when
/// `type == "agent"`). Other `type`s (`shell`/`http`/`cowork`/`function`)
/// decode every `BaseJob` field with `agentConfig` left nil rather than
/// failing — list/detail must render any job type.
struct CronJob: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let name: String
    let description: String?
    let type: String
    let status: String
    let schedule: CronJobSchedule
    let createdAt: String
    let updatedAt: String
    let lastRunAt: String?
    let nextRunAt: String?
    let runCount: Int
    let failCount: Int
    let tags: [String]
    /// Present only when `type == "agent"`.
    let agentConfig: CronAgentConfig?

    private enum CodingKeys: String, CodingKey {
        case id, name, description, type, status, schedule
        case createdAt, updatedAt, lastRunAt, nextRunAt
        case runCount, failCount, tags, config
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        description = try container.decodeIfPresent(String.self, forKey: .description)
        type = try container.decode(String.self, forKey: .type)
        status = try container.decode(String.self, forKey: .status)
        schedule = try container.decode(CronJobSchedule.self, forKey: .schedule)
        createdAt = try container.decode(String.self, forKey: .createdAt)
        updatedAt = try container.decode(String.self, forKey: .updatedAt)
        lastRunAt = try container.decodeIfPresent(String.self, forKey: .lastRunAt)
        nextRunAt = try container.decodeIfPresent(String.self, forKey: .nextRunAt)
        runCount = try container.decodeIfPresent(Int.self, forKey: .runCount) ?? 0
        failCount = try container.decodeIfPresent(Int.self, forKey: .failCount) ?? 0
        tags = try container.decodeIfPresent([String].self, forKey: .tags) ?? []
        agentConfig = type == "agent"
            ? try container.decodeIfPresent(CronAgentConfig.self, forKey: .config)
            : nil
    }

    static func == (lhs: CronJob, rhs: CronJob) -> Bool { lhs.id == rhs.id }
    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

/// Body of `POST v1/cron/jobs` for an agent-type job (`CreateJobInput`,
/// types.ts:197-214, narrowed to what `CreateAutomationTaskSheet` collects).
/// `schedule` is sent as a plain string — the server's `parseScheduleToType`
/// (cron/service.ts:220-222) accepts cron expressions and natural language
/// alike, so no client-side parsing/validation happens here.
struct CreateAgentJobRequest: Encodable, Sendable {
    struct Config: Encodable, Sendable {
        let prompt: String
        /// The agent that runs it (`AgentJob.config.agentId`); nil is the default model.
        var agentId: String? = nil
        /// "provider/model" (`AgentJob.config.model`); nil is the default model.
        var model: String? = nil
    }
    let name: String
    let type: String
    let schedule: String
    let config: Config
    var description: String? = nil
    /// "paused" to create it switched off; the server defaults to "active".
    var status: String? = nil
    var maxRetries: Int? = nil
    var timeoutSeconds: Int? = nil
}

/// Body of `PUT v1/cron/jobs/:id` (`UpdateJobInput`, types.ts): only the fields
/// that changed. The server merges `config` over the job's own, so a
/// `.null` value is how `agentId` is cleared.
struct UpdateCronJobRequest: Encodable, Sendable {
    var name: String? = nil
    var description: String? = nil
    var schedule: String? = nil
    var config: [String: JSONValue]? = nil
}

// MARK: - Run record

/// One run of a job (`CronRun`, types.ts:161-191, narrowed to the fields the
/// run-history list renders).
struct CronRun: Decodable, Sendable, Identifiable {
    let id: String
    let jobId: String
    let status: String
    let scheduledAt: String
    let startedAt: String?
    let finishedAt: String?
    let durationMs: Int?
    let output: String?
    let error: String?
    let triggeredBy: String
}
