import Foundation
import BackgroundTasks
@preconcurrency import UserNotifications

/// Gap 2 mitigation (background stream liveness): the pty/permission SSE
/// streams are torn down when the app backgrounds — `ChatViewModel
/// .handleScenePhase` cancels the chat stream, and `CodeModeView`'s
/// `pollPendingPermissions()` loop is a `.task` bound to that view's
/// lifecycle, so it stops the moment the view disappears. There's no
/// realistic way to keep a live socket open while suspended (iOS doesn't
/// grant that background mode to a plain client app), so this is the
/// pragmatic middle ground recommended in the plan: a periodic
/// `BGAppRefreshTask` polls the permission queue while backgrounded and
/// surfaces anything new as a local notification, so an approval request
/// doesn't sit unseen until the user happens to reopen the app.
///
/// True push-on-server-event (Gap 3) would remove the need for polling
/// entirely, but that needs backend device-token plumbing this client can't
/// add alone — see `docs/TODO.md`.
enum BackgroundRefreshManager {
    static let taskIdentifier = "com.allternit.mobile.refresh"

    private static let lastSeenPermissionIDsKey = "allternit-bg-refresh-seen-permission-ids"

    /// Must run synchronously during app launch, before the scene connects
    /// — `BGTaskScheduler` requires the handler registered before
    /// `application(_:didFinishLaunchingWithOptions:)` returns (called from
    /// `AllternitApp.init()` here, SwiftUI's equivalent launch point).
    static func register() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: taskIdentifier, using: nil) { task in
            guard let refreshTask = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            handle(refreshTask)
        }
    }

    /// Submitted every time the app backgrounds (`AllternitApp`'s
    /// `scenePhase` handler). `earliestBeginDate` is only a floor — iOS
    /// decides the actual firing time based on usage patterns and battery
    /// state, and may not run it at all under tight budgets.
    static func scheduleNextRefresh() {
        let request = BGAppRefreshTaskRequest(identifier: taskIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// Sendable box for handing the (non-Sendable) `BGAppRefreshTask` to the
    /// detached `Task` below — same pattern as `MeshClient.NodeBox` for its
    /// (non-Sendable) `MeshNode`. `BGAppRefreshTask` isn't marked `Sendable`
    /// by the SDK, but Apple's contract for it (`setTaskCompleted`/
    /// `expirationHandler`) is thread-safe by design.
    private struct TaskBox: @unchecked Sendable {
        let task: BGAppRefreshTask
    }

    private static func handle(_ task: BGAppRefreshTask) {
        // Keep the chain going regardless of this run's outcome — otherwise
        // a single skipped/expired run would end background refresh for
        // the rest of the session.
        scheduleNextRefresh()

        let box = TaskBox(task: task)
        let work = Task {
            await pollForNewPermissions()
            box.task.setTaskCompleted(success: true)
        }
        box.task.expirationHandler = {
            work.cancel()
        }
    }

    /// Diffs the pending-permission list against what was last seen (a
    /// UserDefaults id set, since there's no push channel to dedupe
    /// server-side) and posts one local notification per newly-seen
    /// request. Silently no-ops without notification authorization or
    /// network reachability — this is best-effort, not a guarantee.
    private static func pollForNewPermissions() async {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        guard settings.authorizationStatus == .authorized else { return }

        guard let pending = try? await PermissionClient.shared.listPending(), !pending.isEmpty else { return }

        let defaults = UserDefaults.standard
        let seenIDs = Set(defaults.stringArray(forKey: lastSeenPermissionIDsKey) ?? [])
        let newRequests = pending.filter { !seenIDs.contains($0.id) }

        for request in newRequests {
            await NotificationService.postPermissionRequestNotification(for: request)
        }

        // Persist the full current set (not just the new ones) so a
        // request that gets replied to and later re-appears with the same
        // id — shouldn't happen, but the queue is server state we don't
        // control — doesn't re-notify every cycle.
        defaults.set(pending.map(\.id), forKey: lastSeenPermissionIDsKey)
    }
}
