import SwiftUI

// Connect-kit day-one steps (option A, Eoj 2026-11-13; web
// allternit-ai `setup/KitSetup.tsx`): Your AI as connect-kit rows in three
// small groups, then Your phone with "Add a computer" offered for later.
// Every row is the same ConnectionRowView and opens the same sheet as
// Settings › Connections, and reads the same `GET /api/v1/connections`.

/// The brain this setup picked: the connection id and the provider saved as the default.
struct SetupKitBrain: Equatable {
    var id: String
    var provider: String
    var modelId: String?
    var label: String
}

enum SetupKit {
    static let baseBrains = ["allternit", "claude", "copilot", "gemini", "grok", "kimi", "chatgpt"]
    static let moreBrains = ["api-key", "local"]

    /// The provider id a connected key saves as the default brain.
    static func provider(forKey id: String) -> String {
        switch id {
        case "openai-key": return "openai"
        case "anthropic-key": return "anthropic"
        case "google-key", "gemini": return "google"
        case "kimi-key": return "moonshot"
        default: return id.replacingOccurrences(of: "-key", with: "")
        }
    }

    /// "Brain: X · works on any device", for the result card.
    static func whereClause(_ id: String) -> String {
        guard let e = ConnectionsCatalog.entry(id) else { return "works on any device" }
        switch e.runsOn {
        case .computer: return "runs on your Mac or PC"
        case .runner: return "runs on Allternit, no computer needed"
        default: return "works on any device"
        }
    }

    static func outcome(_ brain: SetupKitBrain?) -> StepOutcome? {
        guard let brain else { return nil }
        var lines = ["Brain: \(brain.label) · \(whereClause(brain.id))"]
        if brain.id == "allternit" { lines.insert("Included in your account · nothing to set up", at: 0) }
        else if brain.id.hasSuffix("-key") || brain.id == "gemini" { lines.insert("Checked your key", at: 0) }
        else { lines.insert("Signed in with a code", at: 0) }
        return StepOutcome(status: .connected, label: brain.label, detail: lines)
    }
}

/// Step 1: Your AI.
struct SetupKitAIStep: View {
    @ObservedObject var store: ConnectionsStore
    let brain: SetupKitBrain?
    /// The first Allternit Cloud model, saved when Allternit is picked.
    let allternitModel: String?
    let onPick: (SetupKitBrain) -> Void

    @State private var showMore = false
    @State private var sheet: ConnSheet?
    @State private var page: ConnPage?
    @State private var showConnectors = false

    private var ids: [String] {
        showMore || brain.map { SetupKit.moreBrains.contains($0.id) || $0.id.hasSuffix("-key") } == true
            ? SetupKit.baseBrains + SetupKit.moreBrains : SetupKit.baseBrains
    }

