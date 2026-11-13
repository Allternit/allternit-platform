import SwiftUI

/// Settings → Self-hosted server: connect this iPhone to an Allternit Agents
/// Server you run yourself (HTTPS and an `ask_…` API key), see its status and
/// agents, and start a session on it. Lives inside Settings; no rail tab.
struct SelfHostedSettingsView: View {
    @ObservedObject private var store = SelfHostedServerStore.shared
    @State private var urlText = ""
    @State private var keyText = ""
    @State private var seeded = false
    /// True while the user types a new key. Otherwise a saved key shows as a
    /// "Saved in Keychain" row: a filled SecureField that leaves the screen makes
    /// iOS offer "Save Password?" (as an allternit.com password), which is wrong
    /// for a server key and covered the session screen.
    @State private var editingKey = false
    private enum Field { case url, key }
    @FocusState private var focused: Field?

    var body: some View {
        PhoneSettingsPage(title: "Self-hosted server") {
            SettingsDescription(text: "Run your own Allternit Agents Server and use its agents from this iPhone. Your server makes no calls to Allternit. The address must be HTTPS, and your API key stays in this iPhone’s Keychain.")

            SettingsSectionHeading(text: "Connect")
            SettingsCardBox {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Server address").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        .accessibilityHidden(true)
                    TextField("https://agents.example.com", text: $urlText)
                        .modifier(SHFieldStyle())
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .focused($focused, equals: .url)
                        .submitLabel(.next)
                        .onSubmit { focused = .key }
                        .accessibilityLabel("Server address")
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text("API key").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        .accessibilityHidden(true)
                    if showsKeyField {
                        SecureField("ask_…", text: $keyText)
                            .modifier(SHFieldStyle())
                            .textContentType(.oneTimeCode)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .focused($focused, equals: .key)
                            .submitLabel(.go)
                            .onSubmit { if canConnect { connect() } }
                            .accessibilityLabel("API key")
                    } else {
                        HStack {
                            Text("Saved in Keychain (ends \(String(store.apiKey.suffix(4))))")
                                .font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                            Spacer()
                            Button("Change") { editingKey = true; focused = .key }
                                .font(.system(size: 15, weight: .medium))
                                .foregroundColor(Color("TextPrimary"))
                        }
                        .padding(.horizontal, 13)
                        .frame(minHeight: 44)
                        .background(Color("FillNeutral"))
                        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .accessibilityElement(children: .combine)
                        .accessibilityLabel("API key saved in Keychain")
                    }
                }
                HStack(spacing: 10) {
                    Button(store.isConfigured ? "Save and test" : "Connect") { connect() }
                        .buttonStyle(SettingsPrimaryButtonStyle())
                        .disabled(store.phase == .connecting || !canConnect)
                    if store.isConfigured {
                        Button("Disconnect") {
                            store.disconnect(); urlText = ""; keyText = ""; editingKey = false
                        }
                        .buttonStyle(SettingsQuietButtonStyle())
                    }
                }
            }

            stateView

            if store.phase == .connected {
                statusCard
                agentsSection
                NavigationLink {
                    SelfHostedSessionView()
                } label: {
                    HStack {
                        Text("Start a session").font(.system(size: 16, weight: .medium))
                        Spacer()
                        Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 16)
                    .frame(minHeight: 50)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                }
                .disabled(store.agents.isEmpty || store.environments.isEmpty)
                .padding(.top, 4)
            }
        }
        .onAppear {
            guard !seeded else { return }
            seeded = true
            urlText = store.serverURL
            if store.isConfigured { Task { await store.reload() } }
        }
    }

    private var showsKeyField: Bool { editingKey || !store.isConfigured }

    private var canConnect: Bool {
        !urlText.isEmpty && (showsKeyField ? !keyText.isEmpty : true)
    }

    private func connect() {
        focused = nil
        let key = showsKeyField ? keyText : store.apiKey
        Task {
            await store.connect(urlText: urlText, key: key)
            // Saved: drop the typed key so no filled secure field stays on screen.
            if store.phase == .connected {
                keyText = ""; editingKey = false
            }
        }
    }

    // MARK: States

    @ViewBuilder private var stateView: some View {
        switch store.phase {
        case .notConfigured:
            SettingsDescription(text: "Not connected. Enter your server’s HTTPS address and an API key (it starts with ask_) from your server’s admin.")
        case .connecting:
            HStack(spacing: 10) {
                ProgressView()
                Text("Connecting…").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            }
            .padding(.bottom, 14)
            .accessibilityElement(children: .combine)
        case .failed(let error):
            SettingsErrorBanner(message: error.message)
        case .connected:
            HStack(spacing: 8) {
                Circle().fill(Theme.statusSuccess).frame(width: 8, height: 8)
                Text("Connected").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
            }
            .padding(.bottom, 14)
            .accessibilityElement(children: .combine)
        }
    }

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionHeading(text: "Server")
            SettingsCardBox {
                row("Version", store.info?.version ?? "–")
                row("Provider", store.info?.provider ?? "–")
                row("Workers online", "\(store.workersOnline)")
                row("Queue depth", "\(store.queueDepth)")
                if store.workersOnline == 0 {
                    Text(store.environments.isEmpty
                         ? "This server has no environments yet. Create one on the server, then pull to reconnect."
                         : "No workers online. Chats that only use the model work, but a turn that runs a tool will wait until a worker starts for its environment.")
                        .font(.system(size: 13))
                        .foregroundColor(Theme.statusWarning)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack {
            Text(title).font(.system(size: 15)).foregroundColor(Color("TextSecondary"))
            Spacer()
            Text(value).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
        }
        .accessibilityElement(children: .combine)
    }

    private var agentsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionHeading(text: "Agents")
            if store.agents.isEmpty {
                SettingsDescription(text: "No agents on this server yet. Create one with the server’s API or the CLI, then reconnect.")
            } else {
                PhoneSettingsCard {
                    ForEach(Array(store.agents.enumerated()), id: \.element.id) { index, agent in
                        if index > 0 { PhoneCardDivider() }
                        HStack(spacing: 12) {
                            Image(systemName: "sparkles").frame(width: 22).foregroundColor(Color("TextPrimary"))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(agent.name).font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                                Text(agent.model).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                            }
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 16)
                        .frame(minHeight: 50, alignment: .leading)
                        .accessibilityElement(children: .combine)
                    }
                }
                .padding(.bottom, 16)
            }
        }
    }
}

/// 44pt bordered field, as the other Settings forms.
struct SHFieldStyle: ViewModifier {
    func body(content: Content) -> some View {
        content
            .font(.system(size: 16))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 13)
            .frame(minHeight: 44)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(Color("BorderStrong"), lineWidth: 1)
            )
    }
}
