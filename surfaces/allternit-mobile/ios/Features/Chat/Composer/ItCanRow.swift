import SwiftUI

// MARK: - It can choices

/// The "It can" toggles of an artifact kind (registry `can`): Use my tools,
/// Keep its own data, Ask AI while open. A kind shows only the toggles it can
/// honour (`.none` in the registry hides one); the registry also sets each
/// toggle's starting value.
struct ItCanChoices: Equatable, Sendable {
    var connectors = false
    /// Names of the connected tools ticked under "Use my tools" (the web
    /// binding's `can.connectors`).
    var connectorNames: [String] = []
    var storage = false
    var ai = false

    init(entry: AgentModeEntry) {
        connectors = entry.connectors == .on
        storage = entry.storage == .on
        ai = entry.ai == .on
    }

    /// One line the session prompt carries, so the agent builds what was ticked.
    func promptLine(for entry: AgentModeEntry) -> String? {
        var can: [String] = []
        if entry.connectors != .none && connectors { can.append(connectorNames.isEmpty ? "use the person's connected tools" : "use the person's connected tools (\(connectorNames.joined(separator: ", ")))") }
        if entry.storage != .none && storage { can.append("keep its own data") }
        if entry.ai != .none && ai { can.append("ask AI while it is open") }
        guard !can.isEmpty else { return nil }
        return "## It can\n\nThe person wants this artifact to " + can.joined(separator: ", ") + "."
    }
}

/// The ticked choices per mode, kept while the app runs, plus the hand-off
/// from the deck's PDF mode to the library's file picker.
@MainActor
final class ItCanStore: ObservableObject {
    static let shared = ItCanStore()

    @Published private var byMode: [String: ItCanChoices] = [:]
    /// Set when PDF is picked in the deck; the Artifacts library opens its
    /// file picker once and clears it.
    var wantsPDFPick = false

    func choices(for entry: AgentModeEntry) -> ItCanChoices {
        byMode[entry.id] ?? ItCanChoices(entry: entry)
    }

    func set(_ choices: ItCanChoices, for entry: AgentModeEntry) {
        byMode[entry.id] = choices
    }

    func takePDFPick() -> Bool {
        defer { wantsPDFPick = false }
        return wantsPDFPick
    }
}

// MARK: - Launch

/// What a picked deck mode starts on Send. Modes that run on the chat's own
/// agent mode (Agent, Swarm) return nil and send as before.
enum DeckLaunch {
    /// The artifact kind a picked mode makes. App is a Build session.
    static func kind(for entry: AgentModeEntry) -> ArtifactStudioKind? {
        guard entry.launchType == .session else { return nil }
        if entry.id == "app" { return .build }
        return ArtifactStudioKind(rawValue: entry.session ?? entry.id)
    }
}

// MARK: - Row

/// The thin "It can" row above the composer card, shown only while an
/// artifact mode is picked (approved look: R2-deck-picked). Each chip is a
/// toggle; a mode shows only the ones it can honour.
struct ItCanRow: View {
    let entry: AgentModeEntry
    @ObservedObject private var store = ItCanStore.shared
    @StateObject private var tools = ItCanToolsModel()

    private var choices: ItCanChoices { store.choices(for: entry) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            togglesRow
            if entry.connectors != .none && choices.connectors { toolPicker }
        }
        .task { await tools.loadIfNeeded() }
    }

    /// The user's connected tools as brand logos to tick (web ItCan logo picker).
    @ViewBuilder private var toolPicker: some View {
        if tools.phase == .loading {
            ProgressView().controlSize(.small).padding(.horizontal, 8)
        } else if tools.tools.isEmpty {
            Text(tools.phase == .failed ? "Couldn't load your tools." : "No connected tools yet. Connect some in Settings.")
                .font(.system(size: 12)).foregroundColor(Color("TextTertiary")).padding(.horizontal, 8)
        } else {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 10) {
                    ForEach(tools.tools) { tool in
                        let on = choices.connectorNames.contains(tool.name)
                        Button {
                            Haptics.light()
                            var next = choices
                            if on { next.connectorNames.removeAll { $0 == tool.name } } else { next.connectorNames.append(tool.name) }
                            store.set(next, for: entry)
                        } label: {
                            AppLogoView(id: tool.id, name: tool.name, url: tool.baseURL, size: 32)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                                        .stroke(Color("TextPrimary"), lineWidth: on ? 2 : 0)
                                )
                                .opacity(on ? 1 : 0.55)
                                .frame(minWidth: 44, minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(tool.name)
                        .accessibilityValue(on ? "On" : "Off")
                        .accessibilityAddTraits(.isButton)
                    }
                }
                .padding(.horizontal, 8)
            }
        }
    }

    private var togglesRow: some View {
        HStack(spacing: 14) {
            Text("IT CAN")
                .font(.system(size: 10, weight: .bold))
                .tracking(0.6)
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            if entry.connectors != .none {
                toggle("Use my tools", on: choices.connectors) {
                    $0.connectors.toggle()
                    if !$0.connectors { $0.connectorNames = [] }
                }
            }
            if entry.storage != .none {
                toggle("Keep its own data", on: choices.storage) { $0.storage.toggle() }
            }
            if entry.ai != .none {
                toggle("Ask AI while open", on: choices.ai) { $0.ai.toggle() }
            }
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 32)
    }

    private func toggle(_ label: String, on: Bool, _ flip: @escaping (inout ItCanChoices) -> Void) -> some View {
        Button {
            Haptics.light()
            var next = choices
            flip(&next)
            store.set(next, for: entry)
        } label: {
            HStack(spacing: 6) {
                Capsule()
                    .fill(on ? Color("TextPrimary") : Color("BorderSubtle"))
                    .frame(width: 26, height: 16)
                    .overlay(alignment: on ? .trailing : .leading) {
                        Circle().fill(Color("BgPrimary")).frame(width: 12, height: 12).padding(2)
                    }
                Text(label)
                    .font(.system(size: 12, weight: on ? .bold : .regular))
                    .foregroundColor(Color(on ? "TextPrimary" : "TextSecondary"))
                    .fixedSize()
            }
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityValue(on ? "On" : "Off")
        .accessibilityAddTraits(.isButton)
    }
}

/// The connected tools the logo picker lists (real brand logos only: a tool
/// with no bundled logo and no site to fetch one from is left out).
@MainActor
final class ItCanToolsModel: ObservableObject {
    enum Phase { case idle, loading, loaded, failed }
    @Published private(set) var tools: [Connector] = []
    @Published private(set) var phase: Phase = .idle

    func loadIfNeeded() async {
        guard phase == .idle || phase == .failed else { return }
        phase = .loading
        do {
            let all = try await ConnectorsClient().listConnectors()
            tools = all.filter { $0.isConnected && (AppLogoCatalog.image(id: $0.id, name: $0.name, url: $0.baseURL) != nil || $0.baseURL != nil) }
            phase = .loaded
        } catch {
            phase = .failed
        }
    }
}
