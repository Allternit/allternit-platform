import ActivityKit
import Foundation

/// Starts/updates/ends the Lock Screen + Dynamic Island Live Activity for a
/// running `Loop`, mirroring the same iteration-budget data
/// `LoopStaminaRing` renders in-app. One Activity per loop id at a time —
/// `activities` tracks them so `LoopStore` can call `sync(with:)` after
/// every fetch/refresh without needing to know which loops already have one.
///
/// The app starts loop activities (cloud-api has no push-to-start for
/// loops). They are requested with `pushType: .token`, and each update token
/// goes to the cloud with `activityKey = loopId` (when the Live Activities
/// switch is on), so cloud-api can update and end them from
/// `snapshot.loops` while the app is suspended (`routes/live_activity.rs`).
@MainActor
final class LoopLiveActivityManager {
    static let shared = LoopLiveActivityManager()

    /// cloud-api `LOOP_ATTRIBUTES`.
    nonisolated static let attributesType = "LoopActivityAttributes"

    private var activities: [String: Activity<LoopActivityAttributes>] = [:]
    private var observed: Set<String> = []
    private var installed = false

    private init() {}

    /// Launch (`BotLiveActivityManager.installAtLaunch()`): adopt loop
    /// activities that outlived the last process and keep their tokens current.
    func startObserving() {
        guard !installed else { return }
        installed = true
        for existing in Activity<LoopActivityAttributes>.activities
        where existing.activityState == .active || existing.activityState == .stale {
            activities[existing.attributes.loopId] = existing
            observe(activityId: existing.id, loopId: existing.attributes.loopId)
        }
    }

    private func observe(activityId: String, loopId: String) {
        guard observed.insert(activityId).inserted else { return }
        Task { await Self.watchTokens(activityId: activityId, loopId: loopId) }
        Task { await Self.watchState(activityId: activityId, loopId: loopId) }
    }

    private func didEnd(activityId: String, loopId: String) async {
        observed.remove(activityId)
        if activities[loopId]?.id == activityId { activities.removeValue(forKey: loopId) }
        await ActivityTokenRegistry.shared.remove(kind: .laUpdate, attributesType: Self.attributesType, activityKey: loopId)
    }

    private nonisolated static func watchTokens(activityId: String, loopId: String) async {
        guard let tracked = live(activityId) else { return }
        for await token in tracked.pushTokenUpdates {
            await ActivityTokenRegistry.shared.update(kind: .laUpdate, attributesType: attributesType, activityKey: loopId, token: token)
        }
    }

    private nonisolated static func watchState(activityId: String, loopId: String) async {
        guard let tracked = live(activityId) else { return }
        for await state in tracked.activityStateUpdates where state == .ended || state == .dismissed {
            await LoopLiveActivityManager.shared.didEnd(activityId: activityId, loopId: loopId)
            return
        }
    }

    /// Sign-out (`BotLiveActivityManager.willSignOut()`): end every loop activity now.
    func endAllNow() {
        activities.removeAll()
        let ids = Activity<LoopActivityAttributes>.activities.map(\.id)
        Task {
            for id in ids {
                await Self.live(id)?.end(nil, dismissalPolicy: .immediate)
            }
        }
    }

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
        let content = ActivityContent(state: Self.contentState(for: loop), staleDate: nil)
        let activity: Activity<LoopActivityAttributes>
        do {
            activity = try Activity.request(attributes: attributes, content: content, pushType: .token)
        } catch {
            // No push entitlement (unsigned or simulator builds): fall back
            // to a local-only activity.
            do {
                activity = try Activity.request(attributes: attributes, content: content)
            } catch {
                // Simulator/device without Live Activities enabled, or the
                // per-app concurrent-activity limit — nothing actionable here.
                return
            }
        }
        activities[loop.id] = activity
        observe(activityId: activity.id, loopId: loop.id)
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
