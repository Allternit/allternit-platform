import Foundation

/// The bar calls behind "Do with Allternit" (SystemSurfaces/Intents/DoIntents.swift),
/// over `APIClient` (base URL + Bearer, relay route included). Contract:
/// handoffs/island-bar-team-map-2026-11-25/CONTRACT.md sections 1-5.
///
/// Errors are read from the raw body (bar routes send `{error: message,
/// code}`; the gate sends `data.approver`), never shown as a raw code.
extension IntentServices: BarIntentServicing {
    private var api: APIClient { .shared }

    // MARK: Directory

    func listBarApps(query: String?) async throws -> [BarAppInfo] {
        let q = query?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let path = q.isEmpty ? "bar/apps" : "bar/apps?q=\(BarWire.escapeQuery(q))"
        let json = try await BarWire.call(api, "GET", path)
        let apps = (json["apps"] as? [[String: Any]] ?? []).compactMap(BarWire.app)
        // The server may ignore `q` (CONTRACT: no query on /bar/apps); filter here too.
        let n = q.lowercased()
        return n.isEmpty ? apps : apps.filter { $0.name.lowercased().contains(n) || $0.ref.lowercased().contains(n) }
    }

    func listBarTools(appKind: String, appRef: String) async throws -> [BarToolInfo] {
        let json = try await BarWire.call(api, "GET",
                                          "bar/apps/\(BarWire.escape(appKind))/\(BarWire.escape(appRef))/tools")
        return (json["tools"] as? [[String: Any]] ?? []).compactMap { t in
            guard let name = t["name"] as? String, !name.isEmpty else { return nil }
            return BarToolInfo(name: name, title: (t["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? name,
                               hasUI: t["has_ui"] as? Bool ?? false, readOnly: t["read_only"] as? Bool ?? false)
        }
    }

    // MARK: Intent -> review -> run

    func barIntent(appKind: String, appRef: String, tool: String, botId: String?) async throws -> BarIntentInfo {
        var body: [String: Any] = ["app": ["kind": appKind, "ref": appRef], "tool": tool]
        if let botId, !botId.isEmpty { body["bot_id"] = botId }
        let json = try await BarWire.call(api, "POST", "bar/intent", body: body)
        guard let intentId = json["intent_id"] as? String else { throw IntentFailure.failed(CalmCopy.fallback) }
        let app = (json["app"] as? [String: Any]).flatMap(BarWire.appRef)
            ?? BarAppInfo(kind: appKind, ref: appRef, name: appRef, level: "", toolCount: 0)
        let toolObj = json["tool"] as? [String: Any]
        let blanks = (json["blanks"] as? [[String: Any]] ?? []).compactMap(BarWire.blank)
        let labels = Dictionary(blanks.map { ($0.key, $0.label) }, uniquingKeysWith: { a, _ in a })
        let sentence = (json["sentence"] as? [[String: Any]] ?? []).map { part -> String in
            if part["type"] as? String == "blank", let key = part["key"] as? String { return "[\(labels[key] ?? key)]" }
            return part["text"] as? String ?? ""
        }.joined()
        return BarIntentInfo(intentId: intentId, app: app,
                             toolName: toolObj?["name"] as? String ?? tool,
                             toolTitle: toolObj?["title"] as? String ?? tool,
                             sentence: sentence, blanks: blanks)
    }

    func barReview(intentId: String, values: [String: BarValue], botId: String?) async throws -> BarReviewInfo {
        var body: [String: Any] = ["intent_id": intentId, "args": values.mapValues(BarWire.json)]
        if let botId, !botId.isEmpty { body["bot_id"] = botId }
        let json = try await BarWire.call(api, "POST", "bar/review", body: body)
        guard let jobId = json["job_id"] as? String else { throw IntentFailure.failed(CalmCopy.fallback) }
        let rule = json["rule"] as? [String: Any]
        let approver = json["approver"] as? [String: Any]
        let lines = (json["lines"] as? [[String: Any]] ?? []).map {
            BarReviewInfo.Line(label: $0["label"] as? String ?? $0["key"] as? String ?? "",
                               value: BarWire.text($0["value"]))
        }
        return BarReviewInfo(jobId: jobId, level: json["level"] as? String ?? "",
                             needsConfirmation: json["needs_confirmation"] as? Bool ?? true,
                             ruleText: rule?["text"] as? String,
                             approverName: approver?["name"] as? String,
                             approverIsYou: approver?["is_you"] as? Bool ?? false,
                             lines: lines)
    }

    func barRun(jobId: String, wait: TimeInterval) async throws -> BarRunOutcome {
        let id = BarWire.escape(jobId)
        var job: [String: Any]
        do {
            job = try await BarWire.call(api, "POST", "bar/jobs/\(id)/run", noReplyIsNoComputer: true)["job"]
                as? [String: Any] ?? [:]
        } catch let gate as BarWire.Gate {
            return .waitingApproval(approverName: gate.approverName)
        }
        let deadline = Date().addingTimeInterval(wait)
        while true {
            // TODO(#2996): once BotLiveActivityManager.syncBarJob is on main, call it
            // here with `job` so the Live Activity follows the run (lane 33).
            if let outcome = BarWire.outcome(job) { return outcome }
            guard Date() < deadline else { return .working }
            try? await Task.sleep(for: .milliseconds(1500))
            if let next = try? await BarWire.call(api, "GET", "bar/jobs/\(id)")["job"] as? [String: Any] {
                job = next
            }
        }
    }

    func barCancel(jobId: String) async {
        _ = try? await BarWire.call(api, "POST", "bar/jobs/\(BarWire.escape(jobId))/cancel")
    }
}

/// Wire helpers for the bar routes (JSON objects; nullable fields are `NSNull`).
enum BarWire {
    /// `403 owner_approval_required`: the run waits on someone.
    struct Gate: Error { var approverName: String? }

    static func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/?#"))) ?? s
    }

    static func escapeQuery(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=+#"))) ?? s
    }

    /// One call; throws `IntentFailure` (or `Gate`) with plain wording.
    /// `noReplyIsNoComputer`: no network reply counts as no computer
    /// (CONTRACT section 3, used for run only).
    static func call(_ api: APIClient, _ method: String, _ path: String,
                     body: [String: Any]? = nil, noReplyIsNoComputer: Bool = false) async throws -> [String: Any] {
        var request = try await api.authorizedRequest(path: path, method: method)
        if let body { request.httpBody = try JSONSerialization.data(withJSONObject: body) }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await api.send(request)
        } catch RuntimeRelayError.noComputer {
            throw IntentFailure.noComputer
        } catch RuntimeRelayError.unreachable {
            throw noReplyIsNoComputer ? IntentFailure.noComputer : IntentFailure.offline
        } catch is URLError {
            throw noReplyIsNoComputer ? IntentFailure.noComputer : IntentFailure.offline
        } catch APIError.httpError(let status, let message) {
            // `.unavailable` route (cloud picked, no paired computer) answers 503 here.
            throw status == 503 ? IntentFailure.noComputer : IntentFailure.failed(message ?? CalmCopy.fallback)
        }
        guard let http = response as? HTTPURLResponse else { throw IntentFailure.failed(CalmCopy.fallback) }
        guard (200...299).contains(http.statusCode) else {
            throw failure(status: http.statusCode, data: data)
        }
        return (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    static func failure(status: Int, data: Data) -> Error {
        let parsed = APIErrorBody.parse(data)
        let code = parsed.code?.lowercased() ?? ""
        let raw = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        switch (status, code) {
        case (_, "owner_approval_required"):
            let dataObj = (raw?["data"] as? [String: Any]) ?? ((raw?["error"] as? [String: Any])?["data"] as? [String: Any])
            let approver = (dataObj?["approver"] ?? raw?["approver"]) as? [String: Any]
            return Gate(approverName: approver?["name"] as? String)
        case (409, "no_runtime"), (428, _), (_, "compute_required"), (_, "free_computers_retired"):
            return IntentFailure.noComputer
        case (401, _):
            return IntentFailure.signedOut
        case (_, "no_grant"):
            return IntentFailure.failed("This bot has no access to that app. Ask the owner to grant access.")
        case (_, "level_read_only"):
            return IntentFailure.failed("This app is at Browser level here, so it can only read.")
        case (_, "approver_required"):
            return IntentFailure.failed(parsed.calmMessage ?? "A team rule needs an approver, and none is named.")
        case (_, "team_rules_unavailable"), (_, "tier_check_unavailable"):
            return IntentFailure.failed("Can't check the team rules right now. Try again in a moment.")
        case (_, "app_kind_unsupported"):
            return IntentFailure.failed("That app can't run tools from here yet.")
        case (404, _):
            return IntentFailure.notFound("that app or tool")
        default:
            // `missing_fields` carries "Fill in X first." as its message.
            return IntentFailure.failed(parsed.message.flatMap { $0.isEmpty ? nil : $0 }
                                        ?? parsed.calmMessage ?? CalmCopy.fallback)
        }
    }

    static func appRef(_ a: [String: Any]) -> BarAppInfo? {
        guard let kind = a["kind"] as? String, let ref = a["ref"] as? String else { return nil }
        return BarAppInfo(kind: kind, ref: ref, name: (a["name"] as? String) ?? ref, level: "", toolCount: 0)
    }

    static func app(_ row: [String: Any]) -> BarAppInfo? {
        guard var app = (row["app"] as? [String: Any]).flatMap(appRef) else { return nil }
        app.level = row["level"] as? String ?? ""
        app.toolCount = row["tool_count"] as? Int ?? 0
        return app
    }

    static func blank(_ b: [String: Any]) -> BarBlankInfo? {
        guard let key = b["key"] as? String else { return nil }
        let options = (b["options"] as? [[String: Any]] ?? []).map {
            BarBlankInfo.Option(value: text($0["value"]), label: ($0["label"] as? String) ?? text($0["value"]))
        }
        return BarBlankInfo(key: key, label: b["label"] as? String ?? key, type: b["type"] as? String ?? "text",
                            required: b["required"] as? Bool ?? false, unit: b["unit"] as? String,
                            options: options,
                            pickerKind: (b["picker"] as? [String: Any])?["kind"] as? String ?? "standard")
    }

    static func json(_ v: BarValue) -> Any {
        switch v {
        case .text(let s): return s
        case .number(let n): return n.rounded() == n && abs(n) < 1e15 ? Int64(n) as Any : n as Any
        case .bool(let b): return b
        }
    }

    static func text(_ any: Any?) -> String {
        switch any {
        case let s as String: return s
        case let n as NSNumber:
            if CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue ? "Yes" : "No" }
            return n.stringValue
        default: return ""
        }
    }

    /// The outcome for a job; nil while it is still working.
    static func outcome(_ job: [String: Any]) -> BarRunOutcome? {
        switch job["state"] as? String ?? "" {
        case "completed": return .completed(receipt: receipt(job))
        case "failed":
            let err = job["error"]
            let why = (err as? String) ?? ((err as? [String: Any])?["message"] as? String) ?? ""
            return .failed(why)
        case "cancelled": return .cancelled
        case "waiting_approval":
            let approver = (job["result"] as? [String: Any])?["approver"] as? [String: Any]
            return .waitingApproval(approverName: approver?["name"] as? String)
        case "input_required": return .needsInput
        default: return nil
        }
    }

    /// "Refunded €320": past-tense verb of the tool title plus the amount
    /// from `result.structuredContent` or `args`. Falls back to "<title>: done",
    /// never raw JSON (CONTRACT section 5).
    static func receipt(_ job: [String: Any]) -> String {
        let title = (job["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (job["tool"] as? String ?? "Done")
        let structured = (job["result"] as? [String: Any])?["structuredContent"] as? [String: Any]
        let args = job["args"] as? [String: Any]
        let amount = (structured?["amount"] as? NSNumber) ?? (args?["amount"] as? NSNumber)
        let currency = (structured?["currency"] as? String) ?? (args?["currency"] as? String)
        let words = title.split(separator: " ").map(String.init)
        guard let verb = words.first.flatMap(pastTense) else { return "\(title): done" }
        if let amount {
            return "\(verb) \(money(amount.doubleValue, currency: currency))"
        }
        let rest = words.dropFirst().joined(separator: " ")
        return rest.isEmpty ? verb : "\(verb) \(rest.lowercased())"
    }

    private static func pastTense(_ verb: String) -> String? {
        let known: [String: String] = [
            "refund": "Refunded", "create": "Created", "send": "Sent", "add": "Added", "update": "Updated",
            "delete": "Deleted", "remove": "Removed", "cancel": "Cancelled", "list": "Listed", "post": "Posted",
            "charge": "Charged", "pay": "Paid", "book": "Booked", "schedule": "Scheduled", "close": "Closed",
            "open": "Opened", "archive": "Archived", "invite": "Invited", "approve": "Approved", "reply": "Replied",
        ]
        return known[verb.lowercased()]
    }

    private static func money(_ value: Double, currency: String?) -> String {
        guard let code = currency?.uppercased(), code.count == 3 else {
            return value.rounded() == value ? String(Int64(value)) : String(value)
        }
        let f = NumberFormatter()
        f.numberStyle = .currency
        f.currencyCode = code
        f.maximumFractionDigits = value.rounded() == value ? 0 : 2
        return f.string(from: NSNumber(value: value)) ?? "\(value) \(code)"
    }
}
