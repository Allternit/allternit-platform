import ActivityKit
import Foundation

/// Shared between the main app target (which starts/updates/ends the
/// Activity from `BotLiveActivityManager`) and the `AllternitWidgets`
/// extension (which renders it) — both targets include this file directly
/// via `project.yml`'s `Core/LiveActivity` source path.
///
/// Unlike `LoopActivityAttributes`, this activity summarizes the dominant
/// bot operational state across all subscribed bots, so a user can glance
/// at the Dynamic Island to see whether any bot needs attention.
struct BotActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// Canonical status raw value (`working`, `waiting_approval`, …).
        var status: String
        /// Human-readable sub-label, e.g. "Running" or "Waiting for approval".
        var activityLabel: String?
        /// Total pending approvals across all active bots (single-bot mode
        /// uses the bot's own count).
        var pendingApprovalsCount: Int
        /// Number of bots currently in a non-idle/non-offline state.
        var activeBotsCount: Int
        /// Number of bots that `needsAttention` (waiting_approval, blocked,
        /// failed, degraded).
        var attentionBotsCount: Int
        /// In single-bot mode, the display name of the pinned bot. Server
        /// pushes fill it with the focus bot's name.
        var displayName: String?

        // Option A, bot first (plan I3). Optional so older pushes and the
        // local `BotStatusStore` projection still decode. Key names and
        // types match cloud-api `routes/live_activity.rs`
        // `bot_content_state`; absent values are left out of the JSON.
        // No `Date` values ride in the content state.

        /// The focus bot's id.
        var botId: String? = nil
        /// What the focus bot is doing now (server cuts it to 120 characters).
        var step: String? = nil
        /// 0...1 when the runtime knows it.
        var progress: Double? = nil
        /// Active bots other than the focus bot ("+N others").
        var othersCount: Int? = nil
        /// Set while the focus bot needs the owner: the full question when
        /// `approveInline` is true, else a generic line ("Needs your approval").
        var question: String? = nil
        /// Inbox item the question resolves (`approval:<id>`, `attention:<id>`).
        var inboxId: String? = nil
        /// Runtime that owns the item; the resolver targets it.
        var runtimeId: String? = nil
        var sessionId: String? = nil
        /// Hash of the approved text; Approve sends it so a changed text is refused.
        var contentHash: String? = nil
        /// True only when Approve and Deny may show on the activity.
        var approveInline: Bool? = nil

        /// Approve / Deny buttons show only with an inline question, an
        /// inbox id and a content hash; otherwise the activity offers Open.
        var showsInlineDecision: Bool {
            approveInline == true && question?.isEmpty == false
                && inboxId?.isEmpty == false && contentHash?.isEmpty == false
        }

        /// `allternit://inbox/<id>` for the Open link.
        var inboxURL: URL? {
            guard let inboxId, !inboxId.isEmpty,
                  let encoded = inboxId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) else { return nil }
            return URL(string: "allternit://inbox/\(encoded)")
        }
    }

    /// Reserved for future variants (e.g., a pinned single-bot activity).
    /// Today the only supported value is `"summary"`.
    var mode: String
}
