import ActivityKit
import Foundation

/// Shared between the main app (`MeetingLiveActivityManager` starts, updates
/// and ends it) and the `AllternitWidgets` extension (`MeetingLiveActivity`
/// draws it); both compile `Core/LiveActivity` via `project.yml`.
///
/// One activity per active meeting session: Gizzi's state in the call, how
/// long it has been live, and whether Gizzi is listening. The app updates it
/// while it runs; cloud-api (`routes/meeting_notify.rs`) starts, updates and
/// ends it by APNs push while the app is suspended. A test there decodes this
/// file, so the field names must match its `content_state`.
struct MeetingActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// CONTRACT §3 `gizzi`: `launching|joining|waiting_admit|live|paused|ended|not_admitted`.
        var gizzi: String
        /// When the call went live (epoch seconds); drives the elapsed timer.
        /// No `Date` in the content state, matching the other activities.
        var liveSince: Double?
        /// False while Gizzi is paused (Stop listening).
        var listening: Bool

        /// The chip text, the same words as the app.
        var stateLabel: String {
            switch gizzi {
            case "launching", "joining", "scheduled": return "Joining"
            case "waiting_admit": return "Waiting to be let in"
            case "live": return "Live"
            case "paused": return "Paused"
            case "not_admitted": return "Not let in"
            case "ended": return "Ended"
            default: return gizzi.capitalized
            }
        }

        /// The second line under the title.
        var detail: String {
            switch gizzi {
            case "live": return listening ? "Gizzi is listening" : "Gizzi stopped listening"
            case "paused": return "Gizzi stopped listening"
            case "waiting_admit": return "Let Gizzi (Allternit) in from the call"
            case "launching", "joining", "scheduled": return "Gizzi is joining the call"
            case "not_admitted": return "Nobody let Gizzi in"
            default: return "Gizzi left the call"
            }
        }

        var liveSinceDate: Date? { liveSince.map { Date(timeIntervalSince1970: $0) } }
    }

    var meetingId: String
    var sessionId: String
    var title: String
    /// "Google Meet" | "Zoom" | "Microsoft Teams".
    var platformLabel: String
}
