import Foundation

/// Why an eval call failed, in the words the screens need (CONTRACT section 6).
enum AgentEvalError: Error, Equatable, Sendable {
    /// No computer to run on (relay no-computer, or the compute gate).
    case noComputer
    /// The computer did not answer (relay unreachable, 502/503/504, no network).
    case unreachable
    /// The suite already holds 100 cases.
    case caseLimit
    /// A live run needs a fresh price first.
    case estimateRequired
    case notFound
    case unknown(String)

    /// Plain sentence for a sheet or page (CONTRACT section 9 copy).
    var message: String {
        switch self {
        case .noComputer: return "Needs a computer"
        case .unreachable: return "Couldn't reach your computer, try again"
        case .caseLimit: return "You have 100 test cases. Delete one to save another."
        case .estimateRequired: return "Get a price first."
        case .notFound: return "That isn't there anymore."
        case .unknown: return "Couldn't save, try again"
        }
    }

    /// Maps a transport error. A 409 is `case_limit` on a save and
    /// `estimate_required` on a run, so the caller passes which call it was.
    static func map(_ error: Error, conflict: AgentEvalError = .unknown("")) -> AgentEvalError {
        if let known = error as? AgentEvalError { return known }
        if let relay = error as? RuntimeRelayError {
            switch relay {
            case .noComputer: return .noComputer
            case .unreachable: return .unreachable
            case .socketUnavailable(let text): return .unknown(text)
            }
        }
        if let api = error as? APIError {
            if api.isComputeGateRefusal { return .noComputer }
            switch api {
            case .httpError(let status, let message):
                switch status {
                case 404: return .notFound
                case 409: return conflict
                case 428: return .noComputer
                case 502, 503, 504: return .unreachable
                default: return .unknown(message ?? "")
                }
            case .transport(let inner): return map(inner, conflict: conflict)
            case .decoding, .invalidResponse: return .unknown("")
            }
        }
        if error is URLError { return .unreachable }
        return .unknown(error.localizedDescription)
    }
}

// MARK: - Shapes (CONTRACT section 6, snake_case on the wire)

struct EvalCaseSummary: Codable, Equatable, Identifiable, Sendable {
    let id: String
    let suite: String?
    let title: String
    /// "good" | "bad"
    let label: String
    let savedAt: Double?
    let needsCheck: Bool
    /// "pass" | "fail" | nil
    let lastResult: String?
    let lastRunAt: Double?
    let sessionId: String?

    enum CodingKeys: String, CodingKey {
        case id, suite, title, label
        case savedAt = "saved_at"
        case needsCheck = "needs_check"
        case lastResult = "last_result"
        case lastRunAt = "last_run_at"
        case sessionId = "session_id"
    }
}

struct EvalSelfTest: Codable, Equatable, Sendable {
    let passed: Bool
    let agreesWithLabel: Bool
    enum CodingKeys: String, CodingKey {
        case passed
        case agreesWithLabel = "agrees_with_label"
    }
}

struct EvalSaveResult: Codable, Equatable, Sendable {
    let caseSummary: EvalCaseSummary
    let selfTest: EvalSelfTest?
    let needsCheck: Bool
    enum CodingKeys: String, CodingKey {
        case caseSummary = "case"
        case selfTest = "self_test"
        case needsCheck = "needs_check"
    }
}

struct EvalEstimate: Codable, Equatable, Sendable {
    let estimateId: String
    let cases: Int
    let k: Int
    let attempts: Int
    let tokensEst: Int
    let minutesEst: Int
    /// "plan" | "metered" | "unknown"
    let billing: String
    let usdEst: Double?

    enum CodingKeys: String, CodingKey {
        case estimateId = "estimate_id"
        case cases, k, attempts, billing
        case tokensEst = "tokens_est"
        case minutesEst = "minutes_est"
        case usdEst = "usd_est"
    }
}

struct EvalCaseResult: Codable, Equatable, Identifiable, Sendable {
    let id: String
    let passes: Int?
    let attempts: Int?
    let pass: Bool
}

/// The report, read loosely: the pass rate and per-case rows are looked up
/// under `suites.<name>` (and its `extra`) or the report's own `extra`.
struct EvalReport: Decodable, Equatable, Sendable {
    let passRate: Double?
    let failedCases: [String]
    let perCase: [EvalCaseResult]

    private struct SuiteBody: Decodable {
        let passRate: Double?
        let failedCases: [String]?
        let perCase: [EvalCaseResult]?
        let extra: Extra?
        struct Extra: Decodable {
            let perCase: [EvalCaseResult]?
            enum CodingKeys: String, CodingKey { case perCase = "per_case" }
        }
        enum CodingKeys: String, CodingKey {
            case passRate = "pass_rate"
            case failedCases = "failed_cases"
            case perCase = "per_case"
            case extra
        }
    }
    private enum CodingKeys: String, CodingKey { case suites, extra }

