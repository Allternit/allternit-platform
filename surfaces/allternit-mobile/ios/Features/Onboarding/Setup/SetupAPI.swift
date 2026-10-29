import Foundation

// -----------------------------------------------------------------------------
// The calls behind the first-run setup, ported from allternit-ai
// (services/setup-api.ts, setup/setupSignals.ts, setup/inventorySources.ts,
// lib/sessions-computer-api.ts, lib/paired-computers.ts, lib/setup/setupChecks.ts):
//
//   Setup config (runtime, under /api, not /api/v1):
//     GET/POST /api/onboarding/config     POST /api/onboarding/validate-key
//     POST     /api/onboarding/provider
//   Brains (runtime):    GET providers/auth/status   GET providers/:id/models
//   Allternit Cloud:     GET {cloud}/v1/models (public catalog)
//   Cloud computer:      GET provisioned-instances   GET hosted-runtimes/entitlement
//   Subscriptions:       GET subscriptions/status
//                        GET/POST subscriptions/gateway/v1/accounts
//                        POST …/accounts/:id/login   POST …/accounts/:id/connect
//                        POST subscriptions/disclosure/ack (person proof, D16)
//   Your own server:     POST {cloud}/api/v1/computers/pairing-codes
//                        GET  {cloud}/api/v1/computers/paired
//   Checklist:           GET setup/checks
//   Connected view:      gateway/provider-accounts, bot-projects, mcp/connectors,
//                        memory/drive/mounts, agents (MeetAgentsAPI), agent-preferences
//
// Runtime and control-plane calls go through APIClient.shared (cloud-api answers
// its own routes and relays the rest to the person's computer). Cloud-only
// calls (`cloud`) go straight to cloud-api, like the web's absolute URLs.
// -----------------------------------------------------------------------------

enum SetupHTTP {
    /// `/api/v1/<path>` through the app's route (relay when on a computer).
    static func v1(_ method: String, _ path: String, body: JSONValue? = nil) async throws -> JSONValue {
        try await ConnectHTTP.call(method, path, body: body)
    }

    /// `/api/<path>` on the runtime (the onboarding router is not under /api/v1).
    static func api(_ method: String, _ path: String, body: JSONValue? = nil, timeout: TimeInterval = 20) async throws -> JSONValue {
        let base = AppConfig.apiBaseURL.lastPathComponent == "v1"
            ? AppConfig.apiBaseURL.deletingLastPathComponent()
            : AppConfig.apiBaseURL
        var request = try await APIClient.shared.authorizedRequest(url: base.appendingPathComponent(path), method: method)
        request.timeoutInterval = timeout
        return try await perform(request, body: body, direct: false)
    }

    /// Straight to cloud-api (`{cloud}/<path>`), skipping the runtime relay.
    static func cloud(_ method: String, _ path: String, body: JSONValue? = nil,
                      headers: [String: String] = [:]) async throws -> JSONValue {
        var request = try await APIClient.shared.authorizedRequest(
            url: AppConfig.cloudAPIBaseURL.appendingPathComponent(path), method: method)
        for (k, v) in headers { request.setValue(v, forHTTPHeaderField: k) }
        return try await perform(request, body: body, direct: true)
    }

    private static func perform(_ req: URLRequest, body: JSONValue?, direct: Bool) async throws -> JSONValue {
        var request = req
        if let body { request.httpBody = try JSONEncoder().encode(body) }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = direct
                ? try await APIClient.shared.session.data(for: request)
                : try await APIClient.shared.send(request)
        } catch let error as APIError {
            if case .httpError(let status, let message) = error {
                throw ConnectError(status: status, code: nil, message: message ?? "Request failed (\(status)).")
            }
            throw ConnectError(status: nil, code: nil, message: FriendlyErrorMessage.from(error.localizedDescription))
        } catch {
            throw ConnectError(status: nil, code: nil, message: FriendlyErrorMessage.from(error.localizedDescription))
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        let json = (try? JSONDecoder().decode(JSONValue.self, from: data)) ?? .null
        guard (200...299).contains(status) else {
            let code = json.at("code")?.jString ?? json.at("error")?.jString
            let message = json.at("message")?.jString ?? code
                ?? (status == 401 || status == 403 ? "Sign in again, then try again." : "Request failed (\(status)).")
            throw ConnectError(status: status, code: code, message: message)
        }
        return json
    }
}

extension JSONValue {
    /// A number field (JSON numbers decode as Double).
    var setupNumber: Double? { if case .number(let n) = self { return n } else { return nil } }
}

