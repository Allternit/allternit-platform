import ActivityKit
import Foundation

/// Starts/updates/ends a Lock Screen + Dynamic Island Live Activity that
/// summarizes the dominant operational state across all subscribed bots.
///
/// One summary Activity is shown at a time. It appears when any bot is
/// working or needs attention, updates as statuses change, and dismisses
/// once every tracked bot is idle or offline.
///
/// The manager is intentionally decoupled from `BotStatusStore`'s SSE fold:
/// `BotStatusStore` calls `sync(with:)` whenever its entries map changes,
/// and this class computes the dominant state and owns the Activity lifecycle.
@MainActor
final class BotLiveActivityManager {
    static let shared = BotLiveActivityManager()

    private var activity: Activity<BotActivityAttributes>?

    private init() {}

    /// Call after any change to `BotStatusStore.entries`. If a bot is pinned,
    /// shows a single-bot Activity while that bot is active; otherwise shows a
    /// summary Activity across all active bots. Ends the Activity when there is
    /// nothing to display.
    func sync(with entries: [String: BotStatusStore.Entry], pinnedBot: BotStatusStore.PinnedBot? = nil) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }

        let contentState = Self.projectedContentState(from: entries, pinnedBot: pinnedBot)

        guard let contentState else {
            end()
            return
        }

        if let activity = activity {
            let id = activity.id
            Task { await Self.live(id)?.update(ActivityContent(state: contentState, staleDate: nil)) }
        } else {
            start(contentState: contentState)
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
        // Single-bot mode: pinned bot takes precedence while it is active.
        if let pinnedBot = pinnedBot,
           let entry = entries[pinnedBot.botId],
           entry.state.status != .idle && entry.state.status != .offline {
            return BotActivityAttributes.ContentState(
                status: entry.state.status.rawValue,
                activityLabel: entry.state.activityLabel,
                pendingApprovalsCount: entry.state.pendingApprovalsCount,
                activeBotsCount: 1,
                attentionBotsCount: entry.state.status.needsAttention ? 1 : 0,
                displayName: pinnedBot.displayName
            )
        }

        let active = entries.values.filter {
            $0.state.status != .idle && $0.state.status != .offline
        }
        let attention = active.filter { $0.state.status.needsAttention }

        guard !active.isEmpty else { return nil }

        let dominant = active.map(\.state.status).max {
            $0.precedence < $1.precedence
        } ?? .idle

        let representative = active.first { $0.state.status == dominant }
        let label = representative?.state.activityLabel

        return BotActivityAttributes.ContentState(
            status: dominant.rawValue,
            activityLabel: label,
            pendingApprovalsCount: active.reduce(0) { $0 + $1.state.pendingApprovalsCount },
            activeBotsCount: active.count,
            attentionBotsCount: attention.count,
            displayName: nil
        )
    }

    private func start(contentState: BotActivityAttributes.ContentState) {
        let attributes = BotActivityAttributes(mode: "summary")
        do {
            activity = try Activity.request(
                attributes: attributes,
                content: .init(state: contentState, staleDate: nil)
            )
        } catch {
            // Live Activities may be disabled in Settings or the per-app
            // concurrent-activity limit may be reached — no recovery needed.
        }
    }

    private func end() {
        guard let activity = activity else { return }
        self.activity = nil
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
