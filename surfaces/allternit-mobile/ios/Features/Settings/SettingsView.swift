import SwiftUI

/// Settings, the PWA's phone Settings sheet (allternit-ai
/// src/fabric-session/phone/screens/SettingsScreen.tsx + settings.config.ts
/// `PHONE_SETTINGS_GROUPS`): ✕ and ⓘ, an account card (plan and usage left),
/// then Account · Assistant · Appearance · Bots & agents · Computers ·
/// Advanced as grouped cards, and Log out at the bottom. Operator pages sit
/// behind the collapsed "Advanced" row, as on the PWA. Each row pushes its
/// page (`SettingsPageID`) with a plain ‹ back.
///
/// Native-only, kept for the App Store: "Let your bots use this iPhone"
/// (Computers), Delete account (Account page), the notifications permission
/// (Notifications page).
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var authManager: AuthManager
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var settings = SettingsStore.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var appearance = AppearanceStore.shared
    /// "Let your bots use this iPhone" status ("Paired · <computer>").
    @ObservedObject private var appPhone = AppPhoneService.shared

    @State private var path = NavigationPath()
    /// Operator pages (models, infrastructure…) behind the Advanced row.
    @State private var showOperator = false
    @State private var isCustomizePresented = false
    @State private var isConnectorsPresented = false
    @State private var safariURL: IdentifiableURL? = nil
    @State private var signOutError: String? = nil

    #if DEBUG
    @State private var isPlatformPresented = false
    @State private var isBrainSpikePresented = false
    #endif

    var body: some View {
        NavigationStack(path: $path) {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 4) {
                        accountCard
                        card(.account, label: "Account")
                        card(.assistant, label: "Assistant")
                        appearanceCard
                        card(.bots, label: "Bots & agents")
                        card(.computers, label: "Computers")
                        advancedCard
                            .id("aboutSection")
                        signOutButton
                    }
                    .padding(EdgeInsets(top: 8, leading: 16, bottom: 32, trailing: 16))
                }
                .background(Color("BgPrimary").ignoresSafeArea())
                .navigationTitle("Settings")
                .navigationBarTitleDisplayMode(.inline)
                // Pushed pages show a bare ‹ like the PWA's CaretLeft, not "‹ Settings".
                .toolbarRole(.editor)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        roundButton("xmark", label: "Close settings") { dismiss() }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        roundButton("info.circle", label: "About Allternit") { path.append(SettingsPageID.about) }
                    }
                }
                .navigationDestination(for: SettingsPageID.self) { page in
                    destination(page)
                }
                #if DEBUG
                .navigationDestination(isPresented: $isPlatformPresented) { PlatformSettingsView() }
                .navigationDestination(isPresented: $isBrainSpikePresented) { BrainSpikeView() }
                #endif
                .sheet(item: $safariURL) { item in SafariView(url: item.url) }
                .sheet(isPresented: $isCustomizePresented) { CustomizeSheet() }
                .sheet(isPresented: $isConnectorsPresented) { ConnectorsListView() }
                .task {
                    PreferencesStore.shared.fetchIfNeeded()
                    UsageStore.shared.fetchUsageIfNeeded()
                    if authManager.isSignedIn { AccountPlanStore.shared.load() }
                }
                .onAppear {
                    #if DEBUG
                    if launchArgumentEnabled("open-settings-memory") { path.append(SettingsPageID.memory) }
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

    // MARK: - Header

    /// `.phone-round-btn`: 40pt filled circle.
    private func roundButton(_ systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 18, weight: .regular))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 40, height: 40)
                .background(Circle().fill(Color("FillNeutral")))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: - Account card

    /// Signed-in person, plan and how much of it is left; opens Profile.
    private var accountCard: some View {
        NavigationLink(value: SettingsPageID.profile) {
            HStack(spacing: 12) {
                AccountAvatar(size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(accountName)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(accountDetail)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                PhoneRowChevron()
            }
            .padding(14)
            .contentShape(Rectangle())
        }
        .buttonStyle(AccountCardButtonStyle())
    }

    private var accountName: String {
        guard authManager.isSignedIn else { return "Account" }
        return authManager.fullName ?? authManager.displayName
    }

    private var accountDetail: String {
        var parts: [String] = []
        if authManager.isSignedIn {
            if let name = plan.planName ?? usageStore.planLabel { parts.append(name) }
            if let percent = usageStore.percentUsed {
                let left = max(0, 100 - Int(percent.rounded()))
                parts.append("\(left)% left · \(usageStore.periodLabel.lowercased())")
            }
            if parts.isEmpty, let email = authManager.primaryEmail { parts.append(email) }
        }
        return parts.isEmpty ? "Sign in to sync across devices" : parts.joined(separator: " · ")
    }

    // MARK: - Cards

    private func card(_ group: SettingsPageID.Group, label: String) -> some View {
        VStack(spacing: 0) {
            PhoneSettingsLabel(text: label)
            PhoneSettingsCard {
                if group == .assistant {
                    // Customize (skills, connectors, plugins) is its own screen on the phone.
                    actionRow("Customize", systemImage: "slider.horizontal.3") { isCustomizePresented = true }
                    PhoneCardDivider()
                }
                rows(SettingsPageID.items(group))
                switch group {
                case .assistant:
                    PhoneCardDivider()
                    PhoneToggleRow(title: "Save chat drafts", systemImage: "square.and.arrow.down", isOn: $settings.autoSave)
                case .computers:
                    // Native: the iPhone app's own phone link (App Store build).
                    PhoneCardDivider()
                    NavigationLink { AppPhonesSettingsView() } label: {
                        PhoneSettingsRowContent(
                            title: "Let your bots use this iPhone",
                            systemImage: "iphone.gen3",
                            value: appPhone.credentials == nil ? "Off" : "Paired"
                        ) { PhoneRowChevron() }
                    }
                    .buttonStyle(PhoneRowButtonStyle())
                default:
                    EmptyView()
                }
            }
        }
    }

    @ViewBuilder
    private func rows(_ pages: [SettingsPageID]) -> some View {
        ForEach(Array(pages.enumerated()), id: \.element) { index, page in
            if index > 0 { PhoneCardDivider() }
            pageRow(page)
        }
    }

    /// `RowButton`: icon, label, value, ›. Connectors opens its sheet.
    @ViewBuilder
    private func pageRow(_ page: SettingsPageID) -> some View {
        if page == .connectors {
            actionRow(page.label, systemImage: page.systemImage) { isConnectorsPresented = true }
        } else {
            NavigationLink(value: page) {
                PhoneSettingsRowContent(title: page.label, systemImage: page.systemImage, value: value(for: page)) {
                    PhoneRowChevron()
                }
            }
            .buttonStyle(PhoneRowButtonStyle())
        }
    }

    private func actionRow(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PhoneSettingsRowContent(title: title, systemImage: systemImage) { PhoneRowChevron() }
        }
        .buttonStyle(PhoneRowButtonStyle())
    }

    /// Row values: the app version on About, the plan on Billing.
    private func value(for page: SettingsPageID) -> String? {
        switch page {
        case .about:
            return Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        case .billing:
            return plan.planName
        default:
            return nil
        }
    }

    private var appearanceCard: some View {
        VStack(spacing: 0) {
            PhoneSettingsLabel(text: "Appearance")
            PhoneSettingsCard {
                AppearanceTiles()
                PhoneCardDivider()
                PhoneToggleRow(title: "Reduce motion", systemImage: "pause",
                               isOn: $appearance.reduceMotion)
            }
        }
    }

    /// Advanced card: the everyday pages, the operator pages behind an
    /// "Advanced" row, then About and Open on the web.
    private var advancedCard: some View {
        let pages = SettingsPageID.items(.advanced)
        return VStack(spacing: 0) {
            PhoneSettingsLabel(text: "Advanced")
            PhoneSettingsCard {
                rows(pages.filter { $0.phone.order < 9 })
                PhoneCardDivider()
                Button(action: { withAnimation(.easeOut(duration: 0.2)) { showOperator.toggle() } }) {
                    PhoneSettingsRowContent(title: "Advanced", systemImage: "wrench") {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .rotationEffect(.degrees(showOperator ? 90 : 0))
                    }
                }
                .buttonStyle(PhoneRowButtonStyle())
                .accessibilityValue(showOperator ? "Expanded" : "Collapsed")
                if showOperator {
                    PhoneCardDivider()
                    rows(SettingsPageID.items(.operatorPages))
                    #if DEBUG
                    PhoneCardDivider()
                    actionRow("Brain Spike (debug)", systemImage: "arrow.triangle.branch") { isBrainSpikePresented = true }
                    #endif
                }
                PhoneCardDivider()
                rows(pages.filter { $0.phone.order >= 9 })
                PhoneCardDivider()
                actionRow("Open on the web", systemImage: "arrow.up.right.square") {
                    safariURL = IdentifiableURL(url: URL(string: "https://ai.allternit.com")!)
                }
            }
        }
    }

    // MARK: - Log out

    /// `.phone-signout`: a 50pt filled pill; Sign in when signed out.
    private var signOutButton: some View {
        VStack(spacing: 8) {
            if authManager.isSignedIn {
                Button(action: {
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
                    HStack(spacing: 10) {
                        Image(systemName: "rectangle.portrait.and.arrow.right")
                            .font(.system(size: 18))
                        Text("Log out")
                    }
                    .font(.system(size: 16))
                    .foregroundColor(Theme.statusError)
                    .frame(maxWidth: .infinity, minHeight: 50)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                }
                .buttonStyle(.plain)
            } else {
                Button(action: {
                    dismiss()
                    authManager.signIn()
                }) {
                    Text("Sign in")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 50)
                        .background(Color("FillNeutral"))
                        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                }
                .buttonStyle(.plain)
            }
            if let signOutError {
                Text("Couldn't log out. \(signOutError)")
                    .font(.system(size: 13))
                    .foregroundColor(Theme.statusError)
            }
        }
        .padding(.top, 22)
    }

    // MARK: - Pages

    @ViewBuilder
    private func destination(_ page: SettingsPageID) -> some View {
        switch page {
        case .profile: ProfileSettingsPage()
        case .billing: ComputeBillingView(title: "Billing")
        case .usage: ComputeBillingView(title: "Usage")
        case .notifications: NotificationsSettingsView()
        case .privacy: PrivacySettingsView()
        case .organization: OrganizationAccessView()
        case .signin: AccountProfileView(onClose: { dismiss() })
        case .setup:
            SettingsWebPage(page: page, intro: "What your Allternit depends on. Ready means a live check just passed, not only that something is filled in.")
        case .responseStyle: ResponseStyleSettingsPage()
        case .memory: MemorySettingsView()
        case .voice: VoiceSettingsPage()
        case .appearance: GeneralSettingsPage()
        case .openModels:
            SettingsWebPage(page: page, intro: "Open-weight models with far fewer refusals. They are marked Open in the model picker and use your credits or plan.")
        case .subscriptions:
            SettingsWebPage(page: page, intro: "How your bots reach the apps, bots and coding tools you already use. Your Allternit plan is under Billing.")
        case .agentRules:
            SettingsWebPage(page: page, intro: "Configure · Observe · Extend. Set how agents work, see their contracts, and add your own rules and pieces.")
        case .skills: CapabilitiesSettingsView(kind: .skill)
        case .subagents: SubagentsSettingsView()
        case .connectors: ConnectorsListView()
        case .plugins: CapabilitiesSettingsView(kind: .plugin)
        case .connectedApps:
            SettingsWebPage(page: page, intro: "AI apps you approved to use your bots' phones or read your agents, and the events each one gets. Stop one event, or revoke the app and its next call is refused.")
        case .twin: TwinSettingsView()
        case .lens:
            SettingsWebPage(page: page, intro: "Lens continuously syncs connected sources into your local knowledge vault and serves it to AI tools via MCP — separate from the Connectors page, which is for one-off tool actions.")
        case .devices: DevicePairingView()
        case .computers: CloudInstancesManagerView()
        case .desktopPet:
            SettingsNotePage(page: page, heading: "Desktop pet",
                             note: "The desktop pet is part of Allternit Desktop for Mac and Windows: your bot floats above your other windows and shows what your bots are doing. Open these settings in Allternit Desktop to turn it on or off.")
        case .extensions: ProductsSettingsView(part: .extensions)
        case .remoteControl: AllternitMobileSettingsPage()
        case .gizziCode: ProductsSettingsView(part: .gizziCode)
        case .cowork: ProductsSettingsView(part: .cowork)
        case .diagnostics: DeveloperSettingsPage()
        case .status:
            SettingsWebPage(page: page, intro: "Whether everything your bots use is working. Subscriptions you sign in with and channels your bots post to appear here.")
        case .about: AboutSettingsPage()
        case .models: ModelManagementView()
        case .imageProviders:
            SettingsWebPage(page: page, intro: "Connect the image generation backends you want to use. Pollinations is free and works without a key.")
        case .videoProviders:
            SettingsWebPage(page: page, intro: "Connect the video generation backends you want to use. Pollinations is free and works without a key.")
        case .mcp: McpServersSettingsView()
        case .apiKeys:
            SettingsWebPage(page: page, intro: "Detected providers and their auth state. Connect a subscription or CLI brain in one click; API-key providers use a key.")
        case .permissions:
            SettingsNotePage(page: page, heading: "System permissions",
                             note: "Permission guide is only available in the Allternit Desktop app.")
        case .infrastructure: InfrastructureSettingsView()
        case .security:
            SettingsWebPage(page: page, intro: "Policies, approvals, purpose binding and compliance for your organization.")
        case .agents: agentsPage
        case .webhooks: WebhooksSettingsView()
        }
    }

    /// Settings → Agents: the web's "My Agents" note and its Open Agent Hub.
    private var agentsPage: some View {
        PhoneSettingsPage(title: "Agents") {
            SettingsSectionHeading(text: "My Agents")
            SettingsDescription(text: "Agent setup and discovery live in the Agent Hub view — one registry for creating, browsing, and chatting with agents.")
            Button("Open Agent Hub") {
                dismiss()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                    modeStore.selectBarItem(.agents)
                }
            }
            .buttonStyle(SettingsQuietButtonStyle())
        }
    }
}

/// `.phone-account-card`: 18pt rounded neutral card, stronger when pressed.
private struct AccountCardButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? Color("TextPrimary").opacity(0.09) : Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
}