/// Resolves to the first of `operation` or nil after `seconds`.
func setupWithTimeout<T: Sendable>(_ seconds: Double, _ operation: @escaping @Sendable () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw ConnectError(status: nil, code: "timeout", message: "That took too long.")
        }
        defer { group.cancelAll() }
        guard let first = try await group.next() else { throw CancellationError() }
        return first
    }
}

// MARK: - Models

struct SetupModelRow: Sendable, Equatable, Identifiable {
    let id: String
    let name: String
}

struct SetupProviderAuth: Sendable, Equatable {
    let id: String
    let authenticated: Bool
}

struct SetupChannelAccount: Sendable, Equatable {
    let id: String
    let vendor: String
    let displayName: String?
    let state: String
    let authType: String
}

struct SetupConnector: Sendable, Equatable {
    let id: String
    let name: String
    let connected: Bool
}

struct SetupCloudComputer: Sendable, Equatable {
    let id: String
    let status: String
    let cpuCores: Int?
    let memoryMb: Int?

    var specs: String? {
        guard let cpuCores, let memoryMb, cpuCores > 0, memoryMb > 0 else { return nil }
        return "\(cpuCores) vCPU · \(Int((Double(memoryMb) / 1024).rounded())) GB"
    }

    var isRunning: Bool {
        status.range(of: "running|active|ready|online", options: [.regularExpression, .caseInsensitive]) != nil
    }
}

struct SetupPlan: Sendable, Equatable {
    let id: String
    let name: String
    var isFree: Bool { id == "free" }
}

/// A Subscription Fabric login on the Sessions computer.
struct SetupFabricAccount: Sendable, Equatable, Identifiable {
    let accountId: String
    let provider: String
    let label: String
    let identity: String?
    let enabled: Bool
    let sessionHealth: String
    var id: String { accountId }

    /// Can serve chats (same rule as the model picker's Connected label).
    var isReady: Bool { enabled && (sessionHealth == "ready" || sessionHealth == "degraded") }

    init?(_ v: JSONValue) {
        guard let id = v.at("account_id")?.jString, let provider = v.at("provider")?.jString else { return nil }
        accountId = id
        self.provider = provider
        label = v.at("label")?.jString ?? provider
        identity = v.at("identity")?.jString
        enabled = v.at("enabled")?.jBool ?? false
        sessionHealth = v.at("session_health")?.jString ?? "unknown"
    }
}

/// A login window's state (web `FabricLoginState`).
struct SetupLoginState: Sendable, Equatable {
    enum State: String, Sendable { case none, waiting, signedIn = "signed_in", failed, closed }
    let state: State
    let detail: String?
    let account: SetupFabricAccount?

    init(_ v: JSONValue) {
        state = State(rawValue: v.at("state")?.jString ?? "") ?? .none
        detail = v.at("detail")?.jString
        account = v.at("account").flatMap(SetupFabricAccount.init)
    }
}

struct SetupSubscriptionStatus: Sendable, Equatable {
    struct Terms: Sendable, Equatable {
        let provider: String
        let name: String
        let text: String
        let acknowledged: Bool
    }
    let bound: Bool
    let running: Bool
    let computerId: String?
    let computerName: String?
    let disclosureVersion: Int?
    let terms: [Terms]

    init(_ v: JSONValue) {
        let ref = v.at("computer")
        let present = ref?.jObject != nil && ref?.at("missing")?.jBool != true
        bound = v.at("bound")?.jBool == true && present
        running = bound && ref?.at("running")?.jBool == true
        computerId = present ? ref?.at("id")?.jString : nil
        computerName = present ? ref?.at("name")?.jString : nil
        disclosureVersion = v.at("disclosure")?.at("version")?.setupNumber.map { Int($0) }
        terms = (v.at("disclosure")?.at("providers")?.jArray ?? []).compactMap { p in
            guard let id = p.at("provider")?.jString else { return nil }
            return Terms(provider: id, name: p.at("name")?.jString ?? id, text: p.at("text")?.jString ?? "",
                         acknowledged: p.at("acknowledged")?.jBool == true)
        }
    }
}

struct SetupPairingCode: Sendable, Equatable {
    let code: String
    let command: String
    let expiresAt: Date
}

struct SetupPairedComputer: Sendable, Equatable {
    let id: String
    let name: String
    let os: String?
}

