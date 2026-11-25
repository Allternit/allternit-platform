import Foundation

// The in-app island bar's calls (Island bar + Team map, lane 32). Contract:
// handoffs/island-bar-team-map-2026-11-25/CONTRACT.md sections 1-5.
//
// Directory, review and cancel reuse the "Do with Allternit" services
// (`IntentServices: BarIntentServicing`, Core/BarIntentServices.swift), so the
// bar, Siri and Shortcuts read the same wording. The bar adds what Siri does
// not need: the sentence as parts, recent values per blank, and the job's
// live events (`GET /bar/jobs/:id/events`, SSE wire `{type, session_id, part}`).

/// One part of the intent sentence (`Part`).
enum IslandSentencePart: Equatable, Sendable {
    case text(String)
    case blank(String)
}

/// A recent value for a blank (`Blank.recent`).
struct IslandRecent: Equatable, Sendable, Hashable {
    var value: BarValue
    var label: String
}

/// `POST /bar/intent`, with the sentence as parts and recents per blank.
struct IslandIntent: Equatable, Sendable {
    var intentId: String
    var app: BarAppInfo
    var toolName: String
    var toolTitle: String
    var parts: [IslandSentencePart]
    var blanks: [BarBlankInfo]
    var recents: [String: [IslandRecent]]
}

/// A job's state as the bar needs it (Sendable, unlike the raw object).
struct IslandJobUpdate: Sendable {
    var state: String
    /// Completed: "Refunded €320" (past tense, CONTRACT section 5).
    var receipt: String?
    var error: String?
    /// Ready for `syncBarJob`; nil when the object had no id.
    var snapshot: BotLiveActivityManager.BarJobSnapshot?

    init?(_ job: [String: Any], approverName: String?) {
        guard let state = job["state"] as? String, !state.isEmpty else { return nil }
        self.state = state
        snapshot = BarWire.snapshot(job, approverName: approverName)
        receipt = state == "completed" ? BarWire.receipt(job) : nil
        error = snapshot?.error
    }
}

/// One live event for a job, decoded from the SSE wire form.
enum IslandJobEvent: Sendable {
    /// `{type: bar_job, job}`: the job's new state.
    case job(IslandJobUpdate)
    /// `{type: mcp_task, part}`: a task part (`statusMessage` is the progress line).
    case task(status: String, message: String?)
    /// `{type: mcp_elicitation}`: the tool asks for more.
    case needsInput
    /// `{type: mcp_elicitation_resolved}`.
    case inputResolved
}

struct IslandBarClient: Sendable {
    private var api: APIClient { .shared }

    /// The app's registered services (`IntentServiceRegistry.register(IntentServices())` at launch).
    private func services() throws -> any BarIntentServicing {
        guard let svc = IntentServiceRegistry.current as? any BarIntentServicing else { throw IntentFailure.appRequired }
        return svc
    }

    func apps() async throws -> [BarAppInfo] {
        try await services().listBarApps(query: nil)
    }

    func tools(_ app: BarAppInfo) async throws -> [BarToolInfo] {
        try await services().listBarTools(appKind: app.kind, appRef: app.ref)
    }

    func intent(app: BarAppInfo, tool: String, botId: String?) async throws -> IslandIntent {
        var body: [String: Any] = ["app": ["kind": app.kind, "ref": app.ref], "tool": tool]
        if let botId, !botId.isEmpty { body["bot_id"] = botId }
        let json = try await BarWire.call(api, "POST", "bar/intent", body: body)
        guard let intentId = json["intent_id"] as? String else { throw IntentFailure.failed(CalmCopy.fallback) }
        let rows = json["blanks"] as? [[String: Any]] ?? []
        let blanks = rows.compactMap(BarWire.blank)
        var recents: [String: [IslandRecent]] = [:]
        for row in rows {
            guard let key = row["key"] as? String else { continue }
            recents[key] = (row["recent"] as? [[String: Any]] ?? []).compactMap { r in
                guard let value = Self.value(r["value"]) else { return nil }
                return IslandRecent(value: value, label: (r["label"] as? String) ?? value.display)
            }
        }
        var parts: [IslandSentencePart] = (json["sentence"] as? [[String: Any]] ?? []).compactMap { p in
            if p["type"] as? String == "blank", let key = p["key"] as? String { return .blank(key) }
            if let text = p["text"] as? String { return .text(text) }
            return nil
        }
        // A sentence with no blank parts still shows every blank.
        let inSentence = Set(parts.compactMap { if case .blank(let k) = $0 { return k } else { return nil } })
        for blank in blanks where !inSentence.contains(blank.key) {
            parts.append(.text(" "))
            parts.append(.blank(blank.key))
        }
        let toolObj = json["tool"] as? [String: Any]
        let appInfo = (json["app"] as? [String: Any]).flatMap(BarWire.appRef).map { a -> BarAppInfo in
            var a = a
            a.level = app.level
            a.toolCount = app.toolCount
            if a.name == a.ref { a.name = app.name }
            return a
        } ?? app
        return IslandIntent(intentId: intentId, app: appInfo,
                            toolName: toolObj?["name"] as? String ?? tool,
                            toolTitle: toolObj?["title"] as? String ?? tool,
                            parts: parts, blanks: blanks, recents: recents)
    }

