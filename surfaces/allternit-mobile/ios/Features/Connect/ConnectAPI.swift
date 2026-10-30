import Foundation

// -----------------------------------------------------------------------------
// The calls behind the iPhone's connect flows, ported from allternit-ai:
//   - one-step vendor connect (lib/gateway/api.ts connectCatalog / connect):
//       GET  gateway/connect            POST gateway/connect
//   - channel connect wizards (lib/gateway/connect-api.ts):
//       Telegram managed bots  GET  gateway/channel-accounts/telegram/managed/available
//                              POST channel-onboarding/telegram        (cloud)
//                              GET  channel-onboarding/telegram/:id    (cloud)
//       Email                  GET  agent-email/status
//                              POST agents/:id/identity/email
//                              PATCH agents/:id  (reply mode)
//       Phone number           GET  channels/availability              (cloud)
//                              GET  phone/numbers/search               (cloud)
//                              POST phone/numbers                      (cloud)
//                              POST/GET phone/numbers/:id/registration (cloud)
//                              POST phone/numbers/:id/registration/otp (cloud)
//       Slack/Discord/Teams/WhatsApp Business
//                              GET  channels/slack/install             (cloud)
//                              POST channels/discord/install           (cloud)
//                              POST channels/teams/connect             (cloud)
//                              POST channels/whatsapp/connect          (cloud)
//                              POST gateway/channel-accounts/:provider (computer: record it)
//                              GET/PUT channels/discord/installs/:guild/channels (cloud)
// Everything goes through APIClient.shared: cloud-api answers its own
// control-plane routes and relays the rest to the person's computer.
// -----------------------------------------------------------------------------

/// A failed connect call, with the server's status, code and plain message.
struct ConnectError: LocalizedError, Sendable {
    let status: Int?
    let code: String?
    let message: String

    var errorDescription: String? { message }

    /// 404/503: the backend for this connection isn't on this account yet ("coming soon", not an error).
    var isUnavailable: Bool { status == 404 || status == 503 }
}

/// Plain words for the cloud's phone error codes (connect-api.ts PHONE_ERRORS).
private let phoneErrors: [String: String] = [
    "phone_requires_plan": "Phone numbers come with a paid plan. Upgrade, then pick your number.",
    "number_limit": "You have all the numbers your plan includes. Release one or upgrade.",
    "number_taken": "That number was just taken. Pick another one.",
    "runtime_not_found": "Your computer isn’t signed in to your Allternit account. Sign in there, then try again.",
    "phone_not_configured": "Phone numbers aren’t available right now.",
    "already_registered": "This number is already registered.",
    "registration_not_pending": "This registration is no longer waiting for a code.",
]

enum ConnectHTTP {
    /// One JSON call; throws `ConnectError` with the body's code and message.
    static func call(_ method: String, _ path: String, body: JSONValue? = nil) async throws -> JSONValue {
        var request = try await APIClient.shared.authorizedRequest(path: path, method: method)
        if let body {
            request.httpBody = try JSONEncoder().encode(body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await APIClient.shared.send(request)
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
            var code: String?
            var message: String?
            if case .object(let o) = json {
                if case .object(let nested)? = o["error"] {
                    code = nested["code"]?.jString
                    message = nested["message"]?.jString
                } else {
                    code = o["error"]?.jString ?? o["code"]?.jString
                }
                message = message ?? o["message"]?.jString
                if case .object(let details)? = o["details"], let c = details["error"]?.jString { code = code ?? c }
            }
            let text = code.flatMap { phoneErrors[$0] } ?? message ?? code.map { "\($0.replacingOccurrences(of: "_", with: " ")) (\(status))" }
                ?? (status == 401 || status == 403 ? "Sign in again, then try again." : "Request failed (\(status)).")
            throw ConnectError(status: status, code: code, message: text)
        }
        return json
    }

    static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/?&="))) ?? value
    }
}

