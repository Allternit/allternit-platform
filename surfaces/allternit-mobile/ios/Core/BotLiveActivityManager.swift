import ActivityKit
import Foundation
import OSLog

/// Starts/updates/ends a Lock Screen + Dynamic Island Live Activity that
/// summarizes the dominant operational state across all subscribed bots.
///
/// One activity is shown at a time. Two writers keep it current:
/// - this process, from `BotStatusStore`'s SSE fold (`sync(with:)`) while
///   the app runs;
/// - cloud-api, by APNs Live Activity pushes while the app is suspended
///   (`routes/live_activity.rs`). Activities are requested with
///   `pushType: .token`; their update tokens, and the push-to-start token
///   on iOS 17.2+, go to the cloud through `ActivityTokenRegistry` when the
///   device-local Live Activities switch is on.
///
/// Option A, bot first: the server fills the focus bot (name, step,
/// progress, "+N others") and, when that bot needs the owner, the question.
/// The widget shows Approve / Deny (`ApproveFromLiveActivityIntent`,
/// `DenyFromLiveActivityIntent`) only for an inline question, else Open.
@MainActor
final class BotLiveActivityManager {
    static let shared = BotLiveActivityManager()

    /// cloud-api `BOT_ATTRIBUTES` / `BOT_ACTIVITY_KEY` (the attributes' `mode`).
    nonisolated static let attributesType = "BotActivityAttributes"
    nonisolated static let activityKey = "summary"

    private static let log = Logger(subsystem: "com.allternit.mobile", category: "live-activity")

    private var activity: Activity<BotActivityAttributes>?
    /// True when this process requested the activity from `BotStatusStore`
    /// state. Local sync only ends activities it started; a server-started
    /// or "Watch a bot" activity ends by push (or the 8-hour system limit).
    private var startedLocally = false
    /// Activity ids whose token and state streams are being watched.
    private var observed: Set<String> = []
    private var installed = false

    private init() {}

    // MARK: - Launch

    /// Called once from `AppDelegate` at launch: fills the intent registry,
    /// adopts activities that survived a relaunch (or were started by push),
    /// and starts uploading their tokens.
    static func installAtLaunch() {
        LiveActivityActions.handler = AppLiveActivityActions()
        shared.startObserving()
        LoopLiveActivityManager.shared.startObserving()
        MeetingLiveActivityManager.shared.startObserving()
    }

