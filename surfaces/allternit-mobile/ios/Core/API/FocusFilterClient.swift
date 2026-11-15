import Foundation
import UIKit

/// `PUT {cloud}/api/v1/notifications/focus` — tells the cloud which bots may
/// notify loudly on this device while a Focus is on. Other pushes still go out,
/// as `passive`. The server forgets the filter 24 hours after the last confirm,
/// so the app re-sends the stored filter on foreground and background refresh.
struct FocusFilterClient: Sendable {
    /// The current device id (same value the push-token registration sends).
    /// `UIDevice` is main-actor only, so callers hop to the main actor.
    @MainActor static func currentDeviceId() -> String {
        UIDevice.current.identifierForVendor?.uuidString ?? ""
    }

    static func body(_ request: FocusFilterRequest) throws -> Data {
        try JSONEncoder().encode(request)
    }

    func send(_ request: FocusFilterRequest) async throws {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/notifications/focus")
        var req = try await APIClient.shared.authorizedRequest(url: url, method: "PUT")
        req.httpBody = try Self.body(request)
        let (data, response) = try await URLSession.shared.data(for: req)
        if (response as? HTTPURLResponse)?.statusCode == 404 {
            // `unknown_device`: this phone has not registered a push token yet.
            throw IntentFailure.failed("This phone is not set up for notifications yet.")
        }
        try APIClient.shared.validate(response, data: data)
        FocusFilterStore.save(request)
    }

    /// Re-sends the stored filter when it is active and older than `minAge`.
    /// Called on foreground and from `BackgroundRefreshManager`.
    static func reconfirmIfNeeded(minAge: TimeInterval = 60 * 60) async {
        guard let saved = FocusFilterStore.load(), saved.request.active,
              Date().timeIntervalSince(saved.at) >= minAge else { return }
        var request = saved.request
        request.deviceId = await currentDeviceId()
        try? await FocusFilterClient().send(request)
    }
}

/// The last confirmed filter, so a re-confirm needs no Focus callback.
enum FocusFilterStore {
    private static let key = "allternit-focus-filter-last"
    struct Saved: Codable { var request: FocusFilterRequest; var at: Date }

    static func save(_ request: FocusFilterRequest) {
        guard let data = try? JSONEncoder().encode(Saved(request: request, at: Date())) else { return }
        UserDefaults.standard.set(data, forKey: key)
    }

    static func load() -> Saved? {
        guard let data = UserDefaults.standard.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(Saved.self, from: data)
    }
}