extension JSONValue {
    var jString: String? { if case .string(let s) = self { return s } else { return nil } }
    var jBool: Bool? { if case .bool(let b) = self { return b } else { return nil } }
    var jArray: [JSONValue] { if case .array(let a) = self { return a } else { return [] } }
    var jObject: [String: JSONValue]? { if case .object(let o) = self { return o } else { return nil } }
    func at(_ key: String) -> JSONValue? { jObject?[key] }
}

// MARK: - One-step vendor connect

/// A bot the one-step connect created for a vendor agent.
struct ConnectedBot: Hashable, Sendable {
    let botId: String
    let name: String

    init?(_ v: JSONValue) {
        guard let id = v.at("botId")?.jString else { return nil }
        botId = id
        name = v.at("name")?.jString ?? id
    }
}

/// `GET gateway/connect` row: a connectable product and this person's state for it.
struct ConnectProduct: Identifiable, Hashable, Sendable {
    let product: String
    let name: String
    let offers: String
    /// browser · desktop_app · api_key · local_endpoint · channel
    let signIn: String
    let verifiedLive: Bool
    let terms: String?
    let accountState: String?
    let bots: [ConnectedBot]

    var id: String { product }
    var isConnected: Bool { accountState == "CONNECTED" }

    init?(_ v: JSONValue) {
        guard let p = v.at("product")?.jString else { return nil }
        product = p
        name = v.at("name")?.jString ?? p
        offers = v.at("offers")?.jString ?? ""
        signIn = v.at("signIn")?.jString ?? "browser"
        verifiedLive = v.at("verifiedLive")?.jBool ?? false
        terms = v.at("terms")?.jString
        accountState = v.at("account")?.at("state")?.jString
        bots = (v.at("bots")?.jArray ?? []).compactMap(ConnectedBot.init)
    }

    /// QuickConnect SIGN_IN_TEXT.
    var signInText: String {
        switch signIn {
        case "desktop_app": return "Uses the desktop app"
        case "api_key": return "Uses an API key"
        case "local_endpoint": return "Uses your local server"
        case "channel": return "Uses WhatsApp"
        default: return "Sign in on the website"
        }
    }
}

/// `POST gateway/connect` answer.
struct ConnectResult: Sendable {
    /// ready · needs_consent · needs_sign_in · needs_key · needs_setup · blocked
    let status: String
    let message: String
    let steps: [String]
    let terms: String?
    let bots: [ConnectedBot]

    init(_ v: JSONValue) {
        status = v.at("status")?.jString ?? "needs_setup"
        message = v.at("message")?.jString ?? ""
        steps = (v.at("action")?.at("steps")?.jArray ?? []).compactMap(\.jString)
        terms = v.at("action")?.at("terms")?.jString
        bots = (v.at("bots")?.jArray ?? []).compactMap(ConnectedBot.init)
    }

    /// QuickConnect STATUS_TEXT.
    var statusText: String {
        switch status {
        case "ready": return "Ready"
        case "needs_consent": return "Before you connect"
        case "needs_sign_in": return "Waiting for you to sign in"
        case "needs_key": return "Needs your API key"
        case "needs_setup": return "One step left"
        case "blocked": return "Blocked by a verification check"
        default: return status
        }
    }
}

enum VendorConnectAPI {
    /// nil = a computer from before one-step connect (404): nothing to offer.
    static func catalog() async throws -> [ConnectProduct]? {
        do {
            let v = try await ConnectHTTP.call("GET", "gateway/connect")
            return (v.at("products")?.jArray ?? []).compactMap(ConnectProduct.init)
        } catch let e as ConnectError where e.status == 404 {
            return nil
        }
    }

    static func connect(product: String, fields: [String: String], acceptTerms: Bool) async throws -> ConnectResult {
        var body: [String: JSONValue] = ["product": .string(product)]
        if acceptTerms { body["acceptTerms"] = .bool(true) }
        for (k, v) in fields {
            let t = v.trimmingCharacters(in: .whitespacesAndNewlines)
            if !t.isEmpty { body[k] = .string(t) }
        }
        return ConnectResult(try await ConnectHTTP.call("POST", "gateway/connect", body: .object(body)))
    }

