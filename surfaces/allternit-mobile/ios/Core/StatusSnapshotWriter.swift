import Foundation
import UIKit
import WidgetKit

/// App side of the status widget: writes the latest `status.snapshot` (and a
/// few flags) into the App Group, then asks WidgetKit for fresh timelines.
/// Call it when the app learns a new snapshot (a `status.snapshot` event), after
/// an approve or deny, and when the app comes to the foreground.
enum StatusSnapshotWriter {
    /// Stores `snapshot` for the widget and reloads its timelines.
    @MainActor
    static func write(_ snapshot: StatusSnapshot, signedIn: Bool = true, hasComputer: Bool = true) {
        AppGroupStore.signedIn = signedIn
        AppGroupStore.hasComputer = hasComputer
        AppGroupStore.saveSnapshot(snapshot)
        syncReduceMotion()
        WidgetCenter.shared.reloadTimelines(ofKind: StatusWidgetKind.id)
    }

    /// Fetches `GET /api/v1/status/summary` and writes the result. Returns
    /// false when nothing was stored (signed out, offline, or an HTTP error).
    @MainActor
    @discardableResult
    static func refreshFromCloud() async -> Bool {
        switch await StatusSummaryClient().fetch() {
        case .success(let snapshot):
            write(snapshot)
            return true
        case .failure:
            syncReduceMotion()
            return false
        }
    }

    /// Sign-out: drops the stored snapshot and flags, then reloads the widget
    /// so it shows "Sign in to Allternit".
    @MainActor
    static func clear() {
        AppGroupStore.clearAll()
        WidgetCenter.shared.reloadTimelines(ofKind: StatusWidgetKind.id)
    }

    /// The widget reads this when its environment does not carry Reduce Motion.
    @MainActor
    static func syncReduceMotion() {
        WidgetFlags.reduceMotion = UIAccessibility.isReduceMotionEnabled
    }
}
