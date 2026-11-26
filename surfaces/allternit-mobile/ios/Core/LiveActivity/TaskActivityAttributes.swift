import ActivityKit
import Foundation

/// One bar job (a task the user asked a bot to run through an app) as a Live
/// Activity. Shared between the main app target (`BotLiveActivityManager`
/// starts, updates and ends it) and the `AllternitWidgets` extension
/// (`TaskLiveActivity.swift` renders it); both include this file through
/// `project.yml`'s `Core/LiveActivity` source path.
///
/// Wording follows the Island bar contract, section 5 (handoffs/
/// island-bar-team-map-2026-11-25/CONTRACT.md). The activity is updated by the
/// app process only: cloud-api's `routes/live_activity.rs` knows the Bot and
/// Loop attribute types, not this one, so no push tokens are registered.
struct TaskActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// Contract `JobState`: working, waiting_approval, input_required,
        /// review, completed, failed, cancelled.
        var state: String
        /// The approver, while `state == waiting_approval` ("Waiting on Noah").
        var approverName: String? = nil
        /// Bot running the job (expanded and lock screen second line).
        var botName: String? = nil
        /// Short amount for the compact trailing slot, e.g. "€320".
        var amount: String? = nil
        /// Completed: the receipt line, e.g. "Refunded €320".
        var receipt: String? = nil
        /// Failed: the server's error text.
        var error: String? = nil
        /// True on the owner's phone (the approver). Only then may the
        /// question and Approve / Deny show.
        var isOwner: Bool = false
        /// Owner variant: the full question and the inbox item it resolves.
        /// Same fields and rules as `BotActivityAttributes.ContentState`;
        /// Approve / Deny reuse its `LiveActivityIntent`s.
        var question: String? = nil
        var inboxId: String? = nil
        var runtimeId: String? = nil
        var contentHash: String? = nil
        var approveInline: Bool? = nil

        var isTerminal: Bool {
            state == "completed" || state == "failed" || state == "cancelled"
        }

        /// Approve / Deny only for the owner, with an inline question, an
        /// inbox id and a content hash. Otherwise the activity offers Open.
        var showsInlineDecision: Bool {
            isOwner && state == "waiting_approval" && approveInline == true
                && question?.isEmpty == false
                && inboxId?.isEmpty == false && contentHash?.isEmpty == false
        }

        /// `allternit://inbox/<id>` for the Open link.
        var inboxURL: URL? {
            guard let inboxId, !inboxId.isEmpty,
                  let encoded = inboxId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) else { return nil }
            return URL(string: "allternit://inbox/\(encoded)")
        }

        /// CONTRACT.md section 5 wording. Never a raw code.
        var stateLine: String {
            switch state {
            case "waiting_approval":
                if let name = approverName, !name.isEmpty { return "Waiting on \(name)" }
                return "Waiting on approval"
            case "working": return "Working"
            case "input_required": return "Needs input"
            case "review": return "Review"
            case "completed":
                if let receipt, !receipt.isEmpty { return receipt }
                return "Completed"
            case "failed":
                if let error, !error.isEmpty { return "Failed: \(error)" }
                return "Failed"
            case "cancelled": return "Cancelled"
            default: return state.capitalized
            }
        }

        /// Short word for the compact trailing slot when there is no amount.
        var shortState: String {
            switch state {
            case "waiting_approval": return "Waiting"
            case "working": return "Working"
            case "input_required": return "Input"
            case "review": return "Review"
            case "completed": return "Done"
            case "failed": return "Failed"
            case "cancelled": return "Cancelled"
            default: return state.capitalized
            }
        }

        /// Four-step line: Asked, Approval, Running, Done. Steps reached.
        var stepsReached: Int {
            switch state {
            case "waiting_approval": return 2
            case "working", "input_required": return 3
            case "review": return 3
            case "completed": return 4
            default: return 3 // failed / cancelled stop where they were
            }
        }
    }

    static let stepLabels = ["Asked", "Approval", "Running", "Done"]

    /// Bar job id.
    var jobId: String
    /// What the job does ("Refund payment").
    var title: String
    /// The app it runs in ("Stripe").
    var appName: String
}