    /// QuickConnect `extraFields`: what a product asks for before (re)connecting.
    static func extraFields(product: String, result: ConnectResult?) -> [(key: String, label: String, secret: Bool)] {
        if product == "claude-managed-agents" && result?.status != "ready" {
            return [("apiKey", "Anthropic API key", true)]
        }
        if product == "muse" && result?.status != "ready" {
            return [("agentAddress", "Muse WhatsApp number", false),
                    ("phoneNumberId", "WhatsApp phone number ID", false),
                    ("accessToken", "WhatsApp access token", true),
                    ("appSecret", "Meta app secret", true)]
        }
        return []
    }
}

// MARK: - Channel connect

/// Where a shared-app OAuth flow starts (the cloud's authorize/install link).
struct ChannelConnectStart: Sendable {
    let accountId: String
    let authorizeURL: URL
}

enum ChannelConnectState: Sendable {
    case waiting(adminNeeded: Bool)
    case connected(accountId: String?, workspace: String?, handle: String?)
}

struct PhoneNumberOption: Hashable, Sendable {
    let e164: String
    let detail: String
}

struct BoughtNumber: Sendable {
    let accountId: String
    let e164: String
}

/// registrationFromCloud: approved · rejected · waiting · needs_code · in_review · not_submitted
struct PhoneRegistrationState: Sendable {
    let state: String
    let reason: String?
}

enum ChannelConnectAPI {
    /// The computer the cloud binds a new connection to (`currentRuntimeId`).
    @MainActor
    static func runtimeId() async throws -> String {
        if let id = await RuntimeRelayStore.shared.resolveActiveRuntimeId() { return id }
        throw ConnectError(status: nil, code: "runtime_not_found",
                           message: "No computer is online for your account. Turn your computer on (or connect one), then try again.")
    }

    // Telegram (managed bots)

    static func telegramManagedAvailable() async -> Bool {
        let v = try? await ConnectHTTP.call("GET", "gateway/channel-accounts/telegram/managed/available")
        return v?.at("available")?.jBool ?? false
    }

    @MainActor
    static func createManagedTelegramBot(name: String, botId: String?) async throws -> (id: String, confirmURL: URL) {
        let rt = try await runtimeId()
        var body: [String: JSONValue] = ["runtimeId": .string(rt), "botName": .string(name)]
        if let botId { body["botId"] = .string(botId) }
        let v = try await ConnectHTTP.call("POST", "channel-onboarding/telegram", body: .object(body))
        guard let id = v.at("onboardingId")?.jString, let url = v.at("createUrl")?.jString.flatMap(URL.init(string:)) else {
            throw ConnectError(status: nil, code: nil, message: "Telegram didn’t send a link to confirm. Try again.")
        }
        return (id, url)
    }

    /// awaiting_confirm → nil pair URL; awaiting_pair → pair URL; paired → handle.
    static func managedTelegramStatus(_ id: String) async throws -> (paired: Bool, pairURL: URL?) {
        let v = try await ConnectHTTP.call("GET", "channel-onboarding/telegram/\(ConnectHTTP.escape(id))")
        let state = v.at("state")?.jString ?? ""
        if state == "paired" { return (true, nil) }
        if state == "failed" || state == "expired" {
            throw ConnectError(status: nil, code: state, message: v.at("error")?.jString
                ?? (state == "expired" ? "The Telegram link expired. Start again." : "Telegram couldn’t create the bot. Start again."))
        }
        if state == "connected", let pair = v.at("pairUrl")?.jString.flatMap(URL.init(string:)) { return (false, pair) }
        return (false, nil)
    }

    // Email

