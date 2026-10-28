import SwiftUI

/// "Show in rail" choices per mode (allternit-ai src/shell/rail/rail-tabs.ts):
/// which of the mode's rail tabs the drawer shows. Same keys and tab ids as
/// the web (there in localStorage, here in UserDefaults), so the contract
/// reads the same; choices are per device on both.
@MainActor
final class RailTabsStore: ObservableObject {
    static let shared = RailTabsStore()

    enum TabMode: String {
        case home, code, browser

        var storageKey: String {
            switch self {
            case .home: return "allternit-home-rail-tabs"
            case .code: return "allternit-code-rail-tabs"
            case .browser: return "allternit-browser-rail-tabs"
            }
        }
    }

    struct Tab: Identifiable, Hashable {
        let id: String
        let label: String
        let icon: String
    }

    /// The tabs the phone drawer can show per mode (PhoneDrawer PHONE_TABS,
    /// limited to the rows iOS has).
    static func tabs(for mode: TabMode) -> [Tab] {
        switch mode {
        case .home:
            return [
                Tab(id: "projects", label: "Projects", icon: "folder"),
                Tab(id: "artifacts-library", label: "Artifacts", icon: "doc.text"),
                Tab(id: "automation-tasks", label: "Automation Tasks", icon: "clock"),
                Tab(id: "customize", label: "Customize", icon: "slider.horizontal.3"),
            ]
        case .code:
            return [
                Tab(id: "projects", label: "Projects", icon: "folder"),
                Tab(id: "artifacts-library", label: "Artifacts", icon: "doc.text"),
                Tab(id: "code-automations", label: "Automation Tasks", icon: "clock"),
            ]
        case .browser:
            return [Tab(id: "mini-apps", label: "MCP Apps", icon: "square.grid.2x2")]
        }
    }

    @Published private var saved: [TabMode: [String: Bool]] = [:]
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        for mode in [TabMode.home, .code, .browser] {
            if let data = defaults.string(forKey: mode.storageKey)?.data(using: .utf8),
               let decoded = try? JSONDecoder().decode([String: Bool].self, from: data) {
                saved[mode] = decoded
            }
        }
    }

    /// Default: every tab shows (rail-tabs DEFAULTS for the phone's tabs).
    func isVisible(_ id: String, in mode: TabMode) -> Bool {
        saved[mode]?[id] ?? true
    }

    func toggle(_ id: String, in mode: TabMode) {
        var map = saved[mode] ?? [:]
        map[id] = !isVisible(id, in: mode)
        saved[mode] = map
        if let data = try? JSONEncoder().encode(map), let json = String(data: data, encoding: .utf8) {
            defaults.set(json, forKey: mode.storageKey)
        }
    }

    func anyHidden(in mode: TabMode) -> Bool {
        Self.tabs(for: mode).contains { !isVisible($0.id, in: mode) }
    }
}

/// The drawer's More row (PhoneDrawer MoreRow): choose which tabs show in
/// this mode's rail. A sheet of switches rather than a menu, so several can
/// change in one visit.
struct RailTabsSheet: View {
    let mode: RailTabsStore.TabMode
    @ObservedObject private var store = RailTabsStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(RailTabsStore.tabs(for: mode)) { tab in
                        Toggle(isOn: Binding(
                            get: { store.isVisible(tab.id, in: mode) },
                            set: { _ in store.toggle(tab.id, in: mode) }
                        )) {
                            Label(tab.label, systemImage: tab.icon)
                        }
                        .accessibilityIdentifier("railTab\(tab.id)")
                    }
                } header: {
                    Text("Show in rail")
                } footer: {
                    Text("Hidden tabs stay one tap away here.")
                }
            }
            .navigationTitle("More")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Customize (allternit-ai src/views/customize/CustomizeView.tsx): what the
/// assistant can use. Connectors and Webhooks are native screens; Skills,
/// Plugins and MCP servers read files on your computer, so they open the
/// matching web Settings page in-app (as Settings does today).
struct CustomizeSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @State private var safariURL: IdentifiableURL? = nil
    @State private var isConnectorsPresented = false

    private static let webBase = "https://ai.allternit.com/settings/"

    var body: some View {
        NavigationStack {
            List {
                if !auth.isSignedIn && !auth.isSkippingAuth {
                    FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to customize",
                                      message: "Skills, connectors and plugins belong to your account.",
                                      actionTitle: "Sign in", action: {
                                          dismiss()
                                          auth.isPresentingAuth = true
                                      })
                        .listRowBackground(Color.clear)
                } else {
                    Section {
                        webRow("Skills", icon: "book", section: "skills")
                        Button { isConnectorsPresented = true } label: {
                            row("Connectors", icon: "powerplug", trailing: "chevron.right")
                        }
                        .buttonStyle(.plain)
                        webRow("Plugins", icon: "puzzlepiece.extension", section: "plugins")
                    }
                    Section("More") {
                        webRow("MCP servers", icon: "cpu", section: "mcp")
                        NavigationLink { WebhooksSettingsView() } label: {
                            Label("Webhooks", systemImage: "arrow.triangle.branch")
                        }
                    }
                }
            }
            .navigationTitle("Customize")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .sheet(item: $safariURL) { SafariView(url: $0.url) }
            .sheet(isPresented: $isConnectorsPresented) { ConnectorsListView() }
        }
    }

    private func row(_ title: String, icon: String, trailing: String) -> some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .frame(width: 22)
                .foregroundColor(Color("TextSecondary"))
            Text(title).foregroundColor(Color("TextPrimary"))
            Spacer()
            Image(systemName: trailing)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .contentShape(Rectangle())
    }

    private func webRow(_ title: String, icon: String, section: String) -> some View {
        Button {
            if let url = URL(string: Self.webBase + section) { safariURL = IdentifiableURL(url: url) }
        } label: {
            row(title, icon: icon, trailing: "arrow.up.right")
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens on the web")
    }
}