enum SetupCheckStatus: String, Sendable { case ready, notChecked = "not_checked", needsSetup = "needs_setup"
    var label: String {
        switch self {
        case .ready: return "Ready"
        case .notChecked: return "Not checked"
        case .needsSetup: return "Needs setup"
        }
    }
}

/// One row of the live setup checklist (`GET /api/v1/setup/checks`).
struct SetupCheckRow: Sendable, Equatable, Identifiable {
    let id: String
    let title: String
    let required: Bool
    let status: SetupCheckStatus
    let detail: String
    let fixLabel: String?
    let fixSection: String?
}

// MARK: - Calls

enum SetupAPI {
    private static func iso(_ s: String?) -> Date? {
        guard let s else { return nil }
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    // Setup config

    /// `user.onboardingComplete` from the server's setup config (authoritative).
    static func onboardingComplete() async throws -> Bool {
        let json = try await SetupHTTP.api("GET", "onboarding/config", timeout: 8)
        return json.at("user")?.at("onboardingComplete")?.jBool == true
    }

    /// Persists the default model and the wizard-completion flag (web completeFirstRun).
    static func saveConfig(defaultModel: String?, mode: String) async throws {
        var body: [String: JSONValue] = [
            "onboardingComplete": .bool(true),
            "wizard": .object([
                "lastRunAt": .string(ISO8601DateFormatter().string(from: Date())),
                "lastRunVersion": .string(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "ios"),
                "lastRunCommand": .string("onboard"),
                "lastRunMode": .string(mode),
            ]),
        ]
        if let defaultModel { body["defaultModel"] = .string(defaultModel) }
        _ = try await SetupHTTP.api("POST", "onboarding/config", body: .object(body), timeout: 8)
    }

    static func validateKey(provider: String, key: String) async throws -> (valid: Bool, models: [SetupModelRow], error: String?) {
        let json = try await SetupHTTP.api("POST", "onboarding/validate-key",
                                           body: .object(["provider": .string(provider), "key": .string(key)]))
        return (json.at("valid")?.jBool == true, models(json.at("models")), json.at("error")?.jString)
    }

    /// Saves provider metadata (and, with a key, the key to the runtime's keychain) and makes it the default.
    static func saveProvider(provider: String, apiKey: String?, models: [SetupModelRow]) async throws {
        var map: [String: JSONValue] = [:]
        for m in models {
            map[m.id] = .object(["id": .string(m.id), "name": .string(m.name), "tool_call": .bool(true),
                                 "limit": .object(["context": .number(200000), "output": .number(32000)])])
        }
        var body: [String: JSONValue] = [
            "provider": .string(provider), "authType": .string("api_key"),
            "models": .object(map), "setDefault": .bool(true),
        ]
        if let apiKey { body["apiKey"] = .string(apiKey) }
        if let first = models.first { body["defaultModel"] = .string("\(provider)/\(first.id)") }
        _ = try await SetupHTTP.api("POST", "onboarding/provider", body: .object(body))
    }

    /// Models for a provider that already has a saved key (no key paste needed).
    static func discoverProviderModels(_ provider: String) async throws -> [SetupModelRow] {
        let json = try await SetupHTTP.v1("GET", "providers/\(ConnectHTTP.escape(provider))/models")
        return json.at("supported")?.jBool == true ? models(json.at("models")) : []
    }

    private static func models(_ v: JSONValue?) -> [SetupModelRow] {
        (v?.jArray ?? []).compactMap { m in
            guard let id = m.at("id")?.jString else { return nil }
            return SetupModelRow(id: id, name: m.at("name")?.jString ?? id)
        }
    }

    // Brains

    /// Provider ids the runtime reports as authenticated (anthropic, openai, claude-cli…).
    static func providerAuthStatus() async throws -> [SetupProviderAuth] {
        let json = try await SetupHTTP.v1("GET", "providers/auth/status")
        return (json.at("providers")?.jArray ?? []).compactMap { row in
            guard let id = row.at("provider_id")?.jString else { return nil }
            return SetupProviderAuth(id: id, authenticated: row.at("authenticated")?.jBool == true)
        }
    }

    /// The live Allternit Cloud catalog (`{cloud}/v1/models`, OpenAI-style `data`).
    static func cloudModelCatalog() async -> [SetupModelRow] {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("v1/models")
        guard let (data, response) = try? await URLSession.shared.data(from: url),
              let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode),
              let json = try? JSONDecoder().decode(JSONValue.self, from: data) else { return [] }
        return (json.at("data")?.jArray ?? []).compactMap { item in
            guard let id = item.at("id")?.jString?.trimmingCharacters(in: .whitespaces), !id.isEmpty else { return nil }
            let name = item.at("name")?.jString ?? item.at("extra")?.at("name")?.jString ?? id
            return SetupModelRow(id: id, name: name.isEmpty ? id : name)
        }
    }

