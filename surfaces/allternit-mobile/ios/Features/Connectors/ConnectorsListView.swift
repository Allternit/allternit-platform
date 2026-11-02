import SwiftUI
import SafariServices

/// The composer "+" button's destination — a real connector browser/manager
/// backed entirely by `GET/POST/DELETE /api/v1/connectors*`
/// (`ConnectorsClient`). No attachments, no tool-access toggles: those have
/// no real backend behind them (see the iOS UI polish plan) and aren't built
/// here. Every affordance in this screen calls a real endpoint and renders
/// exactly what it says back — including the honest "not mapped yet" states
/// the backend already returns for most of the 181-entry catalog.
struct ConnectorsListView: View {
    @Environment(\.dismiss) private var dismiss

    @State private var connectors: [Connector] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil
    @State private var searchText = ""

    /// Set while a connect/disconnect call for that id is in flight, so its
    /// row shows a spinner instead of the action button.
    @State private var pendingConnectorId: String? = nil

    /// `authorization_required` responses open here (SFSafariViewController);
    /// wrapped because `URL` isn't `Identifiable`.
    @State private var authorizationURL: IdentifiableURL? = nil
    /// Any other status/message the backend returned, shown verbatim.
    @State private var statusAlert: StatusAlert? = nil
    /// `api_key_required` responses prompt here instead of failing silently.
    @State private var apiKeyPrompt: Connector? = nil
    @State private var apiKeyInput = ""

    private let client = ConnectorsClient()

    private struct IdentifiableURL: Identifiable {
        let url: URL
        var id: String { url.absoluteString }
    }

    private struct StatusAlert: Identifiable {
        let title: String
        let message: String
        var id: String { title + message }
    }

    private var filtered: [Connector] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return connectors }
        return connectors.filter {
            $0.name.localizedCaseInsensitiveContains(query)
                || ($0.category ?? "").localizedCaseInsensitiveContains(query)
                || ($0.description ?? "").localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                searchBar
                Divider().background(Color("BorderSubtle"))
                ScrollView {
                    content
                }
                .refreshable { await loadConnectors() }
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Connectors")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
            }
            .task { await loadConnectors() }
            .sheet(item: $authorizationURL) { item in
                SafariView(url: item.url)
                    .onDisappear { Task { await loadConnectors() } }
            }
            .alert(item: $statusAlert) { alert in
                Alert(title: Text(alert.title), message: Text(alert.message), dismissButton: .default(Text("OK")))
            }
            .alert("API Key", isPresented: apiKeyPromptBinding, presenting: apiKeyPrompt) { connector in
                TextField("API key", text: $apiKeyInput)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button("Connect") {
                    let key = apiKeyInput
                    apiKeyInput = ""
                    Task { await performConnect(connector, apiKey: key) }
                }
                Button("Cancel", role: .cancel) { apiKeyInput = "" }
            } message: { connector in
                Text("Enter an API key for \(connector.name).")
            }
        }
    }

    private var apiKeyPromptBinding: Binding<Bool> {
        Binding(get: { apiKeyPrompt != nil }, set: { if !$0 { apiKeyPrompt = nil } })
    }

    // MARK: - Search

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            TextField("Search connectors", text: $searchText)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            if !searchText.isEmpty {
                Button(action: { searchText = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if isLoading && connectors.isEmpty {
            HStack {
                Spacer()
                ProgressView().padding(.top, 40)
                Spacer()
            }
        } else if let loadError, connectors.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load connectors",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await loadConnectors() } }
            )
            .frame(maxWidth: .infinity)
        } else if filtered.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: searchText.isEmpty ? "puzzlepiece.extension" : "magnifyingglass",
                title: searchText.isEmpty ? "No connectors available" : "No matches",
                message: searchText.isEmpty
                    ? "Add a connector to link external accounts and services."
                    : "No connectors match your search."
            )
            .frame(maxWidth: .infinity)
        } else {
            LazyVStack(spacing: 0) {
                ForEach(filtered) { connector in
                    connectorRow(connector)
                    Divider().background(Color("BorderSubtle")).padding(.leading, 68)
                }
            }
        }
    }

    private func connectorRow(_ connector: Connector) -> some View {
        HStack(spacing: 12) {
            AppLogoView(id: connector.id, name: connector.name, url: connector.baseURL, size: 40)

            VStack(alignment: .leading, spacing: 2) {
                Text(connector.name)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                if let category = connector.category {
                    Text(category)
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }
                if let description = connector.description {
                    Text(description)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                }
            }

            Spacer(minLength: 8)

            actionButton(connector)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    @ViewBuilder
    private func actionButton(_ connector: Connector) -> some View {
        if pendingConnectorId == connector.id {
            ProgressView().frame(width: 72, height: 30)
        } else if connector.isConnected {
            Button(action: { Task { await performDisconnect(connector) } }) {
                Text("Connected")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Theme.statusSuccess)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .background(Theme.statusSuccess.opacity(0.12))
                    .clipShape(Capsule())
            }
            .buttonStyle(.plain)
        } else if connector.connectable {
            Button(action: { Task { await performConnect(connector, apiKey: nil) } }) {
                Text("Connect")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(.black)
                    .padding(.horizontal, 14)
                    .frame(height: 30)
                    .background(Color("AccentPrimary"))
                    .clipShape(Capsule())
            }
            .buttonStyle(.plain)
        } else {
            Text("Unavailable")
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 12)
        }
    }

    // MARK: - Actions

    @MainActor
    private func loadConnectors() async {
        if connectors.isEmpty { isLoading = true }
        loadError = nil
        do {
            connectors = try await client.listConnectors()
        } catch is CancellationError {
            // View disappeared mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    @MainActor
    private func performConnect(_ connector: Connector, apiKey: String?) async {
        pendingConnectorId = connector.id
        defer { pendingConnectorId = nil }

        do {
            let response = try await client.connect(id: connector.id, apiKey: apiKey)
            if let urlString = response.authorizeURL, let url = URL(string: urlString) {
                authorizationURL = IdentifiableURL(url: url)
            } else if response.error == "api_key_required" {
                apiKeyPrompt = connector
            } else {
                await loadConnectors()
                if let message = response.message {
                    statusAlert = StatusAlert(title: response.status ?? "Connector", message: message)
                }
            }
        } catch {
            statusAlert = StatusAlert(title: "Couldn't connect \(connector.name)", message: error.localizedDescription)
        }
    }

    @MainActor
    private func performDisconnect(_ connector: Connector) async {
        pendingConnectorId = connector.id
        defer { pendingConnectorId = nil }
        do {
            try await client.disconnect(id: connector.id)
            await loadConnectors()
        } catch {
            statusAlert = StatusAlert(title: "Couldn't disconnect \(connector.name)", message: error.localizedDescription)
        }
    }
}

// The OAuth authorization-code flows (github/notion/slack today; more as
// `connectors.meta.json` maps them) open in the shared `SafariView` wrapper
// (Features/Settings/SafariView.swift). The backend's own loopback callback
// finishes the exchange server-side; dismissing the view just triggers a
// connector-list refresh to pick up the new status.
