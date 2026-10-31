import SwiftUI

/// Composer "+" sheet tool-access options (Claude iOS parity): when agent
/// tools load for a conversation. `auto` is the default — the agent decides.
enum ToolAccess: String, CaseIterable, Sendable {
    case auto
    case onDemand = "on_demand"
    case always

    var label: String {
        switch self {
        case .auto: return "Auto"
        case .onDemand: return "On demand"
        case .always: return "Always"
        }
    }

    /// One-line explainer under the segmented control (Claude's sheet copy:
    /// "Load when needed" vs "Ready from start").
    var explainer: String {
        switch self {
        case .auto: return "The agent decides which tools to load, when."
        case .onDemand: return "Load when needed"
        case .always: return "Ready from start"
        }
    }
}

/// Response style override selected from the "+" sheet's Style submenu
/// (ChatComposer.tsx parity: Formal / Creative / Technical).
enum ComposerResponseStyle: String, CaseIterable, Sendable {
    case formal
    case creative
    case technical

    var label: String {
        switch self {
        case .formal: return "Formal"
        case .creative: return "Creative"
        case .technical: return "Technical"
        }
    }

    /// Prefix injected into the next user message when the style is active
    /// (mirrors ChatComposer.tsx:1176-1180).
    var promptPrefix: String {
        switch self {
        case .formal: return "Respond in a formal, professional tone. "
        case .creative: return "Respond in a creative, imaginative style. "
        case .technical: return "Respond in a precise, technical manner. "
        }
    }
}

/// Composer tool options behind the "+" sheet's toggles — Web search,
/// Research, the Tool access mode, and Response style. Persisted app-wide
/// (UserDefaults), mirroring AgentModeStore's persistence pattern. Sent on
/// every agent-chat request as `metadata.tools` (see
/// AgentChatClient.AgentChatRequest).
@MainActor
final class ToolOptionsStore: ObservableObject {
    static let shared = ToolOptionsStore()

    @Published var webSearch: Bool {
        didSet { defaults.set(webSearch, forKey: Keys.webSearch) }
    }
    @Published var research: Bool {
        didSet { defaults.set(research, forKey: Keys.research) }
    }
    @Published var toolAccess: ToolAccess {
        didSet { defaults.set(toolAccess.rawValue, forKey: Keys.toolAccess) }
    }
    @Published var activeStyle: ComposerResponseStyle? {
        didSet { defaults.set(activeStyle?.rawValue, forKey: Keys.activeStyle) }
    }

    private let defaults: UserDefaults

    private enum Keys {
        static let webSearch = "allternit-tool-web-search"
        static let research = "allternit-tool-research"
        static let toolAccess = "allternit-tool-access"
        static let activeStyle = "allternit-tool-active-style"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        // Unset compositor toggle seeds from the Settings → Capabilities
        // "Web search" default; once the user touches the "+" sheet toggle
        // its own stored value wins.
        self.webSearch = defaults.object(forKey: Keys.webSearch) as? Bool
            ?? SettingsStore.shared.webSearchDefault
        self.research = defaults.bool(forKey: Keys.research)
        self.toolAccess = defaults.string(forKey: Keys.toolAccess)
            .flatMap(ToolAccess.init(rawValue:)) ?? .auto
        self.activeStyle = defaults.string(forKey: Keys.activeStyle)
            .flatMap(ComposerResponseStyle.init(rawValue:))
    }

    /// Wire shape for the agent-chat request body's `metadata.tools`.
    var optionsForSend: ToolOptions {
        ToolOptions(webSearch: webSearch, research: research, toolAccess: toolAccess.rawValue, style: activeStyle?.rawValue)
    }

    /// Enriches the user's message with active composer prefixes
    /// (web search marker + style prefix) before it is sent, while the UI
    /// bubble still shows the original text.
    func enrichedText(_ text: String) -> String {
        var parts: [String] = []
        if webSearch { parts.append("[web_search_enabled]") }
        if let activeStyle { parts.append(activeStyle.promptPrefix) }
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !parts.isEmpty else { return body }
        return (parts + [body]).joined(separator: " ")
    }

    /// Undoes `enrichedText` so a failed send hands back what the person typed.
    func strippedText(_ text: String) -> String {
        var body = text
        let prefixes = ["[web_search_enabled] "] + ComposerResponseStyle.allCases.map(\.promptPrefix)
        var changed = true
        while changed {
            changed = false
            for prefix in prefixes where body.hasPrefix(prefix) {
                body.removeFirst(prefix.count)
                body = body.trimmingCharacters(in: .whitespaces)
                changed = true
            }
        }
        return body
    }
}