    // Cloud computer

    static func cloudComputers() async throws -> [SetupCloudComputer] {
        let json = try await SetupHTTP.v1("GET", "provisioned-instances")
        let list = json.jObject != nil ? (json.at("instances")?.jArray ?? []) : json.jArray
        return list.compactMap { c in
            guard let id = c.at("id")?.jString else { return nil }
            let status = c.at("status")?.jString ?? "unknown"
            guard status != "deleted" else { return nil }
            return SetupCloudComputer(id: id, status: status,
                                      cpuCores: c.at("cpuCores")?.setupNumber.map { Int($0) },
                                      memoryMb: c.at("memoryMb")?.setupNumber.map { Int($0) })
        }
    }

    static func hostedPlan() async throws -> SetupPlan {
        let json = try await SetupHTTP.v1("GET", "hosted-runtimes/entitlement")
        let id = json.at("planTierId")?.jString ?? json.at("plan_tier_id")?.jString ?? "unknown"
        let name = json.at("planDisplayName")?.jString ?? json.at("plan_display_name")?.jString ?? id
        return SetupPlan(id: id, name: name)
    }

    // Subscription Fabric (on the Sessions computer)

    private static let gateway = "subscriptions/gateway/v1"

    static func subscriptionStatus() async throws -> SetupSubscriptionStatus {
        SetupSubscriptionStatus(try await SetupHTTP.v1("GET", "subscriptions/status"))
    }

    static func fabricAccounts() async throws -> [SetupFabricAccount] {
        try await SetupHTTP.v1("GET", "\(gateway)/accounts").jArray.compactMap(SetupFabricAccount.init)
    }

    /// Adds a login and opens its sign-in window on the Sessions computer.
    static func createFabricAccount(provider: String) async throws -> SetupFabricAccount {
        let json = try await SetupHTTP.v1("POST", "\(gateway)/accounts",
                                          body: .object(["provider": .string(provider), "login": .bool(true)]))
        guard let account = SetupFabricAccount(json) else {
            throw ConnectError(status: nil, code: nil, message: "The Sessions computer didn’t return the new login.")
        }
        // Older gateways create without opening the window.
        if json.at("login")?.jObject == nil {
            _ = try? await SetupHTTP.v1("POST", "\(gateway)/accounts/\(ConnectHTTP.escape(account.accountId))/login")
        }
        return account
    }

    static func openFabricLogin(_ accountId: String) async throws {
        _ = try await SetupHTTP.v1("POST", "\(gateway)/accounts/\(ConnectHTTP.escape(accountId))/login")
    }

    /// Where a login window stands (gateways from 2026-10; older ones answer 404).
    static func fabricLoginState(_ accountId: String) async throws -> SetupLoginState {
        SetupLoginState(try await SetupHTTP.v1("GET", "\(gateway)/accounts/\(ConnectHTTP.escape(accountId))/login"))
    }

    /// Closes an unfinished login window; the account stays auth_required.
    static func cancelFabricLogin(_ accountId: String) async throws {
        _ = try await SetupHTTP.v1("DELETE", "\(gateway)/accounts/\(ConnectHTTP.escape(accountId))/login")
    }

    /// Re-runs the non-spending session probe after a sign-in.
    static func checkFabricAccount(_ accountId: String) async throws -> SetupFabricAccount? {
        SetupFabricAccount(try await SetupHTTP.v1("POST", "\(gateway)/accounts/\(ConnectHTTP.escape(accountId))/connect"))
    }

    /// The person accepting a provider's terms notice (D16): carries their
    /// own session proof (the Clerk session token, as the web phone does).
    static func acknowledgeDisclosure(provider: String, version: Int) async throws {
        var request = try await APIClient.shared.authorizedRequest(path: "subscriptions/disclosure/ack", method: "POST")
        if let token = try await AuthManager.shared.getToken(), !token.isEmpty {
            request.setValue(token, forHTTPHeaderField: "X-Allternit-Human-Proof")
        }
        request.httpBody = try JSONEncoder().encode(JSONValue.object(["provider": .string(provider), "version": .number(Double(version))]))
        let (data, response) = try await APIClient.shared.send(request)
        try APIClient.shared.validate(response, data: data)
    }