    init(passRate: Double?, failedCases: [String], perCase: [EvalCaseResult]) {
        self.passRate = passRate
        self.failedCases = failedCases
        self.perCase = perCase
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let suites = (try? c.decode([String: SuiteBody].self, forKey: .suites)) ?? [:]
        let first = suites.keys.sorted().first.flatMap { suites[$0] }
        let top = try? c.decode(SuiteBody.Extra.self, forKey: .extra)
        passRate = first?.passRate
        failedCases = first?.failedCases ?? []
        perCase = first?.perCase ?? first?.extra?.perCase ?? top?.perCase ?? []
    }
}

struct EvalRun: Decodable, Equatable, Sendable {
    let runId: String
    /// "queued" | "running" | "done" | "failed" | "cancelled"
    let state: String
    let done: Int
    let total: Int
    let report: EvalReport?
    let error: String?

    private struct Progress: Decodable { let done: Int; let total: Int }
    enum CodingKeys: String, CodingKey { case runId = "run_id", state, progress, report, error }

    init(runId: String, state: String, done: Int, total: Int, report: EvalReport?, error: String?) {
        self.runId = runId; self.state = state; self.done = done; self.total = total
        self.report = report; self.error = error
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        runId = try c.decode(String.self, forKey: .runId)
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? "done"
        let p = try c.decodeIfPresent(Progress.self, forKey: .progress)
        done = p?.done ?? 0
        total = p?.total ?? 0
        report = try c.decodeIfPresent(EvalReport.self, forKey: .report)
        error = try c.decodeIfPresent(String.self, forKey: .error)
    }

    var isFinished: Bool { ["done", "failed", "cancelled"].contains(state) }
}

/// What a replay or live start returns.
struct EvalRunStarted: Decodable, Equatable, Sendable {
    let runId: String
    let report: EvalReport?
    enum CodingKeys: String, CodingKey { case runId = "run_id", report }
}

/// Test cases on the person's computer: the `/api/v1/agent-evals` proxy
/// (allternit-api agent_eval_routes.rs). Same route and relay as
/// `AgentChatClient`, because both go through `APIClient`.
final class AgentEvalClient: @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) { self.client = client }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    func saveCase(sessionId: String, label: String, note: String?,
                  quickCheck: [String]) async throws -> EvalSaveResult {
        struct Body: Encodable {
            let sessionId: String
            let label: String
            let note: String?
            let quickCheck: [String]?
            enum CodingKeys: String, CodingKey {
                case sessionId = "session_id", label, note, quickCheck = "quick_check"
            }
        }
        let trimmed = note?.trimmingCharacters(in: .whitespacesAndNewlines)
        let body = Body(sessionId: sessionId, label: label,
                        note: (trimmed?.isEmpty ?? true) ? nil : trimmed,
                        quickCheck: quickCheck.isEmpty ? nil : quickCheck)
        do {
            return try await client.post(path: "agent-evals/cases", body: body)
        } catch { throw AgentEvalError.map(error, conflict: .caseLimit) }
    }

    func listCases(sessionId: String? = nil) async throws -> [EvalCaseSummary] {
        struct Response: Decodable { let cases: [EvalCaseSummary] }
        var path = "agent-evals/cases"
        if let sessionId { path += "?session_id=\(Self.escape(sessionId))" }
        do {
            let response: Response = try await client.get(path: path)
            return response.cases
        } catch { throw AgentEvalError.map(error) }
    }

    func deleteCase(id: String) async throws {
        do { try await client.delete(path: "agent-evals/cases/\(Self.escape(id))") }
        catch { throw AgentEvalError.map(error) }
    }

    func estimateRun(suite: String = "my-cases", caseIds: [String]? = nil, k: Int? = nil) async throws -> EvalEstimate {
        struct Body: Encodable {
            let suite: String
            let caseIds: [String]?
            let k: Int?
            enum CodingKeys: String, CodingKey { case suite, caseIds = "case_ids", k }
        }
        do {
            return try await client.post(path: "agent-evals/runs/estimate",
                                         body: Body(suite: suite, caseIds: caseIds, k: k))
        } catch { throw AgentEvalError.map(error) }
    }

    /// Replay returns the report at once. Live needs `estimateId` and returns
    /// a run id to poll with `getRun`.
    func startRun(suite: String = "my-cases", mode: String, estimateId: String? = nil,
                  caseIds: [String]? = nil, k: Int? = nil) async throws -> EvalRunStarted {
        struct Body: Encodable {
            let suite: String
            let mode: String
            let estimateId: String?
            let caseIds: [String]?
            let k: Int?
            enum CodingKeys: String, CodingKey {
                case suite, mode, k
                case estimateId = "estimate_id"
                case caseIds = "case_ids"
            }
        }
        do {
            return try await client.post(path: "agent-evals/runs",
                                         body: Body(suite: suite, mode: mode, estimateId: estimateId,
                                                    caseIds: caseIds, k: k))
        } catch { throw AgentEvalError.map(error, conflict: .estimateRequired) }
    }

    func getRun(id: String) async throws -> EvalRun {
        do { return try await client.get(path: "agent-evals/runs/\(Self.escape(id))") }
        catch { throw AgentEvalError.map(error) }
    }

    func cancelRun(id: String) async throws {
        do { try await client.post(path: "agent-evals/runs/\(Self.escape(id))/cancel") }
        catch { throw AgentEvalError.map(error) }
    }
}
