import Foundation

/// allternit-api team routes (contract §1) through the shared APIClient
/// (`AppConfig.apiBaseURL` is `…/api/v1`, Bearer auth and relay routing like
/// every bot call). Unlike `APIClient.get`, it keeps the error body's code so
/// "no computer", "signed out" and "team store unavailable" each get their own
/// sentence (`TeamMapError`). Mirrors ai `src/lib/team/api.ts`.
struct TeamMapClient {
    static let shared = TeamMapClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    // MARK: Reads

    /// `GET /api/v1/team/map`.
    func map() async throws -> TeamMap {
        try await call("team/map")
    }

    /// `GET /api/v1/team/rules`: every rule with all its fields (the map's short form lacks them).
    func rules() async throws -> [TeamFullRule] {
        let reply: RulesReply = try await call("team/rules")
        return reply.rules
    }

    /// `GET /api/v1/team/apps/:kind/:ref/access`: who can use one app.
    func appAccess(kind: String, ref: String) async throws -> TeamAppAccess {
        try await call("team/apps/\(Self.enc(kind))/\(Self.enc(ref))/access")
    }

    // MARK: Writes

    /// `PUT /api/v1/team/grants` (bot owner, org owner, admin).
    func setLevel(_ grant: TeamGrant, level: String) async throws -> TeamGrant {
        let body = GrantInput(botKind: grant.botKind, botId: grant.botId, appKind: grant.app.kind,
                              appRef: grant.app.ref, level: level)
        let reply: GrantReply = try await call("team/grants", method: "PUT", body: body)
        return reply.grant
    }

    /// `PUT /api/v1/team/people/:user_id`; nil = top of the team.
    func setReportsTo(userId: String, managerUserId: String?) async throws {
        let _: ReportsToReply = try await call("team/people/\(Self.enc(userId))", method: "PUT",
                                               body: ReportsToInput(reportsToUserId: managerUserId))
    }

    /// `POST /api/v1/team/rules` (org owner/admin).
    func createRule(_ input: TeamRuleInput) async throws {
        let _: RuleReply = try await call("team/rules", method: "POST", body: input)
    }

    /// `PATCH /api/v1/team/rules/:id` (org owner/admin): merged into the stored rule.
    func updateRule(id: String, input: TeamRuleInput) async throws {
        var patch = input
        patch.clearsMissing = true
        let _: RuleReply = try await call("team/rules/\(Self.enc(id))", method: "PATCH", body: patch)
    }

    /// `PATCH /api/v1/team/rules/:id` with only `enabled` (the row's On/Off).
    func setRuleEnabled(id: String, enabled: Bool) async throws {
        let _: RuleReply = try await call("team/rules/\(Self.enc(id))", method: "PATCH", body: EnabledInput(enabled: enabled))
    }

    /// `DELETE /api/v1/team/grants?bot_kind&bot_id&app_kind&app_ref` (bot owner, org owner, admin).
    func revokeGrant(_ grant: TeamGrant) async throws {
        let query = [("bot_kind", grant.botKind), ("bot_id", grant.botId),
                     ("app_kind", grant.app.kind), ("app_ref", grant.app.ref)]
            .map { "\($0.0)=\(Self.encQuery($0.1))" }.joined(separator: "&")
        let _: RevokedReply = try await call("team/grants?\(query)", method: "DELETE")
    }

    /// `DELETE /api/v1/team/rules/:id` (org owner/admin).
    func deleteRule(id: String) async throws {
        let _: DeletedReply = try await call("team/rules/\(Self.enc(id))", method: "DELETE")
    }

    /// Decides an ask through the call its `resolve` names (contract §2: the
    /// client never picks the store's route itself). The body's `decision`
    /// is set to "approve" or "deny"; every other field goes back as sent.
    func resolve(_ ask: TeamAsk, approve: Bool) async throws {
        guard let resolve = ask.resolve else { throw TeamMapError.readOnly }
        let method = resolve.method.uppercased()
        var request = try await client.authorizedRequest(url: url(for: resolve.path), method: method)
        var body: [String: JSONValue] = [:]
        if case .object(let fields)? = resolve.body { body = fields }
        body["decision"] = .string(approve ? "approve" : "deny")
        if method != "GET" {
            request.httpBody = try JSONEncoder().encode(body)
        }
        _ = try await send(request)
    }

    // MARK: Transport

    private func call<T: Decodable>(_ path: String, method: String = "GET") async throws -> T {
        let request = try await client.authorizedRequest(path: path, method: method)
        return try decode(try await send(request))
    }

    private func call<T: Decodable, B: Encodable>(_ path: String, method: String, body: B) async throws -> T {
        var request = try await client.authorizedRequest(path: path, method: method)
        request.httpBody = try JSONEncoder().encode(body)
        return try decode(try await send(request))
    }

