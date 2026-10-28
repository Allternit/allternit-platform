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
        let userInfo = response.notification.request.content.userInfo
        let urlPath = userInfo["url"] as? String
        let event = (userInfo["data"] as? [String: Any])?["event"] as? String
        await MainActor.run {
            PushNotificationManager.shared.handleTap(urlPath: urlPath, event: event)
        }
    }
}
