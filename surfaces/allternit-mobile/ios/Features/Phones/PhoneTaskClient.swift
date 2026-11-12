import Foundation
import UIKit

/// Phone task routes on the owner's computer, through the relay
/// (surfaces/docs/api/phone-holds.mdx):
///
/// - `POST /api/v1/phones/:serial/task` `{goal, background, thread_id?, session_id?, bot_id?, source}`
///   → 202 `{run_id}`, or 403 `{status:"refused", reason, message}`
/// - `GET  /api/v1/phones/skills` and `POST /api/v1/phones/skills/:name/run` (saved phone routines)
/// - `GET  /api/v1/phones/history` → `{runs:[…]}` newest first; a live row adds `status`,
///   `step`, `step_label`, `trail` and, while a hold waits, `approvalId`
/// - `GET  /api/v1/phones/runs/:runId/thumb` → JPEG (404/401 = no thumbnail, never an error)
/// - `GET  /api/v1/phones/holds/:id` and `/crop` → the hold and its review image
///
/// Linked phones come from the cloud (`ACIComputersClient.listPhones`).
final class PhoneTaskClient: @unchecked Sendable {
    static let shared = PhoneTaskClient()

    enum Failure: LocalizedError, Equatable {
        case signedOut
        case noComputer
        case offline
        case quota
        case failed(String)

        var errorDescription: String? {
            switch self {
            case .signedOut: return "Sign in to run tasks on your phone."
            case .noComputer: return "Pair a computer first. Phone tasks run through Allternit Desktop."
            case .offline: return "Your computer didn't answer. Try again in a moment."
            case .quota: return "Your phone minutes are used up this month."
            case .failed(let why): return why
            }
        }
    }

    enum StartResult: Equatable, Sendable {
        case started(runId: String)
        case refused(String)
    }

    struct Routine: Equatable, Sendable, Identifiable {
        let name: String
        let description: String?
        var id: String { name }
    }

    struct RunSummary: Equatable, Sendable {
        let runId: String
        let serial: String
        let goal: String
        /// `running` | `waiting_approval` while live; empty once the run ended.
        let status: String
        /// Live only: the current step, its label and the finished steps so far.
        var step: Int = 0
        var stepLabel: String? = nil
        var trail: [RunStep] = []
        /// Live only, while a hold waits: the inbox item is `approval:<approvalId>`.
        var approvalId: String? = nil
        /// Recorded steps, and how many failed.
        var stepCount: Int = 0
        var failed: Int = 0
    }

    private let api: APIClient

    init(api: APIClient = .shared) { self.api = api }

    // MARK: Phones

    func phones() async throws -> [ACIComputersClient.CloudPhone] {
        do { return try await ACIComputersClient.shared.listPhones() }
        catch ACIComputersClient.Failure.signedOut { throw Failure.signedOut }
        catch ACIComputersClient.Failure.auth { throw Failure.signedOut }
        catch is URLError { throw Failure.offline }
    }

    // MARK: Tasks

    func start(serial: String, goal: String, source: String, sessionId: String? = nil,
               threadId: String? = nil, botId: String? = nil) async throws -> StartResult {
        var body: [String: Any] = ["goal": goal, "background": true, "source": source]
        if let sessionId, !sessionId.isEmpty { body["session_id"] = sessionId }
        if let threadId, !threadId.isEmpty { body["thread_id"] = threadId }
        if let botId, !botId.isEmpty { body["bot_id"] = botId }
        let (data, status) = try await runtime("phones/\(Self.escape(serial))/task", method: "POST", body: body)
        return try Self.startResult(data: data, status: status)
    }

    func routines() async throws -> [Routine] {
        let (data, status) = try await runtime("phones/skills")
        try Self.check(status, data: data)
        let rows = (Self.object(data)?["skills"] as? [[String: Any]]) ?? []
        return rows.compactMap { row in
            guard let name = row["name"] as? String, !name.isEmpty else { return nil }
            return Routine(name: name, description: (row["description"] as? String).flatMap { $0.isEmpty ? nil : $0 })
        }
    }

    func runRoutine(name: String, serial: String) async throws -> StartResult {
        let body: [String: Any] = ["serial": serial, "background": true]
        let (data, status) = try await runtime("phones/skills/\(Self.escape(name))/run", method: "POST", body: body)
        return try Self.startResult(data: data, status: status)
    }

