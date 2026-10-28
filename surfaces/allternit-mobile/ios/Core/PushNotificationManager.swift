import UIKit
@preconcurrency import UserNotifications

/// Native push (APNs) for the iPhone app.
///
/// Flow:
/// - **Permission** is asked at a moment with context, never on first
///   launch: the "Meet your bots" intro's notifications step, or the
///   empty-chat priming card (`NotificationService.requestAuthorization`).
/// - **Registration**: once authorized, every launch and every sign-in
///   calls `registerForRemoteNotifications()`; the token goes to cloud-api
///   `POST /api/v1/device-tokens` (`PushTokenClient`). Sign-out removes it.
/// - **Delivery**: cloud-api sends every push it sends to the web (bot
///   messages, approvals, runs done, calls, quiet-hours summaries) to APNs
///   too, under the same notification preferences (routes/apns.rs).
/// - **Tap**: the payload's `url` (the same app path the web opens) goes to
///   `DeepLinkRouter` → bot chat, bot page, Settings, Computers.
/// - **Badge**: the server sets 1 ("something new"); opening the app clears it.
///
/// Needs the Push Notifications capability (`aps-environment` entitlement)
/// and a provisioned build; on unsigned simulator builds registration fails
/// and the failure is kept in `lastRegistrationError`.
@MainActor
final class PushNotificationManager: NSObject {
    static let shared = PushNotificationManager()

    private static let deviceTokenDefaultsKey = "allternit-apns-device-token"
    private static let registrationErrorDefaultsKey = "allternit-apns-registration-error"
    private static let registeredForUserKey = "allternit-apns-registered"

    private(set) var lastDeviceTokenHex: String? {
        get { UserDefaults.standard.string(forKey: Self.deviceTokenDefaultsKey) }
        set { UserDefaults.standard.set(newValue, forKey: Self.deviceTokenDefaultsKey) }
    }

    /// Last registration failure (shown in Settings → Notifications when
    /// push can't be set up). Cleared on a successful registration.
    private(set) var lastRegistrationError: String? {
        get { UserDefaults.standard.string(forKey: Self.registrationErrorDefaultsKey) }
        set { UserDefaults.standard.set(newValue, forKey: Self.registrationErrorDefaultsKey) }
    }

    private override init() {}

    // MARK: - Permission + registration

    /// Current system authorization.
    func authorizationStatus() async -> UNAuthorizationStatus {
        await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    /// Asks for permission (system dialog) and registers when granted.
    /// Call only from a screen that explains why (intro step / priming card).
    @discardableResult
    func requestPermissionAndRegister() async -> Bool {
        await NotificationService.requestAuthorization()
    }

    /// Re-registers without prompting when the user already allowed
    /// notifications — app launch and sign-in. Never shows a dialog.
    func registerIfAuthorized() async {
        let status = await authorizationStatus()
        guard status == .authorized || status == .provisional || status == .ephemeral else { return }
        registerForRemoteNotifications()
    }

    /// Called after the user grants authorization (NotificationService).
    func registerForRemoteNotifications() {
        UIApplication.shared.registerForRemoteNotifications()
    }

    /// `AppDelegate.application(_:didRegisterForRemoteNotificationsWithDeviceToken:)`.
    func didRegister(deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        lastDeviceTokenHex = hex
        Task { await sendTokenToServer(hex) }
    }

    /// `AppDelegate.application(_:didFailToRegisterForRemoteNotificationsWithError:)`.
    /// Expected on unsigned builds (no signing team / push capability).
    func didFailToRegister(error: Error) {
        lastRegistrationError = error.localizedDescription
    }

    private func sendTokenToServer(_ hex: String) async {
        // Signed out: keep the token; sign-in re-registers (AuthManager).
        guard AuthManager.shared.isSignedIn else { return }
        do {
            try await PushTokenClient.shared.register(
                token: hex,
                deviceId: UIDevice.current.identifierForVendor?.uuidString
            )
            lastRegistrationError = nil
            UserDefaults.standard.set(true, forKey: Self.registeredForUserKey)
        } catch {
            lastRegistrationError = error.localizedDescription
        }
    }

    /// Sign-in: send the stored token for the new account (or fetch one).
    func didSignIn() async {
        await registerIfAuthorized()
        if let hex = lastDeviceTokenHex {
            await sendTokenToServer(hex)
        }
    }

    /// Sign-out: stop pushes to this phone for the account leaving. Runs
    /// before the session is dropped (the DELETE needs its Bearer).
    func willSignOut() async {
        guard UserDefaults.standard.bool(forKey: Self.registeredForUserKey),
              let hex = lastDeviceTokenHex else { return }
        try? await PushTokenClient.shared.unregister(token: hex)
        UserDefaults.standard.set(false, forKey: Self.registeredForUserKey)
        clearBadge()
    }

    // MARK: - Taps + badge

    /// A notification was tapped: open what it points at.
    func handleTap(urlPath: String?) {
        clearBadge()
        guard let urlPath, !urlPath.isEmpty else { return }
        DeepLinkRouter.shared.open(path: urlPath)
    }

    /// Opening the app counts as seeing what's new.
    func clearBadge() {
        Task { try? await UNUserNotificationCenter.current().setBadgeCount(0) }
    }
}
