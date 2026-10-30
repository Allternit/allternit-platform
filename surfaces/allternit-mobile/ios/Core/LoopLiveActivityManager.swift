import ActivityKit
import Foundation

/// Starts/updates/ends the Lock Screen + Dynamic Island Live Activity for a
/// running `Loop`, mirroring the same iteration-budget data
/// `LoopStaminaRing` renders in-app. One Activity per loop id at a time —
/// `activities` tracks them so `LoopStore` can call `sync(with:)` after
/// every fetch/refresh without needing to know which loops already have one.
///
/// This app-side half is genuinely end-to-end functional (unlike Gap 3):
/// local (non-push) Live Activities need no signing team or backend, only
/// `NSSupportsLiveActivities` in Info.plist and iOS 16.1+.
@MainActor
final class LoopLiveActivityManager {
    static let shared = LoopLiveActivityManager()

    private var activities: [String: Activity<LoopActivityAttributes>] = [:]

    private init() {}

    /// Call after every `LoopStore` fetch/refresh with the current loop
    /// list: starts an Activity for newly-running loops, updates ones
    /// already tracked, and ends ones that reached a terminal state or
    /// dropped out of the list (deleted).
    func sync(with loops: [Loop]) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }

        let loopsByID = Dictionary(uniqueKeysWithValues: loops.map { ($0.id, $0) })

        for loop in loops where loop.state == "running" {
            if let activity = activities[loop.id] {
                let id = activity.id, state = Self.contentState(for: loop)
                Task { await Self.live(id)?.update(ActivityContent(state: state, staleDate: nil)) }
            } else {
                start(for: loop)
            }
        }

        for (loopId, activity) in activities {
            guard let loop = loopsByID[loopId] else {
                end(loopId: loopId, activity: activity, finalState: nil)
                continue
            }
            if loop.state != "running" {
                end(loopId: loopId, activity: activity, finalState: Self.contentState(for: loop))
            }
        }
    }

    private func start(for loop: Loop) {
        let attributes = LoopActivityAttributes(loopId: loop.id, command: loop.command)
        do {
            let activity = try Activity.request(
                attributes: attributes,
                content: .init(state: Self.contentState(for: loop), staleDate: nil)
            )
            activities[loop.id] = activity
        } catch {
            // Simulator/device without Live Activities enabled, or the
            // per-app concurrent-activity limit — nothing actionable here.
        }
    }

    private func end(loopId: String, activity: Activity<LoopActivityAttributes>, finalState: LoopActivityAttributes.ContentState?) {
        activities.removeValue(forKey: loopId)
        let id = activity.id
        Task {
            await Self.live(id)?.end(
                finalState.map { ActivityContent(state: $0, staleDate: nil) },
                dismissalPolicy: .after(.now.addingTimeInterval(30))
            )
        }
    }

    /// Activity isn't Sendable, so tasks carry its id and look it up here.
    private nonisolated static func live(_ id: String) -> Activity<LoopActivityAttributes>? {
        Activity<LoopActivityAttributes>.activities.first { $0.id == id }
    }

    static func contentState(for loop: Loop) -> LoopActivityAttributes.ContentState {
        LoopActivityAttributes.ContentState(
            iterationsCompleted: loop.iterationLog.count,
            maxIterations: loop.maxIterations,
            state: loop.state
        )
    }
}