    func latestRun() async throws -> RunSummary? {
        let (data, status) = try await runtime("phones/history")
        try Self.check(status, data: data)
        let runs = (Self.object(data)?["runs"] as? [[String: Any]]) ?? []
        return runs.first.map(Self.summary)
    }

    func run(_ runId: String) async throws -> RunSummary? {
        let (data, status) = try await runtime("phones/history")
        try Self.check(status, data: data)
        let runs = (Self.object(data)?["runs"] as? [[String: Any]]) ?? []
        return runs.map(Self.summary).first { $0.runId == runId }
    }

    /// nil on 404/401 or any failure: the card then shows no thumbnail.
    func thumb(runId: String) async -> UIImage? {
        guard let (data, status) = try? await runtime("phones/runs/\(Self.escape(runId))/thumb", accept: "image/*"),
              status == 200 else { return nil }
        return UIImage(data: data)
    }

    // MARK: Holds

    func hold(id: String) async throws -> PhoneHoldState {
        let (data, status) = try await runtime("phones/holds/\(Self.escape(id))")
        if status == 404 { return .gone }
        try Self.check(status, data: data)
        return PhoneHoldState(raw: Self.object(data)?["state"] as? String)
    }

    /// The review image. nil when the hold has none (404) or it can't load.
    func crop(holdId: String) async -> UIImage? {
        guard let (data, status) = try? await runtime("phones/holds/\(Self.escape(holdId))/crop", accept: "image/*"),
              status == 200 else { return nil }
        return UIImage(data: data)
    }

    // MARK: Plumbing

    private func runtime(_ path: String, method: String = "GET", body: [String: Any]? = nil,
                         accept: String = "application/json") async throws -> (Data, Int) {
        var request: URLRequest
        do { request = try await api.authorizedRequest(path: path, method: method) }
        catch { throw Failure.signedOut }
        request.setValue(accept, forHTTPHeaderField: "Accept")
        request.timeoutInterval = 20
        if let body { request.httpBody = try JSONSerialization.data(withJSONObject: body) }
        do {
            let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: api.session)
            return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
        } catch RuntimeRelayError.noComputer {
            throw Failure.noComputer
        } catch is URLError {
            throw Failure.offline
        } catch RuntimeRelayError.unreachable {
            throw Failure.offline
        }
    }

    static func startResult(data: Data, status: Int) throws -> StartResult {
        let obj = object(data)
        if status == 403, (obj?["status"] as? String) == "refused" {
            return .refused((obj?["message"] as? String) ?? "Allternit didn't start this phone task.")
        }
        try check(status, data: data)
        guard let runId = (obj?["run_id"] as? String) ?? (obj?["runId"] as? String) else {
            throw Failure.failed("The phone task started without a run id.")
        }
        return .started(runId: runId)
    }

    static func check(_ status: Int, data: Data) throws {
        switch status {
        case 200..<300: return
        case 401, 403: throw Failure.signedOut
        case 402, 429: throw Failure.quota
        case 502, 504: throw Failure.offline
        default:
            let message = object(data)?["message"] as? String ?? object(data)?["error"] as? String
            throw Failure.failed(message ?? "The phone task failed (HTTP \(status)).")
        }
    }

    private static func summary(_ row: [String: Any]) -> RunSummary {
        func s(_ keys: String...) -> String {
            for k in keys { if let v = row[k] as? String, !v.isEmpty { return v } }
            return ""
        }
        var run = RunSummary(runId: s("run_id", "runId"), serial: s("serial"), goal: s("goal", "skill"), status: s("status"))
        run.step = (row["step"] as? NSNumber)?.intValue ?? 0
        run.stepLabel = (row["step_label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        run.trail = RunStep.list(any: row["trail"])
        run.approvalId = (row["approvalId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        run.stepCount = (row["steps"] as? NSNumber)?.intValue ?? 0
        run.failed = (row["failed"] as? NSNumber)?.intValue ?? 0
        return run
    }

    private static func object(_ data: Data) -> [String: Any]? {
        (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    private static func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(["/"])) ?? s
    }
}

/// A hold's state as `GET /phones/holds/:id` reports it.
enum PhoneHoldState: Equatable, Sendable {
    case pending, approved, denied, changed, expired, gone

    init(raw: String?) {
        switch raw {
        case "approved": self = .approved
        case "denied", "rejected": self = .denied
        case "changed": self = .changed
        case "expired": self = .expired
        default: self = .pending
        }
    }
}
