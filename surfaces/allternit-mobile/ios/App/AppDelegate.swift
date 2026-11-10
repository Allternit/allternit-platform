import UIKit
import UserNotifications

/// UIKit callbacks SwiftUI doesn't expose (`@UIApplicationDelegateAdaptor`
/// in AllternitApp): APNs registration and notification taps. See
/// `PushNotificationManager` for the whole push flow.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        // Set before launch finishes so a cold launch from a tap is delivered.
        UNUserNotificationCenter.current().delegate = self
        // Buttons on Allternit notifications (identifiers fixed by the contract).
        NotificationCategories.register()
        // App Intents call into the app through this (SystemSurfaces/Intents).
        IntentServiceRegistry.register(IntentServices())
        // Extensions cannot import AppConfig; they read the cloud URL here.
        AppGroupStore.cloudBaseURL = AppConfig.cloudAPIBaseURL
        // Every foreground: remove delivered notifications whose item is gone.
        NotificationCenter.default.addObserver(
            forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { _ in
            Task { @MainActor in
                guard AuthManager.shared.isSignedIn else { return }
                await NotificationActions.reconcileDelivered()
                // Send anything the share extension left behind.
                await ShareOutboxDrainer.shared.drain()
            }
        }
        // Live Activities: intent registry, adopt running activities, token uploads.
        Task { @MainActor in BotLiveActivityManager.installAtLaunch() }
        Task { @MainActor in
            await PushNotificationManager.shared.registerIfAuthorized()
        }
        return true
    }

    func application(
        _ application: UIApplication,
        didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
    ) {
        Task { @MainActor in
            PushNotificationManager.shared.didRegister(deviceToken: deviceToken)
        }
    }

    func application(
        _ application: UIApplication,
        didFailToRegisterForRemoteNotificationsWithError error: Error
    ) {
        let message = error.localizedDescription
        Task { @MainActor in
            PushNotificationManager.shared.didFailToRegister(error: NSError(
                domain: "APNs", code: 0, userInfo: [NSLocalizedDescriptionKey: message]
            ))
        }
    }

    /// Background pushes. `notification.revoke` removes the delivered
    /// notifications for items answered elsewhere.
    func application(
        _ application: UIApplication,
        didReceiveRemoteNotification userInfo: [AnyHashable: Any]
    ) async -> UIBackgroundFetchResult {
        guard let targets = NotificationActions.revokeTargets(userInfo: userInfo) else { return .noData }
        await NotificationActions.removeDelivered(inboxIds: targets.inboxIds, tags: targets.tags)
        return .newData
    }

    // MARK: - UNUserNotificationCenterDelegate

    /// In the foreground, still show the banner (a bot answering while you
    /// look at another screen is worth seeing). A ring is the exception: the
    /// incoming-call screen takes it (InappCallStore), so no banner.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        let userInfo = notification.request.content.userInfo
        let type = userInfo["type"] as? String
        let room = (userInfo["data"] as? [String: Any])?["room"] as? String
        let handled = await MainActor.run {
            InappCallStore.shared.handlesForegroundPush(type: type, room: room)
        }
        return handled ? [] : [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        // Approve / Deny / Reply / Retry / Snooze run in the background.
        if let action = NotificationActionID(rawValue: response.actionIdentifier),
           action != .open, action != .review {
            await NotificationActions.handle(response: response)
            return
        }
        guard response.actionIdentifier != UNNotificationDismissActionIdentifier else { return }
        let userInfo = response.notification.request.content.userInfo
        let urlPath = userInfo["url"] as? String
        let data = userInfo["data"] as? [String: Any]
        let event = data?["event"] as? String
        let inboxId = data?["inboxId"] as? String
        await MainActor.run {
            PushNotificationManager.shared.handleTap(urlPath: urlPath, event: event, inboxId: inboxId)
        }
    }
}
