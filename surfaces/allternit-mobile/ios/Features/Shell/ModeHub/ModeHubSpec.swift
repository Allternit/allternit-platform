import SwiftUI

// Mode hub spec, iOS port of the frozen contract:
//   /Users/joe/Desktop/allternit-workspace/handoffs/composer-cleanup-2026-11-21/CONTRACT.md
//   (web source: allternit-ai src/shell/mode-hub/contract.ts). Changes after the
//   freeze need a main-session OK; edit CONTRACT.md first, then this file.
//
// I0 inventory: contract section -> existing iOS screen it opens.
// A section with "none" is hidden on iOS (no new screens). Today none is hidden.
//
// | Mode  | id            | Opens on iOS                                                    |
// |-------|---------------|-----------------------------------------------------------------|
// | Home  | recents       | ChatView (ModeBarItem.chats; recents are the drawer list)        |
// | Home  | projects      | ProjectsListView (.projects)                                    |
// | Home  | artifacts     | ArtifactsLibraryView (.artifacts)                               |
// | Home  | automation    | AutomationTasksPhoneView (.automation)                          |
// | Home  | meetings      | MeetingsView (sheet, as the drawer opens it); flag-gated         |
// | Home  | customize     | CustomizeScreen (.customize)                                    |
// | Home  | freeweights   | ModelsTabView + FreeweightsBrowseView (.models)                  |
// | Bots  | main-bot      | BotsModeRootView -> BotRoute.chat(botId, nil) / .page           |
// | Bots  | bots          | BotsModeRootView / BotLaunchpadView (.agents)                   |
// | Bots  | groups        | BotRoute.groups -> GroupChatView                                |
// | Bots  | office        | OfficeFloorView (Features/Office/Floor)                         |
// | Bots  | bot-projects  | BotRoute.project -> BotProjectPage                              |
// | Code  | sessions      | CodeModeView (.code)                                            |
// | Code  | code-projects | ProjectsListView (.projects)                                    |
// | Code  | code-artifacts| ArtifactsLibraryView (.artifacts)                               |
// | Code  | code-automation| AutomationTasksPhoneView (.automation)                         |
// | ACI   | computers     | ACIComputersPane (inside ACITabView, .aci)                      |
// | ACI   | mcp-apps      | ACIMcpAppsView / MiniAppsStoreView (inside ACITabView)           |
// | ACI   | extensions    | ACIExtensionsView (inside ACITabView)                           |
// | ACI   | teach         | TeachView (inside ACITabView)                                   |
//
// Sub-screens inside Bots and ACI are reached by the host (later step) through
// BotRoute / ACITabView's own destination; if a host cannot route to one, it
// falls back to the mode's root tab, never a new screen.
//
// The shell hosts Shelves. The drawer mode icon and composer orb open it.
// Header New is removed; overview lists keep their bottom New action.

// MARK: - Modes

extension ShellMode {
    /// Contract id (web MODE_BUTTONS ids): chat, bot, code, browser.
    var hubId: String {
        switch self {
        case .home: return "chat"
        case .bots: return "bot"
        case .code: return "code"
        case .aci: return "browser"
        }
    }

    /// Label in the hub (contract "Modes": Home, Bots, Code, ACI).
    var hubLabel: String { label }

    /// Main item of the New orb (contract "New orb").
    var newMainLabel: String {
        switch self {
        case .home: return "New chat"
        case .bots, .code: return "New thread"
        case .aci: return "New session"
        }
    }
}

// MARK: - Sections

/// Where a hub section lands on iOS. `nil` target = hidden on iOS.
enum ModeHubTarget: Equatable, Sendable {
    case tab(ModeBarItem)
    case meetingsSheet
    case bots(BotsPart)
    case aci(ACIPart)

    enum BotsPart: String, Sendable { case mainBot, launchpad, groups, office, projects }
    enum ACIPart: String, Sendable { case computers, mcpApps, extensions, teach }

    /// The tab the host switches to first.
    var rootTab: ModeBarItem? {
        switch self {
        case .tab(let t): return t
        case .meetingsSheet: return nil
        case .bots: return .agents
        case .aci: return .aci
        }
    }
}

struct ModeHubSection: Identifiable, Equatable, Sendable {
    let id: String
    let mode: ShellMode
    let label: String
    let short: String
    /// Phosphor icon name from the contract.
    let phosphor: String
    /// SF Symbol used on iOS.
    let symbol: String
    /// "More" row text.
    let more: String
    /// Contract "Requires" (a feature flag name), nil when always available.
    let requires: String?
    /// nil = no existing iOS screen, section hidden.
    let target: ModeHubTarget?

    var isAvailableOnIOS: Bool { target != nil }
}

