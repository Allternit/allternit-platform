import Foundation

/// Cloud Voice: tickets and minutes from cloud-api (`AppConfig.cloudAPIBaseURL`),
/// authenticated with the user's Clerk token the same way every other
/// cloud-api call in the app is.
struct VoiceTicket: Equatable, Sendable {
    let ticket: String
    let wsURL: URL
    /// `wsUrl?ticket=<ticket>` — what the client connects to.
    var connectURL: URL {
        var components = URLComponents(url: wsURL, resolvingAgainstBaseURL: false) ?? URLComponents()
        var items = components.queryItems ?? []
        items.removeAll { $0.name == "ticket" }
        items.append(URLQueryItem(name: "ticket", value: ticket))
        components.queryItems = items
        return components.url ?? wsURL
    }
}

/// This month's Cloud Voice usage (`GET /api/v1/voice/usage/month`).
struct VoiceMonthUsage: Equatable, Sendable {
    let usedSeconds: Int
    let includedSeconds: Int
    let overageRateUsdPerMin: Double?

    var remainingSeconds: Int { max(0, includedSeconds - usedSeconds) }
    var usedMinutes: Int { usedSeconds / 60 }
    var includedMinutes: Int { includedSeconds / 60 }
}

/// Why Cloud Voice can't start. `message` is user-facing.
enum VoiceCloudError: Error, Equatable, LocalizedError {
    /// 402 `no-cloud-minutes`: the plan has no Cloud Voice minutes left.
    case noCloudMinutes(String)
    /// 503 `cloud-unavailable`: the service isn't reachable/configured.
    case cloudUnavailable(String)
    case signedOut
    case other(String)

    var errorDescription: String? {
        switch self {
        case .noCloudMinutes(let message), .cloudUnavailable(let message), .other(let message):
            return message
        case .signedOut:
            return "Sign in to use Cloud Voice."
        }
    }

    /// Maps a cloud-api error response (`{code, message}`) to the typed error.
    static func from(status: Int, body: Data?) -> VoiceCloudError {
        let object = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let code = object?["code"] as? String
        let message = object?["message"] as? String
        switch (status, code) {
        case (402, _), (_, "no-cloud-minutes"?):
            return .noCloudMinutes(message ?? "Your plan has no Cloud Voice minutes. Upgrade to use Cloud Voice.")
        case (503, _), (_, "cloud-unavailable"?):
            return .cloudUnavailable(message ?? "Cloud Voice is not available right now.")
        case (401, _):
            return .signedOut
        default:
            return .other(message ?? "Cloud Voice failed (HTTP \(status)).")
        }
    }

    static func parseTicket(_ data: Data) -> VoiceTicket? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ticket = object["ticket"] as? String, !ticket.isEmpty,
              let ws = object["wsUrl"] as? String, let url = URL(string: ws) else { return nil }
        return VoiceTicket(ticket: ticket, wsURL: url)
    }

    static func parseUsage(_ data: Data) -> VoiceMonthUsage? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        func int(_ key: String) -> Int? {
            if let value = object[key] as? Int { return value }
            if let value = object[key] as? Double { return Int(value) }
            return nil
        }
        guard let used = int("usedSeconds"), let included = int("includedSeconds") else { return nil }
        return VoiceMonthUsage(usedSeconds: used, includedSeconds: included,
                               overageRateUsdPerMin: object["overageRateUsdPerMin"] as? Double)
    }
}

final class VoiceCloudClient: @unchecked Sendable {
    static let shared = VoiceCloudClient()

    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = AppConfig.cloudAPIBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// `POST /api/v1/voice/tickets` → a 60 s single-use ticket + the voice
    /// service's WebSocket URL.
    func mintTicket() async throws -> VoiceTicket {
        let url = baseURL.appendingPathComponent("api/v1/voice/tickets")
        let (data, status) = try await perform(url: url, method: "POST")
        guard (200...299).contains(status) else { throw VoiceCloudError.from(status: status, body: data) }
        guard let ticket = VoiceCloudError.parseTicket(data) else {
            throw VoiceCloudError.other("Cloud Voice returned an unreadable ticket.")
        }
        return ticket
    }

    /// `GET /api/v1/voice/usage/month`.
    func monthUsage() async throws -> VoiceMonthUsage {
        let url = baseURL.appendingPathComponent("api/v1/voice/usage/month")
        let (data, status) = try await perform(url: url, method: "GET")
        guard (200...299).contains(status) else { throw VoiceCloudError.from(status: status, body: data) }
        guard let usage = VoiceCloudError.parseUsage(data) else {
            throw VoiceCloudError.other("Cloud Voice usage was unreadable.")
        }
        return usage
    }

    private func perform(url: URL, method: String) async throws -> (Data, Int) {
        let request: URLRequest
        do {
            request = try await APIClient.shared.authorizedRequest(url: url, method: method)
        } catch {
            throw VoiceCloudError.signedOut
        }
        if request.value(forHTTPHeaderField: "Authorization") == nil {
            #if DEBUG
            // `-skip-auth` dev shim sets other headers; let the server decide.
            #else
            throw VoiceCloudError.signedOut
            #endif
        }
        do {
            let (data, response) = try await session.data(for: request)
            return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
        } catch {
            throw VoiceCloudError.cloudUnavailable("Couldn't reach Cloud Voice. Check your connection.")
        }
    }
}
