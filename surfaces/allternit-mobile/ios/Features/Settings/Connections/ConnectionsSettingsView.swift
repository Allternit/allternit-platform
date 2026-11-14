import SwiftUI

/// A settings page a row opens (the existing flow for that kind of connection).
enum ConnPage: String, Identifiable, Hashable {
    case voice, phoneControl, pairComputer, cloudComputer, billing, gizzi
    var id: String { rawValue }
}

/// Where a row's tap goes. Shared by Settings › Connections and first-run setup.
enum ConnRoute {
    case sheet(ConnSheet)
    case page(ConnPage)
    case connectors
    case channels
    case none

    /// Rows that live in their own settings page show a chevron.
    @MainActor
    static func opensPage(_ entry: ConnEntry, store: ConnectionsStore) -> Bool {
        ["openai-voice", "elevenlabs", "phone-control", "this-mac", "server", "gizzi"].contains(entry.id)
            || entry.group == .apps || entry.group == .channels
            // No cloud computer yet: the row says "See plans" instead.
            || (entry.id == "cloud-computer" && store.state(entry.id) == .on)
    }

    @MainActor
    static func forConnect(_ id: String, store: ConnectionsStore) -> ConnRoute {
        if id == "api-key" { return .sheet(.keys(only: nil)) }
        if id == "gemini" { return .sheet(.keys(only: "google-key")) }
        if id.hasSuffix("-key") { return .sheet(.keys(only: id)) }
        switch id {
        case "openai-voice", "elevenlabs": return .page(.voice)
        case "phone-control": return .page(.phoneControl)
        case "this-mac", "server": return .page(.pairComputer)
        case "cloud-computer": return .page(store.state(id) == .on ? .cloudComputer : .billing)
        case "gizzi": return .page(.gizzi)
        case "local": return .sheet(.onComputer(id))
        // Signs in on this iPhone (contract §9.9), with or without a computer.
        case "chatgpt": return .sheet(.chatgptDevice)
        case "phone", "allternit": return .none
        default: break
        }
        guard let entry = ConnectionsCatalog.entry(id) else { return .none }
        switch entry.group {
        case .apps: return .connectors
        case .channels: return .channels
        default: return entry.ways.contains(.code) ? .sheet(.login(id)) : .none
        }
    }
}

/// Settings › Connections (connect kit option A). One page for every
/// connection: what needs you, then one card per group with the same row,
/// the same four status words and the same sheet. Brains are split into
/// "Works on any device", "Needs your computer" and "Coming soon".
struct ConnectionsSettingsView: View {
    @ObservedObject private var store = ConnectionsStore.shared
    @State private var sheet: ConnSheet?
    @State private var page: ConnPage?
    @State private var showConnectors = false
    @State private var showChannels = false
    @State private var filter: Filter = .all
    @State private var query = ""
    @State private var actionError: String?

    enum Filter: String, CaseIterable { case all, on, needs }

