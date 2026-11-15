import Foundation
@preconcurrency import UserNotifications

/// Local response notifications (Phase 5).
///
/// When the user opts in (empty-chat card → PermissionPrimingSheet → system
/// dialog), a completed stream posts a LOCAL notification if the app isn't
/// active at completion time. This covers the step-away-while-streaming
/// case where the process is still alive (the SSE connection itself is
/// cancelled on `.background` — see ChatViewModel.handleScenePhase).
///
/// Server push (APNs) — a bot answering after the app was suspended or
/// closed — is `PushNotificationManager` + cloud-api routes/apns.rs; the
/// authorization granted here also registers this phone for it.
enum NotificationService {
    /// System authorization, requested only after the app-owned priming
    /// sheet (AppPermission.notifications) got a Continue.
    static func requestAuthorization() async -> Bool {
        do {
            let granted = try await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .sound, .badge])
            if granted {
                // Gap 3 — same consent moment, no separate prompt. See
                // PushNotificationManager for why this can't be verified
                // end-to-end on this machine (no signing team, no backend
                // device-token endpoint).
                await PushNotificationManager.shared.registerForRemoteNotifications()
            }
            return granted
        } catch {
            return false
        }
    }

    /// Posts a local "Allternit responded" notification when the user has
    /// granted authorization; a no-op otherwise. Called on stream
    /// completion when the app isn't active (the caller checks app state).
    static func postResponseNotification(preview: String) async {
        // Settings → Notifications → Response completions.
        guard NotificationPrefs.isOn(NotificationPrefs.responseKey) else { return }
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        guard settings.authorizationStatus == .authorized else { return }

        let content = UNMutableNotificationContent()
        content.title = "Allternit responded"
        content.body = preview.isEmpty ? "Your response is ready." : preview
        if NotificationPrefs.isOn(NotificationPrefs.soundKey) { content.sound = .default }

        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil // deliver immediately
        )
        try? await center.add(request)
    }

    /// Posts a local "approval needed" notification for a permission
    /// request discovered by `BackgroundRefreshManager`'s background poll.
    /// Reuses the same `.notifications` authorization as
    /// `postResponseNotification` — no separate opt-in.
    static func postPermissionRequestNotification(for request: PermissionRequest) async {
        // Settings → Notifications → Approval requests.
        guard NotificationPrefs.isOn(NotificationPrefs.approvalKey) else { return }
        let content = UNMutableNotificationContent()
        content.title = "Approval needed"
        content.body = "\(request.permission.capitalized) request is waiting on you."
        if NotificationPrefs.isOn(NotificationPrefs.soundKey) { content.sound = .default }

        let notificationRequest = UNNotificationRequest(
            identifier: "permission-\(request.id)",
            content: content,
            trigger: nil // deliver immediately
        )
        try? await UNUserNotificationCenter.current().add(notificationRequest)
    }
}
