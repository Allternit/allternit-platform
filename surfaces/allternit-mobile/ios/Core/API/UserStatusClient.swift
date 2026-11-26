import Foundation

/// The owner's presence: online, autopilot, do not disturb or away.
/// `GET/PUT /api/v1/me/status` (cloud-api, one document in account_prefs).
/// Same four rows and sub-lines as the web status menu.
enum UserStatus: String, CaseIterable, Identifiable, Sendable {
    case online, autopilot, dnd, away

    var id: String { rawValue }

    var title: String {
        switch self {
        case .online: return "Online"
        case .autopilot: return "Autopilot"
        case .dnd: return "Do not disturb"
        case .away: return "Away"
        }
    }

    var subline: String {
        switch self {
        case .online: return ""
        case .autopilot: return "Non-urgent asks wait. Gizzi keeps working."
        case .dnd: return "Non-urgent asks wait"
        case .away: return "A summary waits for you"
        }
    }
}

final class UserStatusClient: @unchecked Sendable {
    static let shared = UserStatusClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    private struct Body: Codable {
        let status: String
        let since: String?
    }

    /// Nothing saved reads as online.
    func get() async throws -> UserStatus {
        let request = try await client.authorizedRequest(path: "me/status")
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        do {
            let body = try JSONDecoder().decode(Body.self, from: data)
            return UserStatus(rawValue: body.status) ?? .online
        } catch {
            throw APIError.decoding(error)
        }
    }

    @discardableResult
    func set(_ status: UserStatus) async throws -> UserStatus {
        var request = try await client.authorizedRequest(path: "me/status", method: "PUT")
        request.httpBody = try JSONEncoder().encode(["status": status.rawValue])
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        return status
    }
}
