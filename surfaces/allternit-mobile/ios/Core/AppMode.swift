import SwiftUI

/// Platform modes, mirroring the web shell's `AppMode`
/// (surfaces/ai.allternit.com/src/shell/ModeSwitcher.tsx:24-70).
///
/// `design` is skipped on iOS — on the web it opens an external window.
/// This is the session-stamping / theme mode. The visible switcher is the
/// drawer's mode dial over `ShellMode` (Home · Bots · Code · ACI), which
/// maps onto these. Cowork is a composer-level toggle inside Chats
/// (BottomDock.tsx ChatCoworkToggle), not a dial mode.
enum AppMode: String, CaseIterable, Sendable {
    case chat
    case cowork
    case code
    case browser

    /// `origin_surface` value sent on session create — matches the raw
    /// value the web's mode-session-store uses (mode-session-store.ts:738).
    var originSurface: String { rawValue }

    var label: String {
        switch self {
        case .chat: return "Chat"
        case .cowork: return "Cowork"
        case .code: return "Code"
        case .browser: return "ACI"
        }
    }

    /// Mode-aware theme (accent / soft / glow / icon) — see ModeTheme.swift.
    var theme: ModeTheme { ModeTheme(mode: self) }
}

/// The phone shell's four modes — the PWA mode dial's Home · Bots · Code ·
/// ACI (allternit-ai src/shell/mode-buttons.ts MODE_BUTTONS, phone-nav.ts
/// `PhoneMode`). Each picks the drawer's rail, wordmark and recents and opens
/// its root surface. `AppMode` stays the session-stamping / theme enum
/// underneath (Bots stamps no surface of its own).
enum ShellMode: String, CaseIterable, Sendable {
    case home, bots, code, aci

    var label: String {
        switch self {
        case .home: return "Home"
        case .bots: return "Bots"
        case .code: return "Code"
        case .aci: return "ACI"
        }
    }

    /// SF Symbols for the web's Phosphor House / Robot / TerminalWindow / Globe.
    var icon: String {
        switch self {
        case .home: return "house.fill"
        case .bots: return "cpu.fill"
        case .code: return "terminal.fill"
        case .aci: return "globe"
        }
    }

    /// The root section a mode opens on (phone-nav.ts MODE_SECTION).
    var rootTab: ModeBarItem {
        switch self {
        case .home: return .chats
        case .bots: return .agents
        case .code: return .code
        case .aci: return .aci
        }
    }

    /// The session-stamping mode underneath. Bots keeps chat (bot threads
    /// are chats with an `agent_id`).
    var appMode: AppMode {
        switch self {
        case .home, .bots: return .chat
        case .code: return .code
        case .aci: return .browser
        }
    }

    /// The wordmark suffix beside A://TERNIT (PhoneDrawer WORDMARK_SUFFIX);
    /// Code shows the gizzi code mark instead.
    var wordmarkSuffix: String? {
        switch self {
        case .home, .code: return nil
        case .bots: return "BOTS"
        case .aci: return "ACI"
        }
    }

    /// Accent used for the active dial and highlights.
    var accent: Color {
        switch self {
        case .home: return Theme.accentChat
        case .bots: return Theme.accentCowork
        case .code: return Theme.accentCode
        case .aci: return Theme.accentBrowser
        }
    }

    /// The mode a section belongs to (phone-nav.ts SECTION_MODE); nil for
    /// shared sections (Projects, Artifacts, …) that keep the current mode.
    static func owner(of tab: ModeBarItem) -> ShellMode? {
        switch tab {
        case .chats: return .home
        case .agents: return .bots
        case .code: return .code
        case .aci: return .aci
        case .projects, .artifacts, .automation, .models: return nil
        }
    }
}

/// App-wide mode state, injected from AllternitApp as an @EnvironmentObject.
///
/// Persists to UserDefaults under the same key the web uses in localStorage
/// ("allternit-platform-mode", ModeSwitcher.tsx:73), so the contract stays
/// recognizable across surfaces.
@MainActor
final class AppModeStore: ObservableObject {
    static let storageKey = "allternit-platform-mode"
    static let shellStorageKey = "allternit-phone-mode"

    @Published var mode: AppMode {
        didSet {
            UserDefaults.standard.set(mode.rawValue, forKey: Self.storageKey)
        }
    }

