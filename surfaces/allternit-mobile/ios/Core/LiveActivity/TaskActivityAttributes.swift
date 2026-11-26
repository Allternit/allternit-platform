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
/// app process while it runs, and by cloud-api push (`routes/live_activity.rs`,
/// `bar_job.changed`) otherwise: push-to-start opens it, the update token
/// (`activityKey` = `jobId`) updates and ends it. The ContentState keys below
/// are the push payload's `content-state` keys exactly.
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
                if let receipt, !receipt.isEmpty { return BarReceiptWording.pastTense(receipt: receipt) }
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

/// Past-tense receipt wording for bar jobs (CONTRACT.md section 5: "Refunded
/// €320", never "Refund payment €320"). Shared by the widget (pushed receipts
/// arrive as the runtime's `"<tool label> <amount>"`) and the app
/// (`BarWire.receipt`, the in-app bar), so both read the same.
enum BarReceiptWording {
    /// Leading verb of a tool label -> its past tense. nil = not known.
    static func pastTense(verb: String) -> String? {
        let known: [String: String] = [
            "refund": "Refunded", "create": "Created", "send": "Sent", "add": "Added", "update": "Updated",
            "delete": "Deleted", "remove": "Removed", "cancel": "Cancelled", "list": "Listed", "post": "Posted",
            "charge": "Charged", "pay": "Paid", "book": "Booked", "schedule": "Scheduled", "close": "Closed",
            "open": "Opened", "archive": "Archived", "invite": "Invited", "approve": "Approved", "reply": "Replied",
            "get": "Got", "find": "Found", "search": "Searched", "issue": "Issued", "transfer": "Transferred",
            "assign": "Assigned", "move": "Moved", "start": "Started", "stop": "Stopped", "run": "Ran",
        ]
        return known[verb.lowercased()]
    }

    /// Rewrites a present-tense receipt into the past tense:
    /// "Refund payment €320" -> "Refunded €320", "Refund payment 320 CHF" ->
    /// "Refunded 320 CHF", "Create invoice done" -> "Created invoice".
    /// A receipt that does not start with a known verb is kept as sent.
    static func pastTense(receipt: String) -> String {
        let words = receipt.split(separator: " ").map(String.init)
        guard let first = words.first, let verb = pastTense(verb: first) else { return receipt }
        var rest = Array(words.dropFirst())
        if rest.last?.lowercased() == "done" {
            rest.removeLast()
            return rest.isEmpty ? verb : "\(verb) \(rest.joined(separator: " ").lowercased())"
        }
        // The amount starts at the first word with a digit or a currency sign.
        if let i = rest.firstIndex(where: { w in w.contains(where: { $0.isNumber || $0.isCurrencySymbol }) }) {
            return "\(verb) \(rest[i...].joined(separator: " "))"
        }
        return rest.isEmpty ? verb : "\(verb) \(rest.joined(separator: " ").lowercased())"
    }
}
