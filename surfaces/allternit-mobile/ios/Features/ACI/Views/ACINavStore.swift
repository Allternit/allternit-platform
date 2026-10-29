import Foundation

/// ACI's four views — the web phone's `AciTab` (phone-nav.ts): the computer
/// agent, MCP Apps beside it, the paired computers and ACI extensions. The
/// drawer's ACI rail picks one; the screen itself shows no tab strip.
enum ACITab: String, CaseIterable, Sendable {
    case agent
    case apps
    case computers
    case extensions
}

/// Which ACI view is showing, shared by the drawer rail and `ACITabView`
/// (phone-nav.ts `aciTab` / `setAciTab` / `openTeach`).
@MainActor
final class ACINavStore: ObservableObject {
    static let shared = ACINavStore()

    @Published var tab: ACITab = .agent
    /// Teach (site APIs) open full screen over ACI.
    @Published var isTeachOpen = false

    func open(_ tab: ACITab) {
        self.tab = tab
    }

    /// The drawer's Teach row: the Agent view with Teach over it.
    func openTeach() {
        tab = .agent
        isTeachOpen = true
    }
}

/// A suggestion shown until it's dismissed once (useOnceNudge.ts): same
/// storage key names as the PWA's localStorage.
enum ACIOnceNudge {
    static func key(_ name: String) -> String { "allternit-phone:nudge:\(name)" }

    static func seen(_ name: String) -> Bool {
        UserDefaults.standard.string(forKey: key(name)) == "1"
    }

    static func remember(_ name: String) {
        UserDefaults.standard.set("1", forKey: key(name))
    }
}
