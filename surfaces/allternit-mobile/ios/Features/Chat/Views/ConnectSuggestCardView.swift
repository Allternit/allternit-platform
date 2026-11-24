import SwiftUI

/// The `connector_suggest` card (Primitives Wave D, D2): the connectors the
/// agent offered, why, and Connect on each. The card carries ids, names and
/// groups only, never a link or code: Connect opens the app's own Connect
/// flow for that id (ConnRoute, the same routing Settings › Connections and
/// first-run setup use), so the person connects it themselves.
struct ConnectSuggestCardView: View {
    let suggestion: AgentConnectorSuggestion

    @ObservedObject private var store = ConnectionsStore.shared
    @State private var sheet: ConnSheet?
    @State private var page: ConnPage?
    @State private var showConnectors = false
    @State private var showChannels = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .foregroundColor(Color("AccentPrimary"))
                    .accessibilityHidden(true)
                Text(suggestion.connectors.count == 1 ? "Suggested connection" : "Suggested connections")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))

            if let reason = suggestion.reason {
                Text(reason)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(spacing: 0) {
                ForEach(Array(suggestion.connectors.enumerated()), id: \.element.id) { index, connector in
                    if index > 0 {
                        Divider().background(Color("BorderSubtle"))
                    }
                    row(connector)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
        .accessibilityLabel(suggestion.reason.map { "Suggested connections. \($0)" } ?? "Suggested connections")
        .sheet(item: $sheet, onDismiss: reload) { s in
            ConnectSheetView(sheet: s, store: store)
        }
        .sheet(item: $page, onDismiss: reload) { p in
            NavigationStack {
                ConnPageView(page: p)
                    .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { page = nil } } }
            }
        }
        .sheet(isPresented: $showConnectors, onDismiss: reload) { ConnectorsListView() }
        .sheet(isPresented: $showChannels, onDismiss: reload) { GiveBotPhoneSheet(botId: nil) }
    }

    private func row(_ connector: AgentConnectorSuggestion.Connector) -> some View {
        let state = effectiveState(connector)
        return HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(connector.name)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let group = connector.group {
                    Text(group)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            switch state {
            case "connected":
                Label("Connected", systemImage: "checkmark.circle.fill")
                    .font(.caption.weight(.semibold))
                    .foregroundColor(Theme.statusSuccess)
            case "unavailable":
                Text("Not available here")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            default:
                Button { connect(connector.id) } label: {
                    Text("Connect").font(.caption.weight(.semibold))
                }
                .buttonStyle(.bordered)
                .frame(minHeight: 44)
                .accessibilityLabel("Connect \(connector.name)")
            }
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }

    /// The card's state, updated by what this app knows now (a connection
    /// made from the card turns its row to Connected).
    private func effectiveState(_ connector: AgentConnectorSuggestion.Connector) -> String? {
        if ConnectionsCatalog.entry(connector.id) != nil, store.state(connector.id) == .on {
            return "connected"
        }
        return connector.state
    }

    private func connect(_ id: String) {
        switch ConnRoute.forConnect(id, store: store) {
        case .sheet(let s): sheet = s
        case .page(let p): page = p
        case .channels: showChannels = true
        // Catalog apps (Gmail, Linear, …) connect in the connectors browser.
        case .connectors, .none: showConnectors = true
        }
    }

    private func reload() {
        Task { await store.load() }
    }
}