    func startObserving() {
        guard !installed else { return }
        installed = true
        for existing in Activity<BotActivityAttributes>.activities {
            adopt(id: existing.id)
        }
        Task { await Self.watchNewActivities() }
        if #available(iOS 17.2, *) {
            Task { await Self.watchPushToStartTokens() }
        }
    }

    /// An activity this process did not request (push-to-start, relaunch).
    fileprivate func adopt(id: String) {
        guard let found = Self.live(id) else { return }
        if activity == nil, found.activityState == .active || found.activityState == .stale {
            activity = found
            startedLocally = false
        }
        observe(id: id)
    }

    private func observe(id: String) {
        guard observed.insert(id).inserted else { return }
        Task { await Self.watchTokens(id: id) }
        Task { await Self.watchState(id: id) }
    }

    fileprivate func didEnd(id: String) async {
        observed.remove(id)
        if activity?.id == id {
            activity = nil
            startedLocally = false
        }
        // Another live bot activity (rare) keeps the slot's token.
        let othersLive = Activity<BotActivityAttributes>.activities.contains {
            $0.id != id && ($0.activityState == .active || $0.activityState == .stale)
        }
        if !othersLive {
            await ActivityTokenRegistry.shared.remove(kind: .laUpdate, attributesType: Self.attributesType, activityKey: Self.activityKey)
        }
    }

    private nonisolated static func watchNewActivities() async {
        for await new in Activity<BotActivityAttributes>.activityUpdates {
            let id = new.id
            await BotLiveActivityManager.shared.adopt(id: id)
        }
    }

    @available(iOS 17.2, *)
    private nonisolated static func watchPushToStartTokens() async {
        for await token in Activity<BotActivityAttributes>.pushToStartTokenUpdates {
            await ActivityTokenRegistry.shared.update(kind: .laStart, attributesType: attributesType, activityKey: nil, token: token)
        }
    }

    private nonisolated static func watchTokens(id: String) async {
        guard let tracked = live(id) else { return }
        for await token in tracked.pushTokenUpdates {
            await ActivityTokenRegistry.shared.update(kind: .laUpdate, attributesType: attributesType, activityKey: activityKey, token: token)
        }
    }

    private nonisolated static func watchState(id: String) async {
        guard let tracked = live(id) else { return }
        for await state in tracked.activityStateUpdates where state == .ended || state == .dismissed {
            await BotLiveActivityManager.shared.didEnd(id: id)
            return
        }
    }

    // MARK: - Local sync (BotStatusStore)

    /// Call after any change to `BotStatusStore.entries`. If a bot is pinned,
    /// shows a single-bot Activity while that bot is active; otherwise shows a
    /// summary Activity across all active bots. Ends an Activity this process
    /// started when there is nothing to display.
    func sync(with entries: [String: BotStatusStore.Entry], pinnedBot: BotStatusStore.PinnedBot? = nil) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }

        let contentState = Self.projectedContentState(from: entries, pinnedBot: pinnedBot)

        guard let contentState else {
            if startedLocally { end() }
            return
        }

        if let activity = activity {
            let id = activity.id
            let merged = Self.merged(local: contentState, current: activity.content.state)
            guard merged != activity.content.state else { return }
            Task { await Self.live(id)?.update(ActivityContent(state: merged, staleDate: nil)) }
        } else {
            start(contentState: contentState)
            startedLocally = activity != nil
        }
    }

    /// Pure projection of bot state into a Live Activity content state. If a
    /// pinned bot is active, returns a single-bot state; otherwise returns the
    /// dominant summary state. Returns `nil` when nothing is active, so tests
    /// can verify the logic without touching ActivityKit.
    static func projectedContentState(
        from entries: [String: BotStatusStore.Entry],
        pinnedBot: BotStatusStore.PinnedBot? = nil
    ) -> BotActivityAttributes.ContentState? {
        let active = entries.filter {
            $0.value.state.status != .idle && $0.value.state.status != .offline
        }

        // Single-bot mode: pinned bot takes precedence while it is active.
        if let pinnedBot = pinnedBot, let entry = active[pinnedBot.botId] {
            var state = BotActivityAttributes.ContentState(
                status: entry.state.status.rawValue,
                activityLabel: entry.state.activityLabel,
                pendingApprovalsCount: entry.state.pendingApprovalsCount,
                activeBotsCount: 1,
                attentionBotsCount: entry.state.status.needsAttention ? 1 : 0,
                displayName: pinnedBot.displayName
            )
            state.botId = pinnedBot.botId
            return state
        }

        let attention = active.values.filter { $0.state.status.needsAttention }

        guard !active.isEmpty else { return nil }

        let dominant = active.values.map(\.state.status).max {
            $0.precedence < $1.precedence
        } ?? .idle

        let representative = active.values.first { $0.state.status == dominant }
        let label = representative?.state.activityLabel

        return BotActivityAttributes.ContentState(
            status: dominant.rawValue,
            activityLabel: label,
            pendingApprovalsCount: active.values.reduce(0) { $0 + $1.state.pendingApprovalsCount },
            activeBotsCount: active.count,
            attentionBotsCount: attention.count,
            displayName: nil
        )
    }

    /// Local state wins for status and counts (the SSE fold is fresher while
    /// the app runs); the server-only fields of the same focus bot are kept.
    /// The question survives only while the local status still needs the
    /// owner, so a stale Approve button never outlives its item.
    static func merged(local: BotActivityAttributes.ContentState,
                       current: BotActivityAttributes.ContentState) -> BotActivityAttributes.ContentState {
        var out = local
        if let localBot = local.botId, let currentBot = current.botId, localBot != currentBot { return out }
        out.botId = local.botId ?? current.botId
        out.displayName = local.displayName ?? current.displayName
        out.step = current.step
        out.progress = current.progress
        out.sessionId = current.sessionId
        out.runtimeId = current.runtimeId
        out.othersCount = max(local.activeBotsCount - 1, 0)
        if local.status == "waiting_approval" || local.status == "waiting_input" {
            out.question = current.question
            out.inboxId = current.inboxId
            out.contentHash = current.contentHash
            out.approveInline = current.approveInline
        }
        return out
    }

    // MARK: - Watch a bot (intent)

    /// Starts the activity for one bot (or the summary) from the background,
    /// or refreshes the one that is up. Returns false when Live Activities
    /// are off or the request failed.
    func watch(botId: String?) -> Bool {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return false }
        let store = BotStatusStore.shared
        var state: BotActivityAttributes.ContentState
        if let botId, let entry = store.entries[botId] {
            state = BotActivityAttributes.ContentState(
                status: entry.state.status.rawValue,
                activityLabel: entry.state.activityLabel,
                pendingApprovalsCount: entry.state.pendingApprovalsCount,
                activeBotsCount: entry.state.status == .idle || entry.state.status == .offline ? 0 : 1,
                attentionBotsCount: entry.state.status.needsAttention ? 1 : 0,
                displayName: store.pinnedBot?.botId == botId ? store.pinnedBot?.displayName : nil
            )
            state.botId = botId
        } else if let projected = Self.projectedContentState(from: store.entries, pinnedBot: store.pinnedBot) {
            state = projected
        } else {
            // Nothing known yet (background launch before the SSE fold):
            // the next server push fills it.
            state = BotActivityAttributes.ContentState(
                status: "idle", activityLabel: nil, pendingApprovalsCount: 0,
                activeBotsCount: 0, attentionBotsCount: 0, displayName: nil
            )
            state.botId = botId
        }
        if let activity {
            let id = activity.id
            let next = Self.merged(local: state, current: activity.content.state)
            Task { await Self.live(id)?.update(ActivityContent(state: next, staleDate: nil)) }
            return true
        }
        start(contentState: state)
        startedLocally = false
        return activity != nil
    }

    // MARK: - After an action

    /// Approve / Deny went through: drop the question at once instead of
    /// waiting for the server's next push.
    func didResolve(inboxId: String) {
        guard let activity, activity.content.state.inboxId == inboxId else { return }
        var state = activity.content.state
        state.question = nil
        state.inboxId = nil
        state.contentHash = nil
        state.approveInline = nil
        state.status = "working"
        state.activityLabel = nil
        let id = activity.id
        Task { await Self.live(id)?.update(ActivityContent(state: state, staleDate: nil)) }
    }

    /// The item changed since the push: hide the inline buttons; Open stays.
    func didChange(inboxId: String) {
        guard let activity, activity.content.state.inboxId == inboxId else { return }
        var state = activity.content.state
        state.approveInline = false
        let id = activity.id
        Task { await Self.live(id)?.update(ActivityContent(state: state, staleDate: nil)) }
    }

    // MARK: - Sign-out

    /// Called from `PushNotificationManager.willSignOut()` while the
    /// Bearer still works: deletes the server copies of every activity
    /// token and ends the activities now (they show account data).
    static func willSignOut() async {
        await ActivityTokenRegistry.shared.deleteAll()
        shared.endAllNow()
        LoopLiveActivityManager.shared.endAllNow()
        MeetingLiveActivityManager.shared.endAll()
    }

    private func endAllNow() {
        activity = nil
        startedLocally = false
        let ids = Activity<BotActivityAttributes>.activities.map(\.id)
        Task {
            for id in ids {
                await Self.live(id)?.end(nil, dismissalPolicy: .immediate)
            }
        }
    }

    // MARK: - ActivityKit

    private func start(contentState: BotActivityAttributes.ContentState) {
        let attributes = BotActivityAttributes(mode: Self.activityKey)
        let content = ActivityContent(state: contentState, staleDate: nil)
        do {
            activity = try Activity.request(attributes: attributes, content: content, pushType: .token)
        } catch {
            // No push entitlement (unsigned or simulator builds): a local
            // activity still works, it just can't be updated by push.
            Self.log.notice("push-backed request failed: \(error.localizedDescription, privacy: .public)")
            do {
                activity = try Activity.request(attributes: attributes, content: content)
            } catch {
                // Live Activities may be disabled in Settings or the per-app
                // concurrent-activity limit may be reached — no recovery needed.
                return
            }
        }
        if let activity { observe(id: activity.id) }
    }

    private func end() {
        guard let activity = activity else { return }
        self.activity = nil
        startedLocally = false
        let id = activity.id
        Task {
            await Self.live(id)?.end(
                nil,
                dismissalPolicy: .after(.now.addingTimeInterval(30))
            )
        }
    }

    /// Activity isn't Sendable, so tasks carry its id and look it up here.
    private nonisolated static func live(_ id: String) -> Activity<BotActivityAttributes>? {
        Activity<BotActivityAttributes>.activities.first { $0.id == id }
    }
}