    /// nil = Allternit Mail isn't set up here ("coming soon"). With a bot, provisions (or returns) its address.
    static func emailAddress(botId: String) async throws -> (address: String, replyMode: String?, allowlist: [String])? {
        let rail: JSONValue
        do {
            rail = try await ConnectHTTP.call("GET", "agent-email/status")
        } catch let e as ConnectError where e.isUnavailable {
            return nil
        }
        guard rail.at("configured")?.jBool == true else { return nil }
        let made = try await ConnectHTTP.call("POST", "agents/\(ConnectHTTP.escape(botId))/identity/email", body: .object([:]))
        guard let address = made.at("address")?.jString else { return nil }
        let agent = try? await ConnectHTTP.call("GET", "agents/\(ConnectHTTP.escape(botId))")
        let email = agent?.at("identityChannels")?.at("email")
        var mode = email?.at("replyMode")?.jString
        if mode == "auto_known" { mode = "known" }
        let allow = (email?.at("replyAllowlist")?.jArray ?? []).compactMap(\.jString)
        return (address, mode, allow)
    }

    static func saveEmailReplies(botId: String, mode: String, allowlist: [String]) async throws {
        let wire = mode == "known" ? "auto_known" : mode
        _ = try await ConnectHTTP.call("PATCH", "agents/\(ConnectHTTP.escape(botId))", body: .object([
            "identityChannels": .object(["email": .object([
                "replyMode": .string(wire), "replyAllowlist": .array(allowlist.map { .string($0) }),
            ])]),
        ]))
    }

    /// Who may email the bot: "owner" = the owner's account email plus `allowed`.
    static func emailSenders(botId: String) async throws -> (policy: String, allowed: [String], owner: String?) {
        let v = try await ConnectHTTP.call("GET", "agents/\(ConnectHTTP.escape(botId))/identity/email/senders")
        return (v.at("senderPolicy")?.jString ?? "owner", (v.at("allowedSenders")?.jArray ?? []).compactMap(\.jString),
                v.at("ownerAddress")?.jString)
    }

    static func saveEmailSenders(botId: String, policy: String, allowed: [String]) async throws {
        _ = try await ConnectHTTP.call("PUT", "agents/\(ConnectHTTP.escape(botId))/identity/email/senders", body: .object([
            "senderPolicy": .string(policy), "allowedSenders": .array(allowed.map { .string($0) }),
        ]))
    }

    /// connect-api.ts changeEmailAddress: the old mailbox goes first; if the new
    /// name is taken the bot gets its default address back (`restored`).
    static func changeEmailAddress(botId: String, localPart: String) async throws -> String {
        let path = "agents/\(ConnectHTTP.escape(botId))/identity/email"
        _ = try await ConnectHTTP.call("DELETE", path)
        do {
            let made = try await ConnectHTTP.call("POST", path, body: .object(["localPart": .string(localPart)]))
            return made.at("address")?.jString ?? ""
        } catch {
            let restored = (try? await ConnectHTTP.call("POST", path, body: .object([:])))?.at("address")?.jString
            let status = (error as? ConnectError)?.status
            let why = status == 409 ? "That address is taken."
                : status == 400 ? "Use 3–40 letters, numbers or single hyphens." : error.localizedDescription
            throw EmailAddressError(message: restored.map { "\(why) The bot is back on \($0)." }
                ?? "\(why) The bot has no address right now. Close this and open Email again to get one.", restored: restored)
        }
    }

    struct EmailAddressError: LocalizedError {
        let message: String
        let restored: String?
        var errorDescription: String? { message }
    }

    struct DnsRecord: Sendable {
        let type: String
        let host: String
        let value: String
        let purpose: String?
        let ok: Bool?
    }

    struct DomainCheck: Sendable {
        let domain: String
        let verified: Bool
        let records: [DnsRecord]
    }

    private static func domainCheck(_ v: JSONValue, domain: String) -> DomainCheck {
        DomainCheck(
            domain: v.at("domain")?.jString ?? domain,
            verified: v.at("state")?.jString == "verified",
            records: (v.at("records")?.jArray ?? []).map { r in
                DnsRecord(type: r.at("type")?.jString ?? "", host: r.at("host")?.jString ?? "",
                          value: r.at("value")?.jString ?? "", purpose: r.at("purpose")?.jString, ok: r.at("ok")?.jBool)
            })
    }