    /// The mode dial's mode (Home · Bots · Code · ACI). Picks the drawer's
    /// rail and recents; changed via `setShellMode` or by opening a section
    /// that belongs to a mode.
    @Published private(set) var shellMode: ShellMode {
        didSet {
            UserDefaults.standard.set(shellMode.rawValue, forKey: Self.shellStorageKey)
        }
    }

    /// The section whose surface fills the content pane. Mode roots
    /// (Chats / Bots / Code / ACI) track `shellMode`; Projects, Artifacts,
    /// Automation Tasks and Models are shared drawer sections layered on
    /// top (the mode — and so recents, accent and session stamping — stays
    /// wherever it was).
    @Published var activeTab: ModeBarItem

    /// Which sub-surface the Automation Tasks tab shows: cron jobs
    /// (`/v1/cron`, Phase 1), routines (`/v1/automations/routines`, Phase 2),
    /// or loops (`/v1/automations/loops`, Phase 3). All three list views
    /// read/write this via a shared segmented control so any one is
    /// reachable from the same tab rather than needing a separate nav item.
    @Published var automationKind: AutomationKind = .cron

    init(defaults: UserDefaults = .standard) {
        let shell: ShellMode
        if launchArgumentEnabled("chat") {
            shell = .home
        } else if launchArgumentEnabled("code") {
            shell = .code
        } else if launchArgumentEnabled("browser") || launchArgumentEnabled("open-aci") {
            shell = .aci
        } else if launchArgumentEnabled("bots") || launchArgumentEnabled("open-bots") {
            shell = .bots
        } else if let saved = defaults.string(forKey: Self.shellStorageKey).flatMap(ShellMode.init(rawValue:)) {
            shell = saved
        } else {
            // Pre-shell installs saved only the AppMode; map it across.
            switch defaults.string(forKey: Self.storageKey).flatMap(AppMode.init(rawValue:)) {
            case .code: shell = .code
            case .browser: shell = .aci
            default: shell = .home
            }
        }
        self.shellMode = shell
        self.mode = shell.appMode
        self.activeTab = shell.rootTab
    }

    /// Mode dial: switch modes and open the mode's root section.
    func setShellMode(_ next: ShellMode) {
        shellMode = next
        activeTab = next.rootTab
        if mode != next.appMode { mode = next.appMode }
    }

    /// Open a section. A mode's own section (Chats, Bots, Code, ACI) moves
    /// the dial to that mode; shared sections keep the current mode. Chats
    /// always lands on plain chat (cowork is a composer toggle inside it).
    func selectBarItem(_ item: ModeBarItem) {
        activeTab = item
        if let owner = ShellMode.owner(of: item) {
            shellMode = owner
            if mode != owner.appMode { mode = owner.appMode }
        }
    }
}

/// The sections the content pane can show: the four mode roots plus the
/// drawer's shared sections. `agents` is the Bots root (the case name stays
/// for existing callers).
enum ModeBarItem: String, CaseIterable {
    case chats, projects, artifacts, agents, automation, models, code, aci

    var label: String {
        switch self {
        case .chats: return "Chats"
        case .projects: return "Projects"
        case .artifacts: return "Artifacts"
        case .agents: return "Bots"
        case .automation: return "Automation Tasks"
        case .models: return "Models"
        case .code: return "Code"
        case .aci: return "ACI"
        }
    }

    /// SF Symbols standing in for the web's Phosphor icons.
    var icon: String {
        switch self {
        case .chats: return "bubble.left"
        case .projects: return "folder"
        case .artifacts: return "archivebox"
        case .agents: return "cpu"
        case .automation: return "clock.arrow.circlepath"
        case .models: return "brain.head.profile"
        case .code: return "terminal"
        case .aci: return "globe"
        }
    }

    /// The section a launch-time `AppMode` lands on: cowork lives inside
    /// Chats; code/ACI map to themselves.
    static func tab(for mode: AppMode) -> ModeBarItem {
        switch mode {
        case .chat, .cowork: return .chats
        case .code: return .code
        case .browser: return .aci
        }
    }
}

/// The sub-surfaces of the Automation Tasks tab (`ModeBarItem.automation`).
enum AutomationKind: String, CaseIterable {
    case cron = "Cron"
    case routines = "Routines"
    case loops = "Loops"
}
