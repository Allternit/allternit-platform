import SwiftUI

/// Settings, grouped like the PWA's phone Settings (allternit-ai
/// src/fabric-session/phone/screens/SettingsScreen.tsx + settings.config.ts
/// `PHONE_SETTINGS_GROUPS`): an account card, then Account · Assistant ·
/// Appearance · Bots & agents · Computers · Advanced, and Log out at the
/// bottom. Operator pages sit behind the collapsed "Advanced" row, as on the
/// PWA. Pages without a native screen yet open the same page on the web.
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var authManager: AuthManager
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var settings = SettingsStore.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    /// Response style + custom instructions (`GET/PUT /api/v1/agent-preferences`).
    @ObservedObject private var preferences = PreferencesStore.shared

    /// Operator pages (models, infrastructure…) behind the Advanced row.
    @State private var showOperator = false
    @State private var isConnectorsPresented = false
    @State private var safariURL: IdentifiableURL? = nil
    @State private var signOutError: String? = nil

    // DEBUG deep links (`-open-settings-*`) push these.
    @State private var isMemoryPresented = false
    @State private var isPlatformPresented = false
    #if DEBUG
    @State private var isBrainSpikePresented = false
    #endif

    private static let webBase = "https://ai.allternit.com/settings/"

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                List {
                    accountCard
                    accountSection
                    assistantSection
                    appearanceSection
                    botsSection
                    computersSection
                    SettingsIntroSection(onDismissSettings: { dismiss() })
                    advancedSection
                        .id("aboutSection")
                    signOutSection
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
                .background(Color("BgPrimary"))
                .navigationTitle("Settings")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button(action: { dismiss() }) {
                            Image(systemName: "xmark")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                        }
                        .accessibilityLabel("Close settings")
                    }
                }
                .navigationDestination(isPresented: $isMemoryPresented) { MemorySettingsView() }
                .navigationDestination(isPresented: $isPlatformPresented) { PlatformSettingsView() }
                #if DEBUG
                .navigationDestination(isPresented: $isBrainSpikePresented) { BrainSpikeView() }
                #endif
                .sheet(item: $safariURL) { item in SafariView(url: item.url) }
                .sheet(isPresented: $isConnectorsPresented) { ConnectorsListView() }
                .task {
                    PreferencesStore.shared.fetchIfNeeded()
                    UsageStore.shared.fetchUsageIfNeeded()
                    if authManager.isSignedIn { AccountPlanStore.shared.load() }
                }
                .onAppear {
                    #if DEBUG
                    if launchArgumentEnabled("open-settings-memory") { isMemoryPresented = true }
                    if launchArgumentEnabled("open-settings-platform") { isPlatformPresented = true }
                    if launchArgumentEnabled("open-settings-data") {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                            withAnimation { proxy.scrollTo("aboutSection", anchor: .bottom) }
                        }
                    }
                    if launchArgumentEnabled("open-settings-brain-spike") { isBrainSpikePresented = true }
                    #endif
                }
            }
        }
    }

    // MARK: - Account card

    /// Name, plan and how much is left; opens Account.
    private var accountCard: some View {
        Section {
            NavigationLink {
                AccountProfileView(onClose: { dismiss() })
            } label: {
                HStack(spacing: 12) {
                    AccountAvatar(size: 46)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(authManager.isSignedIn ? (authManager.fullName ?? authManager.displayName) : "Not signed in")
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(accountDetail)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }

    private var accountDetail: String {
        guard authManager.isSignedIn else { return "Sign in to sync across devices" }
        var parts: [String] = []
        if let name = plan.planName ?? usageStore.planLabel { parts.append(name) }
        if let percent = usageStore.percentUsed {
            let left = max(0, 100 - Int(percent.rounded()))
            parts.append("\(left)% left · \(usageStore.periodLabel.lowercased())")
        }
        if parts.isEmpty, let email = authManager.primaryEmail { parts.append(email) }
        return parts.joined(separator: " · ")
    }

    // MARK: - Account

    private var accountSection: some View {
        Section {
            NavigationLink { ComputeBillingView(title: "Billing") } label: {
                SettingsRowLabel(title: "Billing", systemImage: "creditcard", value: plan.planName)
            }
            NavigationLink { ComputeBillingView(title: "Usage") } label: {
                SettingsRowLabel(title: "Usage", systemImage: "gauge.with.dots.needle.33percent",
                                 value: usageStore.percentText.map { "\($0) used" })
            }
            NavigationLink { NotificationsSettingsView() } label: {
                SettingsRowLabel(title: "Notifications", systemImage: "bell")
            }
            NavigationLink { PrivacySettingsView() } label: {
                SettingsRowLabel(title: "Privacy", systemImage: "lock")
            }
            NavigationLink { OrganizationAccessView() } label: {
                SettingsRowLabel(title: "Organization & access", systemImage: "building.2")
            }
            NavigationLink { AccountProfileView(onClose: { dismiss() }) } label: {
                SettingsRowLabel(title: "Account", systemImage: "person")
            }
        } header: {
            Text("Account")
        }
    }

    // MARK: - Assistant

    private var assistantSection: some View {
        Section {
            Picker(selection: responseStyleBinding) {
                ForEach(ResponseStyle.allCases, id: \.self) { style in
                    Text(style.label).tag(style)
                }
            } label: {
                SettingsRowLabel(title: "Response style", systemImage: "text.bubble")
            }
            NavigationLink { CustomInstructionsView() } label: {
                SettingsRowLabel(
                    title: "Custom instructions",
                    systemImage: "text.quote",
                    value: preferences.customInstructions.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "None" : "Set"
                )
            }
            NavigationLink { MemorySettingsView() } label: {
                SettingsRowLabel(title: "Memory", systemImage: "brain")
            }
            NavigationLink { MemoryDriveView() } label: {
                SettingsRowLabel(title: "Memory Drive", systemImage: "externaldrive")
            }
            NavigationLink { VoiceSettingsPage() } label: {
                SettingsRowLabel(title: "Voice", systemImage: "mic")
            }
            NavigationLink { GeneralSettingsPage() } label: {
                SettingsRowLabel(title: "General", systemImage: "gearshape")
            }
        } header: {
            Text("Assistant")
        } footer: {
            if let error = preferences.saveError {
                Text("Couldn't sync preferences. \(FriendlyErrorMessage.from(error))")
                    .foregroundColor(Theme.statusError)
            }
        }
    }

    private var responseStyleBinding: Binding<ResponseStyle> {
        Binding(
            get: { preferences.responseStyle },
            set: { preferences.save(style: $0, instructions: preferences.customInstructions) }
        )
    }

    // MARK: - Appearance

    private var appearanceSection: some View {
        Section {
            AppearanceTiles()
        } header: {
            Text("Appearance")
        }
    }

    // MARK: - Bots & agents

    private var botsSection: some View {
        Section {
            Button(action: {
                dismiss()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    modeStore.selectBarItem(.agents)
                }
            }) {
                SettingsRowLabel(title: "Your bots", systemImage: "person.2", chevron: true)
            }
            .buttonStyle(.plain)
            Button(action: { isConnectorsPresented = true }) {
                SettingsRowLabel(title: "Connectors", systemImage: "point.3.connected.trianglepath.dotted", chevron: true)
            }
            .buttonStyle(.plain)
            webRow("Skills", systemImage: "sparkles", section: "skills")
            webRow("Plugins", systemImage: "puzzlepiece.extension", section: "plugins")
            webRow("Subagents", systemImage: "point.3.filled.connected.trianglepath.dotted", section: "subagents")
            webRow("Your twin", systemImage: "person.2.wave.2", section: "twin")
            NavigationLink { WebhooksSettingsView() } label: {
                SettingsRowLabel(title: "Webhooks", systemImage: "link")
            }
        } header: {
            Text("Bots & agents")
        }
    }

    // MARK: - Computers

    private var computersSection: some View {
        Section {
            NavigationLink { DevicePairingView() } label: {
                SettingsRowLabel(title: "Devices", systemImage: "laptopcomputer.and.iphone")
            }
            NavigationLink { CloudInstancesManagerView() } label: {
                SettingsRowLabel(title: "Cloud computers", systemImage: "cloud")
            }
            NavigationLink { CloudComputerBackupsView() } label: {
                SettingsRowLabel(title: "Backups", systemImage: "clock.arrow.circlepath")
            }
            NavigationLink { AllternitMobileSettingsPage() } label: {
                SettingsRowLabel(title: "Allternit Mobile", systemImage: "iphone")
            }
        } header: {
            Text("Computers")
        }
    }

    // MARK: - Advanced

    @ViewBuilder
    private var advancedSection: some View {
        Section {
            NavigationLink { ProductsSettingsView() } label: {
                SettingsRowLabel(title: "Gizzi Code & Cowork", systemImage: "chevron.left.forwardslash.chevron.right")
            }
            NavigationLink { PlatformSettingsView() } label: {
                SettingsRowLabel(title: "Developer", systemImage: "hammer")
            }
            Button(action: { withAnimation(.easeOut(duration: 0.2)) { showOperator.toggle() } }) {
                HStack(spacing: 12) {
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.subheadline)
                        .frame(width: 22)
                        .foregroundColor(Color("TextSecondary"))
                    Text("Advanced")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .rotationEffect(.degrees(showOperator ? 90 : 0))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(showOperator ? "Expanded" : "Collapsed")

            if showOperator {
                NavigationLink { ModelManagementView() } label: {
                    SettingsRowLabel(title: "Models & engines", systemImage: "cpu")
                }
                NavigationLink { InfrastructureSettingsView() } label: {
                    SettingsRowLabel(title: "Infrastructure", systemImage: "square.stack.3d.up")
                }
                NavigationLink { RuntimeOperationsView() } label: {
                    SettingsRowLabel(title: "Runtime operations", systemImage: "gearshape.2")
                }
                NavigationLink { NodesManagerView() } label: {
                    SettingsRowLabel(title: "Compute nodes", systemImage: "externaldrive.connected.to.line.below")
                }
                NavigationLink { CloudDeployManagerView() } label: {
                    SettingsRowLabel(title: "Cloud deploy", systemImage: "icloud.and.arrow.up")
                }
                NavigationLink { VPSServersManagerView() } label: {
                    SettingsRowLabel(title: "VPS & servers", systemImage: "server.rack")
                }
                NavigationLink { EnterpriseBYOCPanelView() } label: {
                    SettingsRowLabel(title: "Enterprise BYOC", systemImage: "building.columns")
                }
                #if DEBUG
                Button(action: { isBrainSpikePresented = true }) {
                    SettingsRowLabel(title: "Brain Spike (debug)", systemImage: "arrow.triangle.branch", chevron: true)
                }
                .buttonStyle(.plain)
                #endif
            }

            NavigationLink { AboutSettingsPage() } label: {
                SettingsRowLabel(title: "About", systemImage: "info.circle", value: AboutSettingsPage.versionString)
            }
            Button(action: { safariURL = IdentifiableURL(url: URL(string: "https://ai.allternit.com")!) }) {
                SettingsRowLabel(title: "Open on the web", systemImage: "arrow.up.right.square", chevron: true)
            }
            .buttonStyle(.plain)
        } header: {
            Text("Advanced")
        }
    }

    // MARK: - Log out

    private var signOutSection: some View {
        Section {
            if authManager.isSignedIn {
                Button(role: .destructive, action: {
                    Task {
                        do {
                            try await authManager.signOut()
                            AccountPlanStore.shared.reset()
                            dismiss()
                        } catch {
                            signOutError = FriendlyErrorMessage.from(error.localizedDescription)
                        }
                    }
                }) {
                    HStack {
                        Spacer()
                        Label("Log out", systemImage: "rectangle.portrait.and.arrow.right")
                            .font(.subheadline.weight(.semibold))
                        Spacer()
                    }
                }
            } else {
                Button(action: {
                    dismiss()
                    authManager.signIn()
                }) {
                    HStack {
                        Spacer()
                        Text("Sign in").font(.subheadline.weight(.semibold))
                        Spacer()
                    }
                }
            }
        } footer: {
            if let signOutError {
                Text("Couldn't log out. \(signOutError)").foregroundColor(Theme.statusError)
            }
        }
    }

    // MARK: - Helpers

    private func webRow(_ title: String, systemImage: String, section: String) -> some View {
        Button(action: {
            if let url = URL(string: Self.webBase + section) { safariURL = IdentifiableURL(url: url) }
        }) {
            HStack(spacing: 12) {
                Image(systemName: systemImage)
                    .font(.subheadline)
                    .frame(width: 22)
                    .foregroundColor(Color("TextSecondary"))
                Text(title)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Image(systemName: "arrow.up.right")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens on the web")
    }
}

// MARK: - General

/// Assistant → General: the chat capability switches.
private struct GeneralSettingsPage: View {
    @ObservedObject private var settings = SettingsStore.shared

    var body: some View {
        List {
            Section {
                toggle("Artifacts", "Show interactive documents and code alongside chat.", $settings.artifactsEnabled)
                toggle("Code execution and file creation", "Let Allternit run code and create files in chats.", $settings.codeExecutionEnabled)
                toggle("Web search", "Search the web for up-to-date answers. Sets the default for the composer's Web search toggle.", $settings.webSearchDefault)
                toggle("Switch models when a message is flagged", "Automatically retry flagged messages with a different model.", $settings.switchModelsOnFlag)
            } header: {
                Text("Capabilities")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("General")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func toggle(_ title: String, _ explainer: String, _ isOn: Binding<Bool>) -> some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline).foregroundColor(Color("TextPrimary"))
                Text(explainer).font(.caption).foregroundColor(Color("TextSecondary"))
            }
        }
        .tint(Color("AccentPrimary"))
    }
}

// MARK: - Voice

/// Assistant → Voice: dictation language and reading speed.
private struct VoiceSettingsPage: View {
    @ObservedObject private var settings = SettingsStore.shared

    var body: some View {
        List {
            Section {
                // Consumed by DictationController (SFSpeechRecognizer locale);
                // nil = System default.
                Picker("Speech language", selection: $settings.speechLanguage) {
                    Text("System default").tag(SpeechLanguage?.none)
                    ForEach(SpeechLanguage.allCases, id: \.self) { language in
                        Text(language.label).tag(SpeechLanguage?.some(language))
                    }
                }
                .font(.subheadline)
                Picker("Speed", selection: $settings.speechSpeed) {
                    ForEach(SettingsStore.speechSpeeds, id: \.self) { speed in
                        Text(speed == 1.0 ? "1x" : "\(speed)x").tag(speed)
                    }
                }
                .font(.subheadline)
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Voice")
        .navigationBarTitleDisplayMode(.inline)
    }
}

// MARK: - Allternit Mobile (mesh)

/// Computers → Allternit Mobile: this phone's node on the Allternit mesh.
/// Start enrolls via the platform (`POST /api/v1/mesh/enroll`) and joins the
/// tailnet; failures show in the Status row.
private struct AllternitMobileSettingsPage: View {
    @ObservedObject private var mesh = MeshClient.shared
    #if DEBUG
    /// Pre-auth key entry (DEBUG only) — overrides platform enrollment.
    @AppStorage("mesh-auth-key") private var meshAuthKey = ""
    #endif

    var body: some View {
        List {
            Section {
                #if DEBUG
                SecureField("Pre-auth key (debug)", text: $meshAuthKey)
                    .font(.system(.subheadline, design: .monospaced))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                #endif
                HStack {
                    Text("Status").font(.subheadline).foregroundColor(Color("TextPrimary"))
                    Spacer()
                    switch mesh.state {
                    case .idle:
                        Text("Off").font(.subheadline).foregroundColor(Color("TextSecondary"))
                    case .starting:
                        HStack(spacing: 8) {
                            ProgressView()
                            Text("Joining…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                        }
                    case .up(let meshIP):
                        Text(meshIP)
                            .font(.system(.subheadline, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                    case .failed(let message):
                        Text(FriendlyErrorMessage.from(message))
                            .font(.caption)
                            .foregroundColor(Theme.statusError)
                            .lineLimit(2)
                    }
                }
                if case .up = mesh.state {
                    Button(action: { mesh.stop() }) {
                        SettingsRowLabel(title: "Disconnect this phone", systemImage: "stop.circle")
                    }
                } else {
                    Button(action: { Task { await mesh.startWithPlatformAuth() } }) {
                        SettingsRowLabel(title: "Connect this phone", systemImage: "point.3.connected.trianglepath.dotted")
                    }
                    .disabled(mesh.state == .starting)
                }
            } footer: {
                Text("Lets this phone reach your own computers directly over the Allternit mesh.")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Allternit Mobile")
        .navigationBarTitleDisplayMode(.inline)
    }
}

// MARK: - About

struct AboutSettingsPage: View {
    @State private var safariURL: IdentifiableURL? = nil

    static var versionString: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        return "\(version) (\(build))"
    }

    var body: some View {
        List {
            Section {
                HStack {
                    Text("Version").font(.subheadline).foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(Self.versionString).font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
                #if DEBUG
                HStack {
                    Text("API").font(.subheadline).foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(AppConfig.apiBaseURL.absoluteString)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                #endif
            }
            Section {
                link("Report an issue", "exclamationmark.bubble", "https://allternit.com/support")
                link("Privacy policy", "hand.raised", "https://allternit.com/privacy")
                link("Terms of service", "doc.text", "https://allternit.com/terms")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("About")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
    }

    private func link(_ title: String, _ icon: String, _ raw: String) -> some View {
        Button(action: { if let url = URL(string: raw) { safariURL = IdentifiableURL(url: url) } }) {
            SettingsRowLabel(title: title, systemImage: icon, chevron: true)
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Custom instructions editor

/// Pushed editor for the user's custom instructions (Settings → Assistant).
/// Saved via PreferencesStore on Done — the same PUT that syncs STYLE.md
/// into every agent workspace (agent_preferences_routes.rs).
struct CustomInstructionsView: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var preferences = PreferencesStore.shared

    @State private var text: String = ""

    var body: some View {
        TextEditor(text: $text)
            .font(.system(.body, design: .monospaced))
            .padding(12)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Custom Instructions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") {
                        preferences.save(
                            style: preferences.responseStyle,
                            instructions: text.trimmingCharacters(in: .whitespacesAndNewlines)
                        )
                        dismiss()
                    }
                }
            }
            .onAppear {
                text = preferences.customInstructions
            }
    }
}
