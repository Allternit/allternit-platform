import SwiftUI

/// The settings hub (Phase 4), presented as a sheet from the sidebar
/// footer's gear button. iOS-standard grouped sections with the app's card
/// styling; every toggle persists via SettingsStore (UserDefaults), and rows
/// with a live backend call it directly (memory, archive/delete chats).
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var authManager: AuthManager
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var settings = SettingsStore.shared
    /// Plan usage meter (Phase 5) backing the Usage section.
    @ObservedObject private var usageStore = UsageStore.shared
    /// Response-style preferences (Agent section) — backed by
    /// `GET/PUT /api/v1/agent-preferences`, not UserDefaults.
    @ObservedObject private var preferences = PreferencesStore.shared
    /// Embedded tsnet node state for the Mesh section.
    @ObservedObject private var mesh = MeshClient.shared
    #if DEBUG
    /// Pre-auth key entry (DEBUG only) — UserDefaults `mesh-auth-key`, the
    /// same key the `-mesh-auth-key` launch argument feeds. Launch args live
    /// in the arguments domain, so the flag shadows whatever is typed here.
    /// Overrides platform enrollment when set (MeshClient.startWithPlatformAuth).
    @AppStorage("mesh-auth-key") private var meshAuthKey = ""
    #endif

    /// Pushed Memory section (NavigationStack below).
    @State private var isMemoryPresented = false
    /// Pushed Compute Billing settings (Plans & Compute parity).
    @State private var isComputeBillingPresented = false
    /// Pushed Platform settings (Settings > Platform parity).
    @State private var isPlatformPresented = false
    /// Pushed Products settings (Settings > Products parity).
    @State private var isProductsPresented = false
    /// Pushed Infrastructure settings (Settings > Infrastructure parity).
    @State private var isInfrastructurePresented = false
    /// Pushed Model Management view.
    @State private var isModelManagementPresented = false
    /// Pushed Organization & Access settings.
    @State private var isOrganizationAccessPresented = false
    /// Pushed custom-instructions editor (Agent section).
    @State private var isInstructionsPresented = false
    /// Pushed Webhook Triggers panel (Agent section; also pushed from the
    /// bot detail's Webhooks card).
    @State private var isWebhooksPresented = false
    /// Pushed Monitor view (infra section).
    @State private var isMonitorPresented = false
    /// Pushed Runtime Operations view (infra section).
    @State private var isRuntimeOperationsPresented = false
    /// Pushed Compute Nodes view (infra section).
    @State private var isNodesManagerPresented = false
    /// Pushed Cloud Deploy view (infra section).
    @State private var isCloudDeployPresented = false
    /// Pushed VPS & Servers view (infra section).
    @State private var isVPSServersPresented = false
    /// Pushed Cloud Instances view (infra section).
    @State private var isCloudInstancesPresented = false
    /// Pushed Enterprise BYOC panel (infra section).
    @State private var isEnterpriseBYOCPresented = false
    #if DEBUG
    /// Pushed Brain Spike screen (DEBUG-only D3 spike section).
    @State private var isBrainSpikePresented = false
    #endif
    /// Export/support links open in SFSafariViewController.
    @State private var safariURL: IdentifiableURL? = nil

    @State private var isArchiveConfirmPresented = false
    @State private var isDeleteConfirmPresented = false
    /// Set while a bulk archive/delete loop is in flight.
    @State private var isBulkOperationRunning = false
    /// Result/error of a bulk operation, shown verbatim.
    @State private var bulkAlert: BulkAlert? = nil

    private struct BulkAlert: Identifiable {
        let title: String
        let message: String
        var id: String { title + message }
    }

    /// `PATCH /api/v1/agent-sessions/:id` body (agent_session_routes.rs
    /// update_session — `{ active: false }` archives).
    private struct UpdateSessionBody: Encodable {
        let active: Bool
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
            List {
                accountSection
                usageSection
                platformSection
                capabilitiesSection
                agentSection
                modelsSection
                memorySection
                productsSection
                voiceSection
                dataControlsSection
                meshSection
                infraSection
                #if DEBUG
                brainSpikeSection
                #endif
                infrastructureSection
                aboutSection
                    // Anchor for the `-open-settings-data` DEBUG scroll.
                    .id("aboutSection")
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
            }
            .navigationDestination(isPresented: $isMemoryPresented) {
                MemorySettingsView()
            }
            .navigationDestination(isPresented: $isComputeBillingPresented) {
                ComputeBillingView()
            }
            .navigationDestination(isPresented: $isPlatformPresented) {
                PlatformSettingsView()
            }
            .navigationDestination(isPresented: $isProductsPresented) {
                ProductsSettingsView()
            }
            .navigationDestination(isPresented: $isInfrastructurePresented) {
                InfrastructureSettingsView()
            }
            .navigationDestination(isPresented: $isModelManagementPresented) {
                ModelManagementView()
            }
            .navigationDestination(isPresented: $isOrganizationAccessPresented) {
                OrganizationAccessView()
            }
            .navigationDestination(isPresented: $isInstructionsPresented) {
                CustomInstructionsView()
            }
            .navigationDestination(isPresented: $isWebhooksPresented) {
                WebhooksSettingsView()
            }
            .navigationDestination(isPresented: $isMonitorPresented) {
                MonitorView()
            }
            .navigationDestination(isPresented: $isRuntimeOperationsPresented) {
                RuntimeOperationsView()
            }
            .navigationDestination(isPresented: $isNodesManagerPresented) {
                NodesManagerView()
            }
            .navigationDestination(isPresented: $isCloudDeployPresented) {
                CloudDeployManagerView()
            }
            .navigationDestination(isPresented: $isVPSServersPresented) {
                VPSServersManagerView()
            }
            .navigationDestination(isPresented: $isCloudInstancesPresented) {
                CloudInstancesManagerView()
            }
            .navigationDestination(isPresented: $isEnterpriseBYOCPresented) {
                EnterpriseBYOCPanelView()
            }
            #if DEBUG
            .navigationDestination(isPresented: $isBrainSpikePresented) {
                BrainSpikeView()
            }
            #endif
            .sheet(item: $safariURL) { item in
                SafariView(url: item.url)
            }
            .confirmationDialog(
                "Archive all chats?",
                isPresented: $isArchiveConfirmPresented,
                titleVisibility: .visible
            ) {
                Button("Archive All Chats") { Task { await archiveAllChats() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Archived chats are hidden from your history but kept on the server.")
            }
            .confirmationDialog(
                "Delete all chats?",
                isPresented: $isDeleteConfirmPresented,
                titleVisibility: .visible
            ) {
                Button("Delete All Chats", role: .destructive) { Task { await deleteAllChats() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This permanently deletes every conversation. This can't be undone.")
            }
            .alert(item: $bulkAlert) { alert in
                Alert(title: Text(alert.title), message: Text(alert.message), dismissButton: .default(Text("OK")))
            }
            .onAppear {
                // Response-style row state (fetched once per launch; the
                // store dedupes, and ChatView already kicked it off).
                PreferencesStore.shared.fetchIfNeeded()
                #if DEBUG
                // `-open-settings-memory` (DEBUG only): deep-link straight
                // into the Memory section for screenshots (the sidebar's
                // `-open-settings` handling also opens this sheet on it).
                if launchArgumentEnabled("open-settings-memory") {
                    isMemoryPresented = true
                }
                // `-open-settings-platform` (DEBUG only): deep-link straight
                // into the Platform section for Maestro screenshot coverage.
                if launchArgumentEnabled("open-settings-platform") {
                    isPlatformPresented = true
                }
                // `-open-settings-data` (DEBUG only): scroll to the bottom
                // sections (Data controls / About) for screenshots — simctl
                // has no scroll injection. Delayed so the List has laid out.
                if launchArgumentEnabled("open-settings-data") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                        withAnimation { proxy.scrollTo("aboutSection", anchor: .bottom) }
                    }
                }
                // `-open-settings-brain-spike` (DEBUG only): deep-link
                // straight into the D3 spike screen for the automated proof
                // run (`-brain-spike-auto` takes it from there).
                if launchArgumentEnabled("open-settings-brain-spike") {
                    isBrainSpikePresented = true
                }
                #endif
            }
            }
        }
    }

    // MARK: - Account

    @ViewBuilder
    private var accountSection: some View {
        Section {
            HStack(spacing: 12) {
                Text(authManager.isSignedIn ? authManager.avatarInitial : "A")
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("BgPrimary"))
                    .frame(width: 44, height: 44)
                    .background(Color("TextSecondary"))
                    .clipShape(Circle())

                VStack(alignment: .leading, spacing: 2) {
                    if authManager.isSignedIn {
                        Text(authManager.displayName)
                            .font(.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextPrimary"))
                        if let email = authManager.primaryEmail {
                            Text(email)
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    } else {
                        // `-skip-auth` dev bypass: no Clerk session to read.
                        Text("Development bypass")
                            .font(.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextPrimary"))
                        Text("Running with -skip-auth; no account signed in.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }

                Spacer(minLength: 8)

                // Plan from /me/usage when the backend meters usage (Phase
                // 5); static "Free" when unmetered.
                Text(usageStore.planLabel ?? "Free")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 5)
                    .background(Color("BgSecondary"))
                    .clipShape(Capsule())
            }

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isComputeBillingPresented = true
            }) {
                HStack {
                    Text("Plans & Compute")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isOrganizationAccessPresented = true
            }) {
                HStack {
                    Text("Organization & Access")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            // Same sign-out path as the sidebar footer menu.
            Button(role: .destructive, action: {
                Task {
                    do {
                        try await authManager.signOut()
                        dismiss()
                    } catch {
                        bulkAlert = BulkAlert(
                            title: "Sign Out Failed",
                            message: error.localizedDescription
                        )
                    }
                }
            }) {
                Text("Sign Out")
            }
            .disabled(!authManager.isSignedIn)
        } header: {
            Text("Account")
        }
    }

    // MARK: - Usage

    /// Plan usage (monthly for Allternit Cloud) + credits (ChatGPT "Usage and limits" parity, Phase 5).
    /// When the backend reports metering as not configured (UsageStore is
    /// `.unavailable` — never fake numbers) the section shows the backend's
    /// message as plain text.
    @ViewBuilder
    private var usageSection: some View {
        Section {
            // `percentUsed` is nil when the backend has no metering window,
            // even if a snapshot exists. Show the backend's own message instead
            // of a misleading "0% used" progress bar.
            if let snapshot = usageStore.snapshot, usageStore.percentUsed != nil {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text(usageStore.periodLabel)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        if let percentText = usageStore.percentText {
                            Text("\(percentText) used")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }

                    let percent = (usageStore.percentUsed ?? 0) / 100
                    ProgressView(value: min(max(percent, 0), 1))
                        .tint(percent >= 1
                              ? Theme.statusError
                              : (percent >= 0.8 ? Theme.statusWarning : Color("AccentPrimary")))

                    if let resetsLabel = usageStore.resetsLabel {
                        Text("Resets \(resetsLabel)")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .padding(.vertical, 4)

                if let credits = snapshot.credits {
                    HStack {
                        Text("Credits")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Text(String(format: "%g", credits))
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }

                Button(action: {
                    // Placeholder — the real credits purchase flow is TBD.
                    safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/credits")!)
                }) {
                    bulkRowLabel("Buy credits", systemImage: "creditcard")
                }

                Button(action: {
                    // Placeholder — the real upgrade flow is TBD.
                    safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/upgrade")!)
                }) {
                    bulkRowLabel("Get Pro", systemImage: "star")
                }
            } else {
                // The backend's own words when metering isn't configured
                // (503 `usage_metering_unavailable`); a generic friendly state
                // while the first fetch is still in flight.
                let message: String = {
                    if case .unavailable(let backendMessage) = usageStore.availability {
                        return FriendlyErrorMessage.from(backendMessage)
                    }
                    return "Usage metering isn't available on this backend."
                }()
                FriendlyStateView(
                    style: .empty,
                    icon: "chart.bar",
                    title: "Usage unavailable",
                    message: message
                )
                .padding(.vertical, 12)
            }
        } header: {
            Text("Usage")
        }
    }

    // MARK: - Platform

    @ViewBuilder
    private var platformSection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isPlatformPresented = true
            }) {
                HStack {
                    Text("Platform")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Platform")
        } footer: {
            Text("General, appearance, models, API keys, shortcuts, permissions, dispatch, devices, cloud instances, and diagnostics.")
        }
    }

    // MARK: - Capabilities

    @ViewBuilder
    private var capabilitiesSection: some View {
        Section {
            capabilityToggle(
                "Artifacts",
                explainer: "Show interactive documents and code alongside chat.",
                isOn: $settings.artifactsEnabled
            )
            capabilityToggle(
                "Code execution and file creation",
                explainer: "Let Allternit run code and create files in chats.",
                isOn: $settings.codeExecutionEnabled
            )
            capabilityToggle(
                "Web search",
                explainer: "Search the web for up-to-date answers. Sets the default for the composer's Web search toggle.",
                isOn: $settings.webSearchDefault
            )
            capabilityToggle(
                "Switch models when a message is flagged",
                explainer: "Automatically retry flagged messages with a different model.",
                isOn: $settings.switchModelsOnFlag
            )
        } header: {
            Text("Capabilities")
        }
    }

    /// Toggle row with Claude-style one-line explainer copy underneath.
    private func capabilityToggle(_ title: String, explainer: String, isOn: Binding<Bool>) -> some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Text(explainer)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .tint(Color("AccentPrimary"))
    }

    // MARK: - Agent

    /// Response style + custom instructions (backend-persisted via
    /// `GET/PUT /api/v1/agent-preferences`; the PUT also syncs a managed
    /// STYLE.md into each agent workspace — agent_preferences_routes.rs).
    /// These apply to every chat: ChatViewModel injects them into the
    /// agent-chat `systemPrompt` at send time (plan Phases 5-6).
    @ViewBuilder
    private var agentSection: some View {
        Section {
            Picker("Response style", selection: responseStyleBinding) {
                ForEach(ResponseStyle.allCases, id: \.self) { style in
                    Text(style.label).tag(style)
                }
            }
            .font(.subheadline)

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isInstructionsPresented = true
            }) {
                HStack {
                    Text("Custom instructions")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(preferences.customInstructions.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "None" : "Set")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isWebhooksPresented = true
            }) {
                HStack {
                    Text("Webhooks")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button(action: {
                // The hub is a sidebar tab — close the sheet and hop to it.
                dismiss()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    modeStore.selectBarItem(.agents)
                }
            }) {
                HStack {
                    Text("agent | bot hub")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Agent")
        } footer: {
            if let error = preferences.saveError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't sync preferences",
                    message: FriendlyErrorMessage.from(error)
                )
            } else {
                Text("Applies to every chat and syncs to your agents' workspace files (STYLE.md).")
            }
        }
    }

    /// Picker writes save immediately (optimistic; the store rolls back on
    /// a failed PUT and shows the error in this section's footer).
    private var responseStyleBinding: Binding<ResponseStyle> {
        Binding(
            get: { preferences.responseStyle },
            set: { preferences.save(style: $0, instructions: preferences.customInstructions) }
        )
    }

    // MARK: - Models

    @ViewBuilder
    private var modelsSection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isModelManagementPresented = true
            }) {
                HStack {
                    Text("Models & Engines")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Models")
        } footer: {
            Text("Manage cloud model providers and local runtimes.")
        }
    }

    // MARK: - Memory

    @ViewBuilder
    private var memorySection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isMemoryPresented = true
            }) {
                HStack {
                    Text("Memory")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Memory")
        } footer: {
            Text("View what Allternit remembers and manage memory generation.")
        }
    }

    // MARK: - Products

    @ViewBuilder
    private var productsSection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isProductsPresented = true
            }) {
                HStack {
                    Text("Products")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Products")
        } footer: {
            Text("Gizziio Code, Cowork, and Extensions settings.")
        }
    }

    // MARK: - Voice

    @ViewBuilder
    private var voiceSection: some View {
        Section {
            // Consumed by DictationController (SFSpeechRecognizer locale);
            // nil = System default. The dictation onboarding sheet offers
            // the same choices and writes the same key.
            Picker("Speech language", selection: $settings.speechLanguage) {
                Text("System default").tag(SpeechLanguage?.none)
                ForEach(SpeechLanguage.allCases, id: \.self) { language in
                    Text(language.label).tag(SpeechLanguage?.some(language))
                }
            }
            .font(.subheadline)

            Picker("Speed", selection: $settings.speechSpeed) {
                ForEach(SettingsStore.speechSpeeds, id: \.self) { speed in
                    Text(Self.speedLabel(speed)).tag(speed)
                }
            }
            .font(.subheadline)
        } header: {
            Text("Voice")
        }
    }

    private static func speedLabel(_ speed: Double) -> String {
        speed == 1.0 ? "1x" : "\(speed)x"
    }

    // MARK: - Data controls

    @ViewBuilder
    private var dataControlsSection: some View {
        Section {
            // Local only — the backend has no training-preference endpoint yet.
            capabilityToggle(
                "Improve the model for everyone",
                explainer: "Allow your chats to be used to improve Allternit's models.",
                isOn: $settings.improveModel
            )

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isArchiveConfirmPresented = true
            }) {
                bulkRowLabel("Archive all chats", systemImage: "archivebox")
            }
            .disabled(isBulkOperationRunning)

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isDeleteConfirmPresented = true
            }) {
                bulkRowLabel("Delete all chats", systemImage: "trash", destructive: true)
            }
            .disabled(isBulkOperationRunning)

            Button(action: {
                safariURL = IdentifiableURL(url: URL(string: "https://ai.allternit.com")!)
            }) {
                bulkRowLabel("Export data", systemImage: "square.and.arrow.up")
            }
        } header: {
            Text("Data controls")
        } footer: {
            if isBulkOperationRunning {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Working…")
                }
            }
        }
    }

    private func bulkRowLabel(_ title: String, systemImage: String, destructive: Bool = false) -> some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.subheadline)
                .frame(width: 20)
            Text(title)
                .font(.subheadline)
            Spacer()
        }
        .foregroundColor(destructive ? Theme.statusError : Color("TextPrimary"))
        .contentShape(Rectangle())
    }

    // MARK: - Mesh

    /// Mesh (tsnet) bring-up: Start enrolls via the platform
    /// (`POST /api/v1/mesh/enroll` → fresh pre-auth key) and joins the
    /// Headscale tailnet; enroll failures (sign-in required, mesh not
    /// configured) surface in the Status row. The manual key field is
    /// DEBUG-only and overrides enrollment when set.
    @ViewBuilder
    private var meshSection: some View {
        Section {
            #if DEBUG
            SecureField("Pre-auth key (debug)", text: $meshAuthKey)
                .font(.system(.subheadline, design: .monospaced))
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            #endif

            HStack {
                Text("Status")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                switch mesh.state {
                case .idle:
                    Text("Idle")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                case .starting:
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("Joining…")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                case .up(let meshIP):
                    Text(meshIP)
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                case .failed(let message):
                    Text(message)
                        .font(.caption)
                        .foregroundColor(Theme.statusError)
                        .lineLimit(2)
                }
            }

            if case .up = mesh.state {
                Button(action: { mesh.stop() }) {
                    bulkRowLabel("Stop mesh node", systemImage: "stop.circle")
                }
            } else {
                Button(action: {
                    Task { await mesh.startWithPlatformAuth() }
                }) {
                    bulkRowLabel("Start mesh node", systemImage: "point.3.connected.trianglepath.dotted")
                }
                .disabled(mesh.state == .starting)
            }
        } header: {
            Text("Mesh")
        } footer: {
            Text("Embedded tsnet node → \(AppConfig.meshControlURL). Same directory = same node identity across launches.")
        }
    }

    // MARK: - Infra

    @ViewBuilder
    private var infraSection: some View {
        Section {
            Button(action: { isMonitorPresented = true }) {
                bulkRowLabel("Monitor", systemImage: "chart.line.uptrend.xyaxis")
            }
            Button(action: { isRuntimeOperationsPresented = true }) {
                bulkRowLabel("Runtime Operations", systemImage: "gearshape.2")
            }
            Button(action: { isNodesManagerPresented = true }) {
                bulkRowLabel("Compute Nodes", systemImage: "externaldrive.connected.to.line.below")
            }
            Button(action: { isCloudDeployPresented = true }) {
                bulkRowLabel("Cloud Deploy", systemImage: "cloud")
            }
            Button(action: { isVPSServersPresented = true }) {
                bulkRowLabel("VPS & Servers", systemImage: "server.rack")
            }
            Button(action: { isCloudInstancesPresented = true }) {
                bulkRowLabel("Cloud Instances", systemImage: "cloud.fill")
            }
            Button(action: { isEnterpriseBYOCPresented = true }) {
                bulkRowLabel("Enterprise BYOC", systemImage: "building.columns")
            }
        } header: {
            Text("Infrastructure")
        } footer: {
            Text("Live view of agents, system metrics, logs, runtime controls, compute nodes, cloud deployments, SSH connections, and BYO-VPS wizard.")
        }
    }

    // MARK: - Brain Spike (DEBUG, D3 spike)

    #if DEBUG
    /// D3 spike entry point — DEBUG builds only (whole section compile-gated,
    /// so release builds never see it; same precedent as the `-skip-auth`
    /// shim). Embedded-git proof: clone → append frontmatter page → commit
    /// → push.
    @ViewBuilder
    private var brainSpikeSection: some View {
        Section {
            Button(action: { isBrainSpikePresented = true }) {
                bulkRowLabel("Brain Spike (embedded git)", systemImage: "arrow.triangle.branch")
            }
        } header: {
            Text("Spike")
        } footer: {
            Text("D3 feasibility proof — libgit2 \(BrainGit.libgit2Version) vendored. DEBUG only; not in release builds.")
        }
    }
    #endif

    // MARK: - Infrastructure

    @ViewBuilder
    private var infrastructureSection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isInfrastructurePresented = true
            }) {
                HStack {
                    Text("Infrastructure")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Infrastructure")
        } footer: {
            Text("Environment, cloud instances, mesh, BYOC, security, and agents.")
        }
    }

    // MARK: - About

    @ViewBuilder
    private var aboutSection: some View {
        Section {
            HStack {
                Text("Version")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(Self.versionString)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }

            HStack {
                Text("API")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(AppConfig.apiBaseURL.absoluteString)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Button(action: {
                // Placeholder URL — the real support portal is TBD.
                safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/support")!)
            }) {
                bulkRowLabel("Report an issue", systemImage: "exclamationmark.bubble")
            }
        } header: {
            Text("About")
        }
    }

    private static var versionString: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        return "\(version) (\(build))"
    }

    // MARK: - Bulk chat operations

    /// Archive = `PATCH /api/v1/agent-sessions/:id { active: false }` per
    /// session — no bulk endpoint exists, so loop it. Any per-session failure
    /// is reported with the count that succeeded.
    @MainActor
    private func archiveAllChats() async {
        await runBulkOperation(verb: "archive") { session in
            guard session.active else { return false } // already archived
            try await APIClient.shared.patch(
                path: "agent-sessions/\(Self.escape(session.id))",
                body: UpdateSessionBody(active: false)
            )
            return true
        }
    }

    /// Delete = `DELETE /api/v1/agent-sessions/:id` per session.
    @MainActor
    private func deleteAllChats() async {
        await runBulkOperation(verb: "delete") { session in
            try await APIClient.shared.delete(path: "agent-sessions/\(Self.escape(session.id))")
            return true
        }
    }

    /// Shared loop: list sessions, apply `operation` to each, report the
    /// result. Backends without these routes surface the HTTP error here
    /// instead of failing silently.
    @MainActor
    private func runBulkOperation(verb: String, operation: (AgentSession) async throws -> Bool) async {
        isBulkOperationRunning = true
        defer { isBulkOperationRunning = false }

        let sessions: [AgentSession]
        do {
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            sessions = envelope.sessions
        } catch {
            bulkAlert = BulkAlert(
                title: "Couldn't \(verb) chats",
                message: "Not supported by this backend yet — listing conversations failed: \(error.localizedDescription)"
            )
            return
        }

        guard !sessions.isEmpty else {
            bulkAlert = BulkAlert(title: "Nothing to \(verb)", message: "There are no chats to \(verb).")
            return
        }

        var succeeded = 0
        var firstError: String? = nil
        for session in sessions {
            do {
                if try await operation(session) { succeeded += 1 }
            } catch {
                firstError = firstError ?? error.localizedDescription
            }
        }

        if let firstError {
            bulkAlert = BulkAlert(
                title: "Couldn't \(verb) all chats",
                message: "\(succeeded) of \(sessions.count) succeeded. First error: \(firstError)"
            )
        } else {
            bulkAlert = BulkAlert(
                title: "Done",
                message: "\(succeeded) chat\(succeeded == 1 ? "" : "s") \(verb == "archive" ? "archived" : "deleted")."
            )
        }

        if succeeded > 0 {
            NotificationCenter.default.post(name: .historyMutated, object: nil)
        }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}

// MARK: - Custom instructions editor

/// Pushed editor for the user's custom instructions (Settings → Agent).
/// Saved via PreferencesStore on Done — the same PUT that syncs STYLE.md
/// into every agent workspace (agent_preferences_routes.rs), so the file
/// and the next chat's composed system prompt never disagree.
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