    /// Add a company domain for bot mail; answers the DNS records to add.
    static func setEmailDomain(_ domain: String) async throws -> DomainCheck {
        var d = domain.trimmingCharacters(in: .whitespaces).lowercased()
        if d.hasSuffix(".") { d.removeLast() }
        do {
            let v = try await ConnectHTTP.call("PUT", "agent-email/domains/\(ConnectHTTP.escape(d))", body: .object([:]))
            return domainCheck(v, domain: d)
        } catch let e as ConnectError {
            let why = e.code == "domain_taken" ? "That domain is already used for bot email on another Allternit account."
                : e.code == "custom_domains_unavailable" || e.status == 503 ? "Company domains aren’t switched on for your account yet. Your bot keeps its shared address."
                : e.status == 400 ? "Enter a domain like acme.com or mail.acme.com." : e.message
            throw ConnectError(status: e.status, code: e.code, message: why)
        }
    }

    /// Live check (each record looked up at public DNS).
    static func emailDomainStatus(_ domain: String) async throws -> DomainCheck {
        let v = try await ConnectHTTP.call("GET", "agent-email/domains?domain=\(ConnectHTTP.escape(domain))")
        let rows = v.at("domains")?.jArray ?? []
        let one = rows.isEmpty ? v : (rows.first { $0.at("domain")?.jString == domain } ?? rows.last ?? v)
        return domainCheck(one, domain: domain)
    }

    /// Move the bot to a verified domain, keeping the part before the @.
    static func moveEmailToDomain(botId: String, address: String, domain: String) async throws -> String {
        let localPart = String(address.split(separator: "@").first ?? "")
        let path = "agents/\(ConnectHTTP.escape(botId))/identity/email"
        _ = try await ConnectHTTP.call("DELETE", path)
        do {
            let made = try await ConnectHTTP.call("POST", path, body: .object(["localPart": .string(localPart), "domain": .string(domain)]))
            return made.at("address")?.jString ?? address
        } catch {
            let restored = (try? await ConnectHTTP.call("POST", path, body: .object(["localPart": .string(localPart)])))?.at("address")?.jString
            throw EmailAddressError(message: restored.map { "\(error.localizedDescription) The bot is back on \($0)." }
                ?? "\(error.localizedDescription) Open Email again to give the bot an address.", restored: restored)
        }
    }

    // Phone numbers

    static func availability() async -> [String: Bool] {
        guard let v = try? await ConnectHTTP.call("GET", "channels/availability"), let o = v.jObject else { return [:] }
        return o.compactMapValues(\.jBool)
    }

