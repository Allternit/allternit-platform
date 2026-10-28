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
    /// look at another screen is worth seeing).
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let urlPath = response.notification.request.content.userInfo["url"] as? String
        await MainActor.run {
            PushNotificationManager.shared.handleTap(urlPath: urlPath)
        }
    }
}