    // Your own server (account-level pairing, cloud-api)

    static func createPairingCode() async throws -> SetupPairingCode {
        let json = try await SetupHTTP.cloud("POST", "api/v1/computers/pairing-codes")
        guard let code = json.at("code")?.jString, let command = json.at("command")?.jString,
              let expires = iso(json.at("expiresAt")?.jString) else {
            throw ConnectError(status: nil, code: nil, message: "Couldn’t make a pairing code")
        }
        return SetupPairingCode(code: code, command: command, expiresAt: expires)
    }

    static func pairedComputers() async throws -> [SetupPairedComputer] {
        let json = try await SetupHTTP.cloud("GET", "api/v1/computers/paired")
        return (json.at("computers")?.jArray ?? []).compactMap { c in
            guard let id = c.at("id")?.jString else { return nil }
            return SetupPairedComputer(id: id, name: c.at("name")?.jString ?? id, os: c.at("os")?.jString)
        }
    }

    // Setup checklist

    private static let checkOrder = ["account", "brain", "google", "accounts", "notifications", "computers", "channels"]

    /// Server rows, well-formed ones only, in display order (device rows are added by the caller).
    static func setupChecks() async throws -> [SetupCheckRow] {
        let json = try await setupWithTimeout(15) { try await SetupHTTP.v1("GET", "setup/checks") }
        let rows: [SetupCheckRow] = (json.at("rows")?.jArray ?? []).compactMap { r in
            guard let id = r.at("id")?.jString, let title = r.at("title")?.jString, id != "notifications" else { return nil }
            let fix = r.at("fix")
            let label = fix?.at("label")?.jString, section = fix?.at("section")?.jString
            return SetupCheckRow(id: id, title: title, required: r.at("required")?.jBool == true,
                                 status: SetupCheckStatus(rawValue: r.at("status")?.jString ?? "") ?? .notChecked,
                                 detail: r.at("detail")?.jString ?? "",
                                 fixLabel: label != nil && section != nil ? label : nil,
                                 fixSection: label != nil && section != nil ? section : nil)
        }
        return rows
    }

    static func sortChecks(_ rows: [SetupCheckRow]) -> [SetupCheckRow] {
        func rank(_ id: String) -> Int { checkOrder.firstIndex(of: id) ?? checkOrder.count }
        return rows.enumerated().sorted { a, b in
            rank(a.element.id) == rank(b.element.id) ? a.offset < b.offset : rank(a.element.id) < rank(b.element.id)
        }.map(\.element)
    }

    // Connected view sources

    static func channelAccounts() async throws -> [SetupChannelAccount] {
        let json = try await SetupHTTP.v1("GET", "gateway/provider-accounts")
        return (json.at("accounts")?.jArray ?? []).compactMap { a in
            guard let id = a.at("id")?.jString else { return nil }
            return SetupChannelAccount(id: id, vendor: a.at("vendor")?.jString ?? "", displayName: a.at("displayName")?.jString,
                                       state: a.at("state")?.jString ?? "", authType: a.at("authType")?.jString ?? "")
        }
    }

    static func botProjects() async throws -> [SetupModelRow] {
        let json = try await SetupHTTP.v1("GET", "bot-projects")
        return (json.at("projects")?.jArray ?? []).compactMap { p in
            guard let id = p.at("id")?.jString else { return nil }
            return SetupModelRow(id: id, name: p.at("title")?.jString ?? id)
        }
    }

    static func mcpConnectors() async throws -> [SetupConnector] {
        let json = try await SetupHTTP.v1("GET", "mcp/connectors")
        return (json.at("connectors")?.jArray ?? []).compactMap { c in
            guard let id = c.at("id")?.jString else { return nil }
            let enabled = c.at("enabled")?.jBool != false
            let reauth = c.at("needs_reauth")?.jBool == true
            return SetupConnector(id: id, name: c.at("name")?.jString ?? id, connected: enabled && !reauth)
        }
    }

    /// The name the person gave us (Settings → Profile), as typed.
    static func preferredName() async -> String? {
        guard let json = try? await SetupHTTP.v1("GET", "agent-preferences") else { return nil }
        let prefs = json.at("preferences") ?? json
        if let p = prefs.at("preferred_name")?.jString, !p.trimmingCharacters(in: .whitespaces).isEmpty { return p }
        return prefs.at("full_name")?.jString?.split(separator: " ").first.map(String.init)
    }
}