/// The app's side of `LiveActivityActions`: Approve / Deny go through the
/// I1 resolver (`NotificationActions.resolve`) under a background task
/// assertion; "Watch a bot" starts the activity.
struct AppLiveActivityActions: LiveActivityActionHandling {
    func decide(_ decision: LiveActivityDecision, inboxId: String, runtimeId: String?,
                contentHash: String?) async -> LiveActivityActionOutcome {
        let assertion = await BackgroundAssertion.begin(name: "live-activity-\(decision.rawValue)")
        let result = await NotificationActions.resolve(
            inboxId: inboxId,
            action: decision == .approve ? .approve : .deny,
            contentHash: contentHash,
            runtimeId: runtimeId,
            sessionId: nil,
            replyText: nil
        )
        await assertion.end()
        switch result {
        case .done, .alreadyHandled:
            await MainActor.run {
                InboxStore.shared.removeLocally(inboxId)
                BotLiveActivityManager.shared.didResolve(inboxId: inboxId)
            }
            return .done
        case .changed:
            await MainActor.run { BotLiveActivityManager.shared.didChange(inboxId: inboxId) }
            return .changed
        case .offline: return .offline
        case .signedOut: return .signedOut
        case .failed, .invalid: return .failed
        }
    }

    func watch(botId: String?) async -> Bool {
        await BotLiveActivityManager.shared.watch(botId: botId)
    }
}