    var body: some View {
        PhoneSettingsPage(title: "Connections") {
            SettingsDescription(text: "Everything Allternit uses, in one place. Each one connects the same way and shows the same status.")
            if let loadError = store.loadError {
                SettingsErrorBanner(message: "Couldn't load your connections. \(loadError)")
            }
            if let actionError {
                SettingsErrorBanner(message: actionError)
            }
            needsBanner
            filterBar
            if !store.loaded && store.isLoading {
                HStack { Spacer(); ProgressView(); Spacer() }.padding(.vertical, 24)
            }
            ForEach(ConnGroup.allCases, id: \.self) { group in
                groupSection(group)
            }
            if visible.isEmpty {
                Text(query.isEmpty ? "Nothing here yet." : "Nothing matches “\(query)”.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 12)
            }
        }
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .automatic), prompt: "Search connections")
        .refreshable { await store.load() }
        .task { await store.load() }
        .sheet(item: $sheet, onDismiss: { Task { await store.load() } }) { s in
            ConnectSheetView(sheet: s, store: store, onFallback: { f in handleFallback(f) })
        }
        .sheet(isPresented: $showConnectors, onDismiss: { Task { await store.load() } }) { ConnectorsListView() }
        .sheet(isPresented: $showChannels, onDismiss: { Task { await store.load() } }) { GiveBotPhoneSheet(botId: nil) }
        .navigationDestination(item: $page) { p in ConnPageView(page: p) }
        .accessibilityIdentifier("connections-page")
    }

    // MARK: - Filtering

    private var visible: [ConnEntry] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return ConnectionsCatalog.entries.filter { e in
            let s = store.state(e.id)
            if filter == .on && s != .on { return false }
            if filter == .needs && s != .needs { return false }
            return q.isEmpty || e.name.lowercased().contains(q) || (e.blurb ?? "").lowercased().contains(q)
                || e.group.title.lowercased().contains(q)
        }
    }

    private var needs: [ConnEntry] { ConnectionsCatalog.entries.filter { store.state($0.id) == .needs } }

    @ViewBuilder
    private var needsBanner: some View {
        let needs = needs
        if !needs.isEmpty {
            HStack(spacing: 10) {
                Image(systemName: "exclamationmark.triangle.fill").foregroundColor(Color(hex: "#E8A33A")).accessibilityHidden(true)
                Text("\(needs.count) needs you: " + needs.map { "\($0.name) · \(store.row($0.id).reason ?? ConnectionsCatalog.reasonText(store.row($0.id).reasonCode, name: $0.name) ?? "")" }.joined(separator: ", "))
                    .font(.system(size: 13.5))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button("Fix") { act(needs[0], .fix) }.buttonStyle(ConnPrimaryButtonStyle())
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
            .padding(.bottom, 12)
            .accessibilityElement(children: .contain)
        }
    }

    private var filterBar: some View {
        let count = needs.count
        return Picker("Show", selection: $filter) {
            Text("All").tag(Filter.all)
            Text("Connected").tag(Filter.on)
            Text(count > 0 ? "Needs you (\(count))" : "Needs you").tag(Filter.needs)
        }
        .pickerStyle(.segmented)
        .padding(.bottom, 4)
    }

    // MARK: - Groups

    /// Option A's brain sections. "Coming soon" holds every brain this account
    /// can't use yet that doesn't need a computer (the server says which).
    private func brainSections(_ rows: [ConnEntry]) -> [(String, [ConnEntry])] {
        let anyDevice = rows.filter { $0.runsOn != .computer && store.isAvailable($0.id) }
        let computer = rows.filter { $0.runsOn == .computer }
        let soon = rows.filter { $0.runsOn != .computer && !store.isAvailable($0.id) }
        return [("Works on any device", anyDevice), ("Needs your computer", computer), ("Coming soon", soon)].filter { !$0.1.isEmpty }
    }

    @ViewBuilder
    private func groupSection(_ group: ConnGroup) -> some View {
        let rows = visible.filter { $0.group == group }
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text(group.title)
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Spacer()
                    Text("\(rows.filter { store.state($0.id) == .on }.count) connected")
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(.top, 20)
                Text(group.blurb)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 2)
                    .padding(.bottom, 8)
                if group != .brains, rows.count > 1, rows.allSatisfy({ !store.isAvailable($0.id) && store.row($0.id).fallbacks.first == "this-mac" }) {
                    collapsedNeedsComputer(rows)
                } else {
                    PhoneSettingsCard {
                        if group == .brains {
                            let sections = brainSections(rows)
                            ForEach(Array(sections.enumerated()), id: \.element.0) { si, section in
                                if si > 0 { PhoneCardDivider() }
                                Text(section.0.uppercased())
                                    .font(.system(size: 11, weight: .semibold))
                                    .tracking(0.6)
                                    .foregroundColor(Color("TextTertiary"))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 16)
                                    .padding(.vertical, 6)
                                    .background(Color("TextPrimary").opacity(0.03))
                                rowList(section.1)
                            }
                        } else {
                            rowList(rows)
                        }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func rowList(_ rows: [ConnEntry]) -> some View {
        ForEach(Array(rows.enumerated()), id: \.element.id) { i, e in
            if i > 0 { PhoneCardDivider() }
            ConnectionRowView(entry: e, store: store, opensPage: ConnRoute.opensPage(e, store: store) && store.isAvailable(e.id)) { action in
                act(e, action)
            }
        }
    }

    /// Every row in the group runs on a computer and there is none: one card, one button.
    private func collapsedNeedsComputer(_ rows: [ConnEntry]) -> some View {
        HStack(spacing: 12) {
            HStack(spacing: -6) {
                ForEach(rows.prefix(4)) { e in
                    ConnLogoView(logo: e.logo, name: e.name, size: 28)
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color("FillNeutral"), lineWidth: 2))
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(rows.map(\.name).joined(separator: ", "))
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text("These run on a computer. Add one when you want them.")
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Add a computer") { page = .pairComputer }.buttonStyle(ConnPrimaryButtonStyle())
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-group-collapsed")
    }

    // MARK: - Actions

    private func act(_ entry: ConnEntry, _ action: ConnRowAction) {
        actionError = nil
        switch action {
        case .fallback(let f):
            handleFallback(f)
        case .disconnect:
            Task {
                do {
                    try await store.disconnect(entry.id)
                } catch let unconfirmed as ChatGPTSignOutUnconfirmed {
                    actionError = unconfirmed.errorDescription
                } catch {
                    actionError = "Couldn't disconnect \(entry.name). \(ConnectionsClient.describe(error))"
                }
            }
        case .connect, .fix, .open:
            route(ConnRoute.forConnect(entry.id, store: store))
        }
    }

    private func handleFallback(_ id: String) {
        if id == "cloud-computer" { page = .billing; return }
        route(ConnRoute.forConnect(id, store: store))
    }

    private func route(_ r: ConnRoute) {
        switch r {
        case .sheet(let s): sheet = s
        case .page(let p): page = p
        case .connectors: showConnectors = true
        case .channels: showChannels = true
        case .none: break
        }
    }
}

/// The existing settings pages a row opens.
struct ConnPageView: View {
    let page: ConnPage

    var body: some View {
        switch page {
        case .voice: VoiceSettingsPage()
        case .phoneControl: AppPhonesSettingsView()
        case .pairComputer: DevicePairingView()
        case .cloudComputer: CloudInstancesManagerView()
        case .billing: ComputeBillingView(title: "Billing")
        case .gizzi: ProductsSettingsView(part: .gizziCode)
        }
    }
}