    var body: some View {
        let entries = ids.compactMap(ConnectionsCatalog.entry)
        let anyDevice = entries.filter { $0.runsOn != .computer && store.isAvailable($0.id) }
        let computer = entries.filter { $0.runsOn == .computer }
        let soon = entries.filter { $0.runsOn != .computer && !store.isAvailable($0.id) }
        VStack(alignment: .leading, spacing: 8) {
            ForEach([("Works on any device", anyDevice), ("Needs your computer", computer), ("Coming soon", soon)].filter { !$0.1.isEmpty }, id: \.0) { title, rows in
                Text(title.uppercased())
                    .font(SetupPalette.mono(10.5, .semibold)).tracking(0.8)
                    .foregroundStyle(SetupPalette.mut)
                    .padding(.leading, 2)
                    .padding(.top, 4)
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.element.id) { i, e in
                        if i > 0 { Rectangle().fill(SetupPalette.line).frame(height: 0.5) }
                        row(e)
                    }
                }
                .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
                .accessibilityElement(children: .contain)
                .accessibilityLabel(title)
            }
            if !showMore && !(brain.map { SetupKit.moreBrains.contains($0.id) || $0.id.hasSuffix("-key") } ?? false) {
                Button("More ways: an API key, a local brain") { showMore = true }
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(SetupPalette.sub)
                    .padding(.top, 2)
            }
        }
        .sheet(item: $sheet, onDismiss: { Task { await store.load() } }) { s in
            ConnectSheetView(sheet: s, store: store, onConnected: { id in connected(id) }, onFallback: { f in fallback(f) })
        }
        .sheet(item: $page, onDismiss: { Task { await store.load() } }) { p in
            NavigationStack {
                ConnPageView(page: p)
                    .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { page = nil } } }
            }
        }
        .sheet(isPresented: $showConnectors) { ConnectorsListView() }
    }

    private func row(_ e: ConnEntry) -> some View {
        let r = store.row(e.id)
        let isBrain = brain.map { $0.id == e.id || (e.id == "api-key" && $0.id.hasSuffix("-key")) || (e.id == "gemini" && $0.id == "google-key") } ?? false
        let custom: AnyView? = isBrain
            ? AnyView(Image(systemName: "checkmark.circle.fill").foregroundStyle(SetupPalette.ok).font(.system(size: 18)).accessibilityLabel("Your brain"))
            : r.state == .on ? AnyView(Button("Use this") { use(e.id) }.buttonStyle(ConnQuietButtonStyle())) : nil
        return ConnectionRowView(entry: e.id == "allternit" ? ConnEntry(id: e.id, group: e.group, name: "Allternit", logo: e.logo, ways: e.ways) : e,
                                 store: store, tag: isBrain ? "Your brain" : nil, customAction: custom, compact: true) { action in
            switch action {
            case .fallback(let f): fallback(f)
            default:
                switch ConnRoute.forConnect(e.id, store: store) {
                case .sheet(let s): sheet = s
                case .page(let p): page = p
                case .connectors: showConnectors = true
                default: break
                }
            }
        }
    }

    private func fallback(_ id: String) {
        if id == "this-mac" { page = .pairComputer; return }
        if id == "github" { showConnectors = true; return }
        if id.hasSuffix("-key") || id == "api-key" { showMore = true; sheet = .keys(only: id == "api-key" ? nil : id); return }
    }

    /// A row that is already connected becomes the brain with one tap.
    private func use(_ id: String) {
        if id == "api-key" {
            guard let key = ConnectionsCatalog.keyProviders.first(where: { store.state($0.id) == .on }) else { return }
            connected(key.id)
            return
        }
        connected(id)
    }

    private func connected(_ id: String) {
        let found = store.row(id).found
        switch id {
        case "allternit":
            onPick(SetupKitBrain(id: id, provider: "allternit", modelId: allternitModel, label: "Allternit Cloud"))
        case "chatgpt":
            onPick(SetupKitBrain(id: id, provider: "codex-cli", modelId: nil, label: found ?? "ChatGPT"))
        default:
            if id.hasSuffix("-key") {
                let name = ConnectionsCatalog.keyProvider(id)?.name ?? id
                onPick(SetupKitBrain(id: id, provider: SetupKit.provider(forKey: id), modelId: nil, label: "\(name) key"))
            } else if id == "gemini" {
                onPick(SetupKitBrain(id: "google-key", provider: "google", modelId: nil, label: "Gemini · AI Studio key"))
            }
        }
    }
}

/// Step 2: Your phone. On the iPhone itself the phone is already here, so it
/// shows as connected; a computer is offered for later.
struct SetupKitPhoneStep: View {
    @ObservedObject var store: ConnectionsStore
    @State private var showPair = false

    private static let addComputer = ConnEntry(id: "this-mac", group: .computers, name: "Add a computer", logo: .symbol("laptopcomputer"), ways: [.install, .pair], runsOn: .computer)

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("LATER, IF YOU NEED IT")
                .font(SetupPalette.mono(10.5, .semibold)).tracking(0.8)
                .foregroundStyle(SetupPalette.mut)
                .padding(.leading, 2)
                .padding(.top, 10)
            VStack(alignment: .leading, spacing: 0) {
                ConnectionRowView(entry: store.state("this-mac") == .on
                                  ? ConnEntry(id: "this-mac", group: .computers, name: "Your computer", logo: Self.addComputer.logo, ways: Self.addComputer.ways, runsOn: .computer)
                                  : Self.addComputer, store: store,
                                  customAction: store.state("this-mac") == .on ? AnyView(EmptyView()) : nil, compact: true) { action in
                    if case .disconnect = action { return }
                    showPair = true
                }
                Text("Only when a task needs one: clicking apps, using a browser. Chat works without it.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(SetupPalette.sub)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
            }
            .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
            .accessibilityIdentifier("setup-add-computer")
        }
        .sheet(isPresented: $showPair, onDismiss: { Task { await store.load() } }) {
            NavigationStack {
                DevicePairingView()
                    .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { showPair = false } } }
            }
        }
    }
}