    private func decode<T: Decodable>(_ data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw TeamMapError.storeUnavailable
        }
    }

    /// Sends and returns the body of a 2xx reply; anything else is a `TeamMapError`.
    private func send(_ request: URLRequest) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await client.send(request)
        } catch let error as APIError {
            // `.unavailable` routing (Cloud picked, no paired computer) arrives as a 503 here.
            if case .httpError(let status, _) = error, status == 503 || status == 409 || status == 428 {
                throw TeamMapError.noRuntime
            }
            throw TeamMapError.from(error)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            // No reply at all (offline, computer asleep): status 0 = no runtime (contract §3).
            throw TeamMapError.noRuntime
        }
        guard let http = response as? HTTPURLResponse else { throw TeamMapError.storeUnavailable }
        guard (200...299).contains(http.statusCode) else {
            throw TeamMapError.from(status: http.statusCode, body: TeamErrorBody.parse(data))
        }
        return data
    }

    /// `resolve.path` is a server path such as `/api/v1/cowork/approvals`;
    /// it is resolved against the API host, so a store outside `/api/v1`
    /// still works. A full URL is used as sent.
    private func url(for path: String) -> URL {
        if let absolute = URL(string: path), absolute.scheme != nil { return absolute }
        let base = client.baseURL
        var components = URLComponents(url: base, resolvingAgainstBaseURL: false) ?? URLComponents()
        let split = path.split(separator: "?", maxSplits: 1, omittingEmptySubsequences: false)
        let pathOnly = String(split.first ?? "")
        if pathOnly.hasPrefix("/") {
            components.path = pathOnly
        } else {
            components.path = base.path.hasSuffix("/") ? base.path + pathOnly : base.path + "/" + pathOnly
        }
        components.percentEncodedQuery = split.count > 1 ? String(split[1]) : nil
        return components.url ?? base.appendingPathComponent(pathOnly)
    }

    private static func encQuery(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-._~"))) ?? value
    }

    private static func enc(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? value
    }
}

// MARK: - Bodies

private struct GrantInput: Encodable {
    let botKind: String
    let botId: String
    let appKind: String
    let appRef: String
    let level: String

    enum CodingKeys: String, CodingKey {
        case level
        case botKind = "bot_kind"
        case botId = "bot_id"
        case appKind = "app_kind"
        case appRef = "app_ref"
    }
}

private struct GrantReply: Decodable { let grant: TeamGrant }

private struct ReportsToInput: Encodable {
    let reportsToUserId: String?

    enum CodingKeys: String, CodingKey { case reportsToUserId = "reports_to_user_id" }

    // `null` means "top of the team", so it is sent, not omitted.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(reportsToUserId, forKey: .reportsToUserId)
    }
}

private struct ReportsToReply: Decodable {}
private struct RuleReply: Decodable {}
private struct DeletedReply: Decodable {}
private struct RevokedReply: Decodable {}
private struct RulesReply: Decodable { let rules: [TeamFullRule] }
private struct EnabledInput: Encodable { let enabled: Bool }

/// Rule input (contract §2, POST/PATCH /team/rules). On create, nil fields are
/// left out; on PATCH (`clearsMissing`) they are sent as null so a cleared
/// blank (back to "any app", no field, no unit) clears the stored value.
struct TeamRuleInput: Encodable, Equatable {
    var scopeKind: String
    var scopeRef: String?
    var appKind: String?
    var appRef: String?
    var tool: String
    var field: String?
    var op: String
    var value: JSONValue?
    var unitLabel: String?
    var approverKind: String
    var approverUserId: String?
    var clearsMissing = false

    enum CodingKeys: String, CodingKey {
        case tool, field, op, value
        case scopeKind = "scope_kind"
        case scopeRef = "scope_ref"
        case appKind = "app_kind"
        case appRef = "app_ref"
        case unitLabel = "unit_label"
        case approverKind = "approver_kind"
        case approverUserId = "approver_user_id"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(scopeKind, forKey: .scopeKind)
        try c.encode(tool, forKey: .tool)
        try c.encode(op, forKey: .op)
        try c.encode(approverKind, forKey: .approverKind)
        try put(&c, scopeRef, .scopeRef)
        try put(&c, appKind, .appKind)
        try put(&c, appRef, .appRef)
        try put(&c, field, .field)
        try put(&c, value, .value)
        try put(&c, unitLabel, .unitLabel)
        try put(&c, approverUserId, .approverUserId)
    }

    private func put<V: Encodable>(_ c: inout KeyedEncodingContainer<CodingKeys>, _ v: V?, _ key: CodingKeys) throws {
        if let v { try c.encode(v, forKey: key) } else if clearsMissing { try c.encodeNil(forKey: key) }
    }
}