    func review(_ intent: IslandIntent, values: [String: BarValue], botId: String?) async throws -> BarReviewInfo {
        try await services().barReview(intentId: intent.intentId, values: values, botId: botId)
    }

    /// Starts the job, or throws `BarWire.Gate` when a rule sends it to the
    /// approver (403 `owner_approval_required`). Syncs the Live Activity.
    func run(jobId: String, approverName: String?) async throws -> IslandJobUpdate? {
        let job = try await BarWire.call(api, "POST", "bar/jobs/\(BarWire.escape(jobId))/run",
                                         noReplyIsNoComputer: true)["job"] as? [String: Any] ?? [:]
        return await Self.synced(IslandJobUpdate(job, approverName: approverName))
    }

    /// `GET /bar/jobs/:id` (the fallback when the event stream drops). Syncs the Live Activity.
    func job(_ jobId: String, approverName: String?) async throws -> IslandJobUpdate? {
        let job = try await BarWire.call(api, "GET", "bar/jobs/\(BarWire.escape(jobId))")["job"] as? [String: Any] ?? [:]
        return await Self.synced(IslandJobUpdate(job, approverName: approverName))
    }

    private static func synced(_ update: IslandJobUpdate?) async -> IslandJobUpdate? {
        if let snap = update?.snapshot {
            await MainActor.run { BotLiveActivityManager.shared.syncBarJob(snap) }
        }
        return update
    }

    func cancel(jobId: String) async {
        await (try? services())?.barCancel(jobId: jobId)
    }

    /// The job's live events. Each `bar_job` event is also sent to the Live
    /// Activity (`syncBarJob`) here, so every caller keeps it current.
    /// Ends when the server closes the stream.
    func events(jobId: String, approverName: String?) -> AsyncThrowingStream<IslandJobEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = try await api.authorizedRequest(path: "bar/jobs/\(BarWire.escape(jobId))/events")
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 300
                    let (bytes, response) = try await api.sendStream(request)
                    guard let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) else {
                        throw IntentFailure.failed(CalmCopy.cantReach)
                    }
                    for try await line in bytes.lines {
                        try Task.checkCancellation()
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard let data = payload.data(using: .utf8),
                              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                              let event = Self.decode(obj, approverName: approverName) else { continue }
                        if case .job(let update) = event, let snap = update.snapshot {
                            await MainActor.run { BotLiveActivityManager.shared.syncBarJob(snap) }
                        }
                        continuation.yield(event)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// One SSE `data:` object, wire form (`type`, `session_id`, `part`).
    /// The parsed web form (`kind`, `sessionId`) is read too.
    static func decode(_ obj: [String: Any], approverName: String?) -> IslandJobEvent? {
        let type = (obj["type"] as? String) ?? (obj["kind"] as? String) ?? ""
        switch type {
        case "bar_job":
            guard let job = obj["job"] as? [String: Any],
                  let update = IslandJobUpdate(job, approverName: approverName) else { return nil }
            return .job(update)
        case "mcp_task":
            let part = obj["part"] as? [String: Any] ?? [:]
            return .task(status: part["status"] as? String ?? "working",
                         message: (part["statusMessage"] as? String).flatMap { $0.isEmpty ? nil : $0 })
        case "mcp_elicitation": return .needsInput
        case "mcp_elicitation_resolved": return .inputResolved
        default: return nil
        }
    }

    static func value(_ any: Any?) -> BarValue? {
        switch any {
        case let s as String: return .text(s)
        case let n as NSNumber:
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return .bool(n.boolValue) }
            return .number(n.doubleValue)
        default: return nil
        }
    }
}