    static func searchNumbers(tollFree: Bool, areaCode: String) async throws -> [PhoneNumberOption] {
        var path = "phone/numbers/search?type=\(tollFree ? "toll_free" : "local")&limit=10"
        let area = areaCode.trimmingCharacters(in: .whitespaces)
        if !area.isEmpty { path += "&areaCode=\(ConnectHTTP.escape(area))" }
        let v = try await ConnectHTTP.call("GET", path)
        return (v.at("numbers")?.jArray ?? []).compactMap { n in
            guard let e164 = n.at("e164")?.jString else { return nil }
            let monthly = n.at("monthlyCost")?.jString.map { "$\($0)/mo carrier cost" }
            return PhoneNumberOption(e164: e164, detail: [n.at("locality")?.jString, monthly].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
        }
    }

    @MainActor
    static func buyNumber(e164: String, tollFree: Bool, botId: String) async throws -> BoughtNumber {
        let rt = try await runtimeId()
        let v = try await ConnectHTTP.call("POST", "phone/numbers", body: .object([
            "e164": .string(e164), "runtimeId": .string(rt), "botId": .string(botId),
            "type": .string(tollFree ? "toll_free" : "local"),
        ]))
        guard let id = v.at("number")?.at("id")?.jString else {
            throw ConnectError(status: nil, code: nil, message: "The number was bought but its details didn’t come back. Check Your bots’ phones.")
        }
        return BoughtNumber(accountId: id, e164: v.at("number")?.at("e164")?.jString ?? e164)
    }

    /// Port an existing number in (`POST phone/numbers/port`); answers the number's account id.
    @MainActor
    static func portNumber(e164: String, botId: String) async throws -> String {
        let rt = try await runtimeId()
        let v = try await ConnectHTTP.call("POST", "phone/numbers/port", body: .object([
            "e164": .string(e164), "runtimeId": .string(rt), "botId": .string(botId),
        ]))
        guard let id = v.at("number")?.at("id")?.jString else {
            throw ConnectError(status: nil, code: nil, message: "The port started but its details didn’t come back. Check Your bots’ phones.")
        }
        return id
    }

    enum PortState: Sendable { case pending, done, rejected(String?) }

    /// connect-api.ts portStatus.
    static func portStatus(accountId: String) async throws -> PortState {
        let v = try await ConnectHTTP.call("GET", "phone/numbers/\(ConnectHTTP.escape(accountId))/port")
        let state = (v.at("port")?.at("state")?.jString ?? "").lowercased()
        if ["ported", "completed", "done", "activated"].contains(state) { return .done }
        if ["cancelled", "canceled", "exception", "rejected", "failed"].contains(state) { return .rejected(state) }
        return .pending
    }

    private static func registration(_ v: JSONValue, individual: Bool) -> PhoneRegistrationState {
        let reg = v.at("registration")
        let state = reg?.at("state")?.jString ?? "pending"
        let reason = reg?.at("rejectionReason")?.jString
        if state == "approved" { return .init(state: "approved", reason: nil) }
        if state == "rejected" {
            return .init(state: "rejected", reason: reason.map { $0.replacingOccurrences(of: "campaign: ", with: "") })
        }
        if let reason, reason.hasPrefix("waiting:") {
            return .init(state: "waiting", reason: String(reason.dropFirst("waiting:".count)).trimmingCharacters(in: .whitespaces))
        }
        if individual && (reg?.at("campaignId")?.jString ?? "").isEmpty { return .init(state: "needs_code", reason: nil) }
        return .init(state: "in_review", reason: nil)
    }

    /// cloudRegistrationForm.
    static func submitRegistration(accountId: String, individual: Bool, values: [String: String]) async throws -> PhoneRegistrationState {
        func v(_ k: String) -> String { (values[k] ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
        let name = v("legalName").isEmpty ? [v("firstName"), v("lastName")].filter { !$0.isEmpty }.joined(separator: " ") : v("legalName")
        var samples = v("sampleMessages").split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        if samples.isEmpty {
            var useCase = v("useCase")
            if useCase.hasSuffix(".") { useCase.removeLast() }
            samples = ["\(name): \(useCase). Reply STOP to opt out."]
        }
        var body: [String: JSONValue] = [
            "entityType": .string(individual ? "SOLE_PROPRIETOR" : "PRIVATE_PROFIT"),
            "legalName": .string(name), "displayName": .string(name),
            "website": .string(v("website")),
            "vertical": .string(v("vertical").isEmpty ? "PROFESSIONAL" : v("vertical")),
            "street": .string(v("street")), "city": .string(v("city")), "state": .string(v("state").uppercased()),
            "postalCode": .string(v("postalCode")), "country": .string("US"),
            "contactFirstName": .string(v("firstName")), "contactLastName": .string(v("lastName")),
            "contactEmail": .string(v("email")), "contactPhone": .string(v("phone").isEmpty ? v("mobilePhone") : v("phone")),
            "useCase": .string(individual ? "SOLE_PROPRIETOR" : "CUSTOMER_CARE"),
            "useCaseSummary": .string(v("useCase")),
            "sampleMessages": .array(samples.map { .string($0) }),
        ]
        if individual { body["mobilePhone"] = .string(v("mobilePhone")) } else { body["ein"] = .string(v("ein").filter(\.isNumber)) }
        if !v("optInPageUrl").isEmpty { body["optInPageUrl"] = .string(v("optInPageUrl")) }
        let r = try await ConnectHTTP.call("POST", "phone/numbers/\(ConnectHTTP.escape(accountId))/registration", body: .object(body))
        return registration(r, individual: individual)
    }

    static func registrationStatus(accountId: String, individual: Bool) async throws -> PhoneRegistrationState {
        do {
            return registration(try await ConnectHTTP.call("GET", "phone/numbers/\(ConnectHTTP.escape(accountId))/registration"), individual: individual)
        } catch let e as ConnectError where e.status == 404 {
            return .init(state: "not_submitted", reason: nil)
        }
    }

    /// With a pin, checks it; without, asks the carrier to text a new one.
    static func verifyCode(accountId: String, pin: String?) async throws -> PhoneRegistrationState {
        let body: JSONValue = pin.map { .object(["pin": .string($0)]) } ?? .object([:])
        return registration(try await ConnectHTTP.call("POST", "phone/numbers/\(ConnectHTTP.escape(accountId))/registration/otp", body: body), individual: true)
    }

    // Shared-app OAuth (Slack, Discord, Teams, WhatsApp Business)

    /// nil = the provider's app isn't set up on this deployment ("coming soon").
    @MainActor
    static func start(_ provider: String) async throws -> ChannelConnectStart? {
        do {
            let v: JSONValue
            switch provider {
            case "slack":
                v = try await ConnectHTTP.call("GET", "channels/slack/install")
                if v.at("configured")?.jBool == false { return nil }
            case "discord":
                v = try await ConnectHTTP.call("POST", "channels/discord/install", body: .object(["runtimeId": .string(try await runtimeId())]))
            case "teams":
                v = try await ConnectHTTP.call("POST", "channels/teams/connect", body: .object([:]))
            case "whatsapp":
                guard (await availability())["whatsapp"] == true else { return nil }
                v = try await ConnectHTTP.call("POST", "channels/whatsapp/connect", body: .object(["runtimeId": .string(try await runtimeId())]))
            default:
                return nil
            }
            guard let url = v.at("url")?.jString.flatMap(URL.init(string:)) else { return nil }
            return ChannelConnectStart(accountId: provider, authorizeURL: url)
        } catch let e as ConnectError where e.isUnavailable {
            return nil
        }
    }

    /// After the browser step lands on the cloud, record the connection on the computer (idempotent).
    @MainActor
    static func status(_ provider: String) async throws -> ChannelConnectState {
        var body: [String: JSONValue] = [:]
        if provider != "discord", let rt = await RuntimeRelayStore.shared.resolveActiveRuntimeId() { body["runtimeId"] = .string(rt) }
        do {
            let v = try await ConnectHTTP.call("POST", "gateway/channel-accounts/\(provider)", body: .object(body))
            let a = v.at("account")
            return .connected(accountId: a?.at("id")?.jString, workspace: a?.at("displayName")?.jString, handle: a?.at("handle")?.jString)
        } catch let e as ConnectError where e.status == 404 {
            return .waiting(adminNeeded: false)
        }
    }

    static func discordChannels(guildId: String) async throws -> [(id: String, name: String, selected: Bool)] {
        let v = try await ConnectHTTP.call("GET", "channels/discord/installs/\(ConnectHTTP.escape(guildId))/channels")
        return (v.at("channels")?.jArray ?? []).compactMap { c in
            guard let id = c.at("id")?.jString else { return nil }
            return (id, c.at("name")?.jString ?? id, c.at("selected")?.jBool ?? true)
        }
    }

    /// An empty list means every channel (also ones made later).
    static func saveDiscordChannels(guildId: String, ids: [String]) async throws {
        _ = try await ConnectHTTP.call("PUT", "channels/discord/installs/\(ConnectHTTP.escape(guildId))/channels",
                                       body: .object(["channelIds": .array(ids.map { .string($0) })]))
    }
}