enum ModeHubSpec {
    static let sections: [ModeHubSection] = [
        // Home
        .init(id: "recents", mode: .home, label: "Recents", short: "Recents", phosphor: "ClockCounterClockwise", symbol: "clock.arrow.circlepath", more: "All chats", requires: nil, target: .tab(.chats)),
        .init(id: "projects", mode: .home, label: "Projects", short: "Projects", phosphor: "Folder", symbol: "folder", more: "All projects", requires: nil, target: .tab(.projects)),
        .init(id: "artifacts", mode: .home, label: "Artifacts", short: "Artifacts", phosphor: "Shapes", symbol: "square.on.circle", more: "All artifacts", requires: nil, target: .tab(.artifacts)),
        .init(id: "automation", mode: .home, label: "Automation Tasks", short: "Automations", phosphor: "Timer", symbol: "timer", more: "All tasks", requires: nil, target: .tab(.automation)),
        .init(id: "meetings", mode: .home, label: "Meetings", short: "Meetings", phosphor: "VideoCamera", symbol: "video", more: "Open Meetings", requires: "meetings", target: .meetingsSheet),
        .init(id: "customize", mode: .home, label: "Customize", short: "Customize", phosphor: "SlidersHorizontal", symbol: "slider.horizontal.3", more: "Open Customize", requires: nil, target: .tab(.customize)),
        .init(id: "freeweights", mode: .home, label: "Freeweights Studio", short: "Freeweights", phosphor: "Barbell", symbol: "dumbbell", more: "Open Freeweights", requires: nil, target: .tab(.models)),
        // Bots
        .init(id: "main-bot", mode: .bots, label: "Main bot", short: "Main bot", phosphor: "Star", symbol: "star", more: "Open main bot", requires: nil, target: .bots(.mainBot)),
        .init(id: "bots", mode: .bots, label: "Bots", short: "Bots", phosphor: "Robot", symbol: "cpu", more: "All bots", requires: nil, target: .bots(.launchpad)),
        .init(id: "groups", mode: .bots, label: "Group chats", short: "Groups", phosphor: "UsersThree", symbol: "person.3", more: "All group chats", requires: nil, target: .bots(.groups)),
        .init(id: "office", mode: .bots, label: "Office", short: "Office", phosphor: "Buildings", symbol: "building.2", more: "Open Office", requires: nil, target: .bots(.office)),
        .init(id: "bot-projects", mode: .bots, label: "Projects", short: "Projects", phosphor: "Folder", symbol: "folder", more: "All projects", requires: nil, target: .bots(.projects)),
        // Code
        .init(id: "sessions", mode: .code, label: "Sessions", short: "Sessions", phosphor: "TerminalWindow", symbol: "terminal", more: "All sessions", requires: nil, target: .tab(.code)),
        .init(id: "code-projects", mode: .code, label: "Projects", short: "Projects", phosphor: "Folder", symbol: "folder", more: "All projects", requires: nil, target: .tab(.projects)),
        .init(id: "code-artifacts", mode: .code, label: "Artifacts", short: "Artifacts", phosphor: "Shapes", symbol: "square.on.circle", more: "All artifacts", requires: nil, target: .tab(.artifacts)),
        .init(id: "code-automation", mode: .code, label: "Automation Tasks", short: "Automations", phosphor: "Timer", symbol: "timer", more: "All tasks", requires: nil, target: .tab(.automation)),
        // ACI
        .init(id: "computers", mode: .aci, label: "Computers", short: "Computers", phosphor: "Desktop", symbol: "desktopcomputer", more: "All computers", requires: nil, target: .aci(.computers)),
        .init(id: "mcp-apps", mode: .aci, label: "MCP Apps", short: "Apps", phosphor: "SquaresFour", symbol: "square.grid.2x2", more: "All apps", requires: nil, target: .aci(.mcpApps)),
        .init(id: "extensions", mode: .aci, label: "ACI Extensions", short: "Extensions", phosphor: "PuzzlePiece", symbol: "puzzlepiece.extension", more: "All extensions", requires: nil, target: .aci(.extensions)),
        .init(id: "teach", mode: .aci, label: "Teach", short: "Teach", phosphor: "Record", symbol: "record.circle", more: "Open Teach", requires: nil, target: .aci(.teach)),
    ]

    /// Sections shown for a mode on iOS (hidden ones dropped; `enabledFlags`
    /// gates `requires`).
    static func sections(for mode: ShellMode, enabledFlags: Set<String> = ["meetings"]) -> [ModeHubSection] {
        sections.filter {
            $0.mode == mode && $0.isAvailableOnIOS && ($0.requires.map(enabledFlags.contains) ?? true)
        }
    }

    /// A card previews 1 item; an open section lists up to 4, then the More row.
    static let cardPreviewItems = 1
    static let openListItems = 4

    // MARK: Timings (contract "Timings and motion")

    static let holdDuration: Double = 0.450
    static let holdPressScale: CGFloat = 0.94
    static let hoverIntent: Double = 0.120
    /// Orb -> pill spring: stiffness 380, damping 30 (mass 1).
    static let pillSpring = Animation.interpolatingSpring(mass: 1, stiffness: 380, damping: 30)
    static let lensSpring = Animation.interpolatingSpring(mass: 0.7, stiffness: 520, damping: 26)
    static let tickSpring = Animation.interpolatingSpring(mass: 0.6, stiffness: 560, damping: 32)
    static let sheetSpring = Animation.interpolatingSpring(mass: 0.9, stiffness: 420, damping: 38)

    // MARK: Haptics (contract "Haptics")

    enum Haptic { case holdComplete, lensMove, openItem, orbTap, newPick, cardPick }

    @MainActor static func fire(_ h: Haptic) {
        switch h {
        case .holdComplete, .openItem, .newPick: Haptics.medium()
        case .lensMove: Haptics.selection()
        case .orbTap, .cardPick: Haptics.light()
        }
    }

    // MARK: Events

    /// Contract events; the phone drawer's mode icon posts `openModeHub`.
    static let openModeHub = Notification.Name("allternit:open-mode-hub")
    static let modeArrive = Notification.Name("allternit:mode-arrive")
}