// MARK: - Errors

/// `{code, message}` from any of the runtime's error bodies (team: `{error:
/// <code>, message}`; bar: `{error: <sentence>, code}`; gate: `{error: {code,
/// message}}`). Same reading as ai `codeAndMessage`.
struct TeamErrorBody {
    var code: String?
    var message: String?

    static func parse(_ data: Data) -> TeamErrorBody {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return TeamErrorBody()
        }
        var code = object["code"] as? String
        var message = object["message"] as? String
        if let nested = object["error"] as? [String: Any] {
            code = code ?? (nested["code"] as? String) ?? ((nested["data"] as? [String: Any])?["code"] as? String)
            message = message ?? (nested["message"] as? String)
        } else if let error = object["error"] as? String {
            if error.range(of: #"^[a-z0-9_.-]+$"#, options: .regularExpression) != nil {
                code = code ?? error
            } else {
                message = message ?? error
            }
        }
        if message == nil { message = object["detail"] as? String }
        return TeamErrorBody(code: code, message: message)
    }
}

/// Every refusal as one plain sentence (contract §3: never a raw code).
enum TeamMapError: LocalizedError, Equatable {
    /// No computer reachable: 409 `no_runtime`, 428, or no reply.
    case noRuntime
    /// 401: the session is gone.
    case signedOut
    /// The team store could not be read or written (500 `team_*_failed`, 503 `team_rules_unavailable`).
    case storeUnavailable
    /// 503 `tier_check_unavailable`: the ceiling check is down (fails closed).
    case cannotChangeNow
    /// 403 owner/admin only.
    case notAllowed
    /// A platform ask with no resolve route.
    case readOnly
    /// Any other refusal, with the server's own sentence when it sent one.
    case refused(String)

    var errorDescription: String? {
        switch self {
        case .noRuntime: return "No Allternit computer is reachable. Connect one and try again."
        case .signedOut: return "You're signed out. Sign in again to see your team."
        case .storeUnavailable: return "The team list isn't available right now. Try again in a moment."
        case .cannotChangeNow: return "Can't change this right now. Try again in a moment."
        case .notAllowed: return "Only the owner or an admin can change this."
        case .readOnly: return "This ask can only be decided on the platform."
        case .refused(let text): return text
        }
    }

    static func from(status: Int, body: TeamErrorBody) -> TeamMapError {
        let code = body.code?.lowercased()
        switch code {
        case "no_runtime", "runtime_unavailable", "runtime_offline": return .noRuntime
        case "tier_check_unavailable": return .cannotChangeNow
        case "team_rules_unavailable", "team_map_failed", "team_approvals_failed", "team_access_failed",
             "team_write_failed", "team_grants_failed", "team_rules_failed":
            return .storeUnavailable
        case "owner_or_admin_required", "bot_owner_or_admin_required": return .notAllowed
        case "reports_to_cycle": return .refused("That would make a loop: someone would report to their own report.")
        case "manager_not_member", "not_member": return .refused("That person isn't on this team.")
        case "no_org": return .refused("Make a team first to set who reports to whom.")
        case "above_ceiling": return .refused("That level is above what this app allows here. Pick a lower level.")
        case "app_not_installed": return .refused("This app isn't installed for the bot's owner, so its level can't change.")
        case "bot_not_found": return .refused("This bot isn't on the team anymore. Reload the map.")
        case "rule_not_found": return .refused("This rule was deleted. Reload the map.")
        case "invalid_rule":
            return .refused(plainSentence(body.message) ?? "That rule doesn't work. Check each part and try again.")
        default: break
        }
        if status == 401 { return .signedOut }
        if status == 403 { return .notAllowed }
        if status == 409 || status == 428 { return .noRuntime }
        if status == 503 || status >= 500 { return .storeUnavailable }
        return .refused(plainSentence(body.message) ?? "That didn't work. Try again.")
    }

    static func from(_ error: APIError) -> TeamMapError {
        if case .httpError(let status, let message) = error {
            return from(status: status, body: TeamErrorBody(code: nil, message: message))
        }
        return .storeUnavailable
    }

    /// The server's sentence, unless it is a bare code or a status line.
    private static func plainSentence(_ text: String?) -> String? {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty,
              text.range(of: #"^[a-z0-9_.-]+$"#, options: .regularExpression) == nil,
              text.range(of: #"^HTTP \d+"#, options: .regularExpression) == nil else { return nil }
        return text
    }

    /// Any thrown value as a sentence.
    static func text(_ error: Error) -> String {
        if let team = error as? TeamMapError { return team.errorDescription ?? "That didn't work. Try again." }
        return plainErrorText(error)
    }
}
