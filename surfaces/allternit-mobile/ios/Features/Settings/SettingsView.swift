import SwiftUI
import ClerkKit

/// Settings (phone calm I13; web PR #1159 `PHONE_SETTINGS_ROOT`): ✕ and an ⓘ
/// menu, then about 17 rows in the Claude iOS order. The Accounts card, then
/// Account · Assistant · Bots and computers · App, the Appearance tiles, one
/// Advanced row and Log out. `SettingsRoot` holds the list. Every other page
/// sits on the pushed Advanced screen in its section, so nothing is lost.
/// Each row pushes its page (`SettingsPageID`) with a plain ‹ back.
///
/// Native-only, kept for the App Store: Open links in (App), "Let your bots
/// use this iPhone" and Files (Advanced › Computers and devices), Waiting
/// games (Advanced › Assistant), Delete account (Account page), the
/// notifications permission (Notifications page).
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var authManager: AuthManager
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var settings = SettingsStore.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var appearance = AppearanceStore.shared
    @AppStorage(WaitingGamesStore.enabledKey) private var waitingGames = true
    @AppStorage(Haptics.enabledKey) private var hapticsOn = true
    /// "Let your bots use this iPhone" status ("Paired · <computer>").
    @ObservedObject private var appPhone = AppPhoneService.shared
    /// The Meetings row shows with the `meetings` account flag.
    @ObservedObject private var meetings = MeetingsStore.shared
    /// The Files row shows when the Files drive is set up on this phone.
    @ObservedObject private var filesDrive = FilesDrive.shared

    static func meetingsRuleLabel(_ rule: String?) -> String? {
        switch rule {
        case "all_except_1on1": return "All but 1:1s"
        case "all": return "Every call"
        case "off": return "When I ask"
        default: return nil
        }
    }

    @State private var path = NavigationPath()
    @State private var isCustomizePresented = false
    @State private var isConnectorsPresented = false
    @State private var isInfoOpen = false
    @State private var signOutError: String? = nil
    /// Live status line for the System status row; nil until fetched or on failure.
    @State private var systemStatus: SystemStatusLevel? = nil
    /// Accounts card: the person's Clerk organizations and which is active.
    @State private var memberships: [OrganizationMembership] = []
    @State private var activeOrgId: String? = nil
    @State private var switchingTo: String?? = nil
    @State private var accountError: String? = nil
    /// Switch account (I14b): other accounts used on this device, which row
    /// is signing out (a user id, or `addAnotherKey`), and a refused sign-out.
    @State private var rememberedAccounts: [RememberedAccount] = []
    @State private var leaving: String? = nil
    @State private var leaveFailed = false
    @ReducedMotion private var reducedMotion

    #if DEBUG
    @State private var isPlatformPresented = false
    @State private var isBrainSpikePresented = false
    @State private var isMeetingsPresented = false
    #endif

    var body: some View {
        NavigationStack(path: $path) {
            // "Settings" is a block title that docks into the nav bar as the page scrolls.
            DockingTitleScrollView(title: "Settings", chrome: .navigationBar) {
                VStack(spacing: 4) {
                    ForEach(Array(SettingsRoot.entries.enumerated()), id: \.offset) { _, entry in
                        entryView(entry)
                    }
                }
                // The dock adds 14pt per side, so 2 here keeps the 16pt gutter.
                .padding(EdgeInsets(top: 8, leading: 2, bottom: 32, trailing: 2))
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            // The sheet's root has ✕ only, never a ‹ (the PWA's SettingsHome).
            .navigationBarBackButtonHidden(true)
            // Pushed pages show a bare ‹ like the PWA's CaretLeft, not "‹ Settings".
            .toolbarRole(.editor)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    roundButton("xmark", label: "Close settings") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    infoButton
                }
            }
            .navigationDestination(for: SettingsPageID.self) { page in
                destination(page)
                    .environment(\.settingsPageSymbol, page.systemImage)
            }
            .navigationDestination(for: SettingsRoute.self) { route in
                switch route {
                case .advanced: advancedScreen
                case .openLinks: OpenLinksSettingsPage()
                }
            }
            #if DEBUG
            .navigationDestination(isPresented: $isPlatformPresented) { PlatformSettingsView() }
            .navigationDestination(isPresented: $isBrainSpikePresented) { BrainSpikeView() }
            .navigationDestination(isPresented: $isMeetingsPresented) { MeetingsSettingsView() }
            #endif
            .sheet(isPresented: $isCustomizePresented) { CustomizeSheet() }
            .sheet(isPresented: $isConnectorsPresented) { ConnectorsListView() }
            .task {
                PreferencesStore.shared.fetchIfNeeded()
                await meetings.refreshFlag()
                if meetings.isEnabled { await meetings.loadSettings() }
                UsageStore.shared.fetchUsageIfNeeded()
                if authManager.isSignedIn { AccountPlanStore.shared.load() }
                await loadOrganizations()
                rememberActivePlan()
                loadRememberedAccounts()
            }
            // D14: the active account's plan label is cached for when its
            // row is inactive.
            .onChange(of: plan.planName) { _, _ in rememberActivePlan() }
            .onAppear {
                #if DEBUG
                if launchArgumentEnabled("open-settings-memory") { path.append(SettingsPageID.memory) }
                if launchArgumentEnabled("open-settings-connections") { path.append(SettingsPageID.subscriptions) }
                if launchArgumentEnabled("open-settings-platform") { isPlatformPresented = true }
                // The old About section now lives on Advanced.
                if launchArgumentEnabled("open-settings-data")
                    || launchArgumentEnabled(LaunchArgument.calmSettingsAdvanced) {
                    path.append(SettingsRoute.advanced)
                }
                if launchArgumentEnabled("open-settings-brain-spike") { isBrainSpikePresented = true }
                if launchArgumentEnabled(LaunchArgument.calmUsage) || launchArgumentEnabled("open-usage") {
                    path.append(SettingsPageID.usage)
                }
                if launchArgumentEnabled("open-settings-meetings") { isMeetingsPresented = true }
                if launchArgumentEnabled(LaunchArgument.calmSettingsInfo) {
                    // The popover needs the toolbar laid out first.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { isInfoOpen = true }
                }
                #endif
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

    // MARK: - ⓘ menu

    /// ⓘ: the version (opens About), Terms, Privacy Policy, Help and
    /// support, Status (web `InfoMenu`). Opens on tap.
    private var infoButton: some View {
        roundButton("info.circle", label: "About Allternit") {
            Haptics.light()
            isInfoOpen = true
        }
        .accessibilityIdentifier("settingsInfoMenu")
        .popover(isPresented: $isInfoOpen) {
            infoMenu.presentationCompactAdaptation(.popover)
        }
    }

    private var infoMenu: some View {
        VStack(alignment: .leading, spacing: 0) {
            infoRow("Allternit \(AboutSettingsPage.versionString)", systemImage: "info.circle") {
                path.append(SettingsPageID.about)
            }
            Divider().padding(.vertical, 4)
            infoRow("Terms", systemImage: "arrow.up.right.square") { openURL(SettingsInfoLinks.terms) }
            infoRow("Privacy Policy", systemImage: "arrow.up.right.square") { openURL(SettingsInfoLinks.privacy) }
            infoRow("Help and support", systemImage: "arrow.up.right.square") { openURL(SettingsInfoLinks.support) }
            infoRow("Status", systemImage: "arrow.up.right.square") { openURL(SettingsInfoLinks.status) }
        }
        .padding(8)
        .frame(width: 240)
    }

    private func infoRow(_ text: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.selection()
            isInfoOpen = false
            // Let the popover close before a page opens over it.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { action() }
        } label: {
            HStack(spacing: 10) {
                Image(systemName: systemImage).frame(width: 20).accessibilityHidden(true)
                Text(text).lineLimit(1)
                Spacer(minLength: 0)
            }
            .font(.system(size: 16))
            .foregroundColor(Color("TextPrimary"))
            .frame(minHeight: 44)
            .padding(.horizontal, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Root entries

    @ViewBuilder
    private func entryView(_ entry: SettingsRootEntry) -> some View {
        switch entry {
        case .accounts:
            accountsCard
        case .group(let label, let rows):
            VStack(spacing: 0) {
                SectionLabel(label).padding(EdgeInsets(top: 18, leading: 2, bottom: 8, trailing: 2))
                FlatSettingsGroup {
                    ForEach(Array(rows.enumerated()), id: \.element) { index, row in
                        if index > 0 { PhoneCardDivider() }
                        rootRow(row)
                    }
                    if label == "Account" {
                        // Account rows (web SettingsDrilldown `feedback` /
                        // `changelog`): both open the web page.
                        PhoneCardDivider()
                        linkRow("Give feedback", systemImage: "bubble.left", url: SettingsAccountLinks.feedback)
                        PhoneCardDivider()
                        linkRow("View changelog", systemImage: "doc.text", url: SettingsAccountLinks.changelog)
                    }
                }
            }
        case .appearance:
            VStack(spacing: 0) {
                SectionLabel("Appearance").padding(EdgeInsets(top: 18, leading: 2, bottom: 8, trailing: 2))
                PhoneSettingsCard { AppearanceTiles() }
            }
        case .advanced:
            FlatSettingsGroup {
                NavigationLink(value: SettingsRoute.advanced) {
                    PhoneSettingsRowContent(title: "Advanced", systemImage: "wrench",
                                            value: "\(SettingsRoot.advancedPageCount()) pages") { PhoneRowChevron() }
                }
                .buttonStyle(SettingsRowPress())
                .accessibilityIdentifier("settingsAdvancedRow")
            }
            .padding(.top, 18)
        case .logout:
            signOutButton
        }
    }

    @ViewBuilder
    private func rootRow(_ row: SettingsRootRow) -> some View {
        switch row {
        case .page(let page, _):
            pageRow(page, title: row.title)
        case .customize:
            // Catalog (skills, connectors, plugins; E8, nav id `customize`) is its own screen on the phone.
            actionRow(row.title, systemImage: "slider.horizontal.3") { isCustomizePresented = true }
        case .openLinks:
            NavigationLink(value: SettingsRoute.openLinks) {
                PhoneSettingsRowContent(title: row.title, systemImage: "safari",
                                        value: settings.openLinksIn.label) { PhoneRowChevron() }
            }
            .buttonStyle(SettingsRowPress())
        case .haptics:
            PhoneToggleRow(title: row.title, systemImage: "iphone.radiowaves.left.and.right", isOn: $hapticsOn)
                .onChange(of: hapticsOn) { _, on in if on { Haptics.light() } else { Haptics.selection() } }
        case .reduceMotion:
            PhoneToggleRow(title: row.title, systemImage: "figure.walk.motion", isOn: $appearance.reduceMotion)
                .onChange(of: appearance.reduceMotion) { _, _ in Haptics.selection() }
        }
    }

    @ViewBuilder
    private func rows(_ pages: [SettingsPageID]) -> some View {
        ForEach(Array(pages.enumerated()), id: \.element) { index, page in
            if index > 0 { PhoneCardDivider() }
            pageRow(page, title: SettingsRoot.label(for: page))
        }
    }

    /// `RowButton`: icon, label, value, ›. Connectors opens its sheet.
    @ViewBuilder
    private func pageRow(_ page: SettingsPageID, title: String) -> some View {
        if page == .connectors {
            actionRow(title, systemImage: page.systemImage) { isConnectorsPresented = true }
        } else {
            NavigationLink(value: page) {
                PhoneSettingsRowContent(title: title, systemImage: page.systemImage, value: value(for: page)) {
                    PhoneRowChevron()
                }
            }
            .buttonStyle(SettingsRowPress())
        }
    }

    /// A row that opens a web page: label left, ↗ right.
    private func linkRow(_ title: String, systemImage: String, url: URL) -> some View {
        Button {
            Haptics.selection()
            openURL(url)
        } label: {
            PhoneSettingsRowContent(title: title, systemImage: systemImage) {
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextTertiary"))
                    .accessibilityHidden(true)
            }
        }
        .buttonStyle(SettingsRowPress())
        .accessibilityHint("Opens in your browser")
    }

    private func actionRow(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PhoneSettingsRowContent(title: title, systemImage: systemImage) { PhoneRowChevron() }
        }
        .buttonStyle(SettingsRowPress())
    }

    /// Row values: the version on About, the plan on Billing, "N% used" on
    /// Usage only when the usage store already has a number.
    private func value(for page: SettingsPageID) -> String? {
        switch page {
        case .about:
            return Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        case .billing:
            return plan.planName ?? usageStore.planLabel
        case .usage:
            // Same windows and selector as the Usage page's top bar (I15).
            guard authManager.isSignedIn, let percent = usagePercent(cloudUsageWindows(usageStore.snapshot)) else { return nil }
            return "\(percent)% used"
        default:
            return nil
        }
    }

    // MARK: - Accounts card

    /// The signed-in person and their organizations. The active one has a ✓
    /// and opens Profile; tapping another switches to it (Clerk `setActive`,
    /// `AccountSwitch`). Under them (I14b): other accounts used on this
    /// device and "Add another account". Signed out: one row that opens
    /// Profile.
    private var accountsCard: some View {
        VStack(spacing: 0) {
            FlatSettingsGroup {
                if authManager.isSignedIn {
                    ForEach(Array(accountRowModels.enumerated()), id: \.element.id) { index, row in
                        if index > 0 { PhoneCardDivider() }
                        if row.orgId == nil && row.isActive {
                            UserStatusAccountMenu(name: row.name, onAccountSettings: {
                                path.append(SettingsPageID.profile)
                            }) { AccountAvatar(size: 36) }
                        } else {
                        accountRow(row) {
                            if row.orgId == nil {
                                AccountAvatar(size: 36)
                            } else {
                                orgBadge(row.name)
                            }
                        }
                        }
                    }
                    // Other people (I14b, option B): one Clerk session at a
                    // time, so switching person signs out and opens sign-in
                    // with that email filled in (multi-session is not on).
                    if showsRememberedAccounts {
                        ForEach(rememberedAccounts) { account in
                            PhoneCardDivider()
                            rememberedRow(account)
                                .transition(reducedMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
                        }
                        PhoneCardDivider()
                        addAnotherAccountRow
                    }
                } else {
                    NavigationLink(value: SettingsPageID.profile) {
                        HStack(spacing: 12) {
                            AccountAvatar(size: 36)
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Account")
                                    .font(.system(size: 16, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                Text("Sign in to sync across devices")
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            PhoneRowChevron()
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(SettingsRowPress())
                }
            }
            if leaveFailed {
                Text(RememberedAccounts.switchFailedText)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(EdgeInsets(top: 6, leading: 6, bottom: 0, trailing: 6))
            }
            if let accountError {
                Text(accountError)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(EdgeInsets(top: 6, leading: 6, bottom: 0, trailing: 6))
            }
        }
    }

    private func accountRow<Avatar: View>(_ row: AccountRowModel,
                                          @ViewBuilder avatar: () -> Avatar) -> some View {
        let orgId = row.orgId
        let name = row.name
        let detail = row.detail
        let active = row.isActive
        return Button(action: {
            if active {
                path.append(SettingsPageID.profile)
            } else {
                Task { await switchAccount(to: orgId) }
            }
        }) {
            HStack(spacing: 12) {
                avatar()
                VStack(alignment: .leading, spacing: 2) {
                    Text(name)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let detail, !detail.isEmpty {
                        Text(detail)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if switchingTo == .some(orgId) {
                    ProgressView()
                } else if active {
                    // Scales in on the newly active row; no motion when
                    // motion is reduced.
                    Image(systemName: "checkmark")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("BrandAccent"))
                        .transition(reducedMotion ? .identity : .scale(scale: 0.4).combined(with: .opacity))
                        .accessibilityHidden(true)
                }
            }
            .animation(reducedMotion ? nil : .spring(response: 0.3, dampingFraction: 0.7), value: active)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(minHeight: 56)
            .contentShape(Rectangle())
        }
        .buttonStyle(SettingsRowPress())
        .disabled(switchingTo != nil)
        .accessibilityAddTraits(active ? .isSelected : [])
        .accessibilityHint(active ? "Opens your profile" : "Switches to this account")
    }

    // MARK: Switch account (I14b)

    private static let addAnotherKey = "add-another-account"

    /// Real Clerk sign-in, or the sample card under `-calm-accounts`.
    private var showsRememberedAccounts: Bool {
        #if DEBUG
        if SampleWorkspace.isEnabled { return launchArgumentEnabled(LaunchArgument.calmAccounts) }
        #endif
        return authManager.isClerkConfigured
    }

    /// A remembered account: tap the row to switch (sign out, then sign-in
    /// with its email filled in); the visible ✕ removes it from this device.
    private func rememberedRow(_ account: RememberedAccount) -> some View {
        let isLeaving = leaving == account.userId
        return HStack(spacing: 0) {
            Button(action: { switchPerson(key: account.userId, email: account.email) }) {
                HStack(spacing: 12) {
                    RememberedAccountAvatar(account: account, size: 36)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(account.name)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Text(isLeaving ? "Signing out…" : account.detail)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    if isLeaving { ProgressView() }
                }
                .padding(.leading, 16)
                .padding(.vertical, 10)
                .frame(minHeight: 56)
                .contentShape(Rectangle())
            }
            .buttonStyle(SettingsRowPress())
            .accessibilityLabel("\(account.name), \(account.email)")
            .accessibilityHint("Signs out, then opens sign-in for this account")

            Button(action: { forgetRemembered(account) }) {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.trailing, 6)
            .accessibilityLabel("Remove \(account.email) from this device")
        }
        .disabled(leaving != nil || switchingTo != nil)
    }

    /// "Add another account": sign out, then an empty sign-in.
    private var addAnotherAccountRow: some View {
        let isLeaving = leaving == Self.addAnotherKey
        return Button(action: { switchPerson(key: Self.addAnotherKey, email: nil) }) {
            HStack(spacing: 12) {
                Image(systemName: "person.badge.plus")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color("BgPrimary")))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Add another account")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    if isLeaving {
                        Text("Signing out…")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if isLeaving { ProgressView() }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(minHeight: 56)
            .contentShape(Rectangle())
        }
        .buttonStyle(SettingsRowPress())
        .disabled(leaving != nil || switchingTo != nil)
        .accessibilityHint("Signs out, then opens an empty sign-in")
    }

    private func loadRememberedAccounts() {
        #if DEBUG
        if SampleWorkspace.isEnabled {
            rememberedAccounts = SettingsAccountsSample.rememberedAccounts
            leaveFailed = launchArgumentEnabled(LaunchArgument.calmAccountsSwitchFailed)
            return
        }
        #endif
        rememberedAccounts = RememberedAccounts.others(excluding: authManager.currentUserId)
    }

    private func forgetRemembered(_ account: RememberedAccount) {
        Haptics.light()
        if !SampleWorkspace.isEnabled { RememberedAccounts.forget(userId: account.userId) }
        withAnimation(Motion.pick(Motion.deck, reduced: reducedMotion)) {
            rememberedAccounts.removeAll { $0.userId == account.userId }
        }
    }

    /// Person switch (`AccountSwitch.switchPerson`). Success leaves for the
    /// sign-in screen; a refused sign-out keeps this account and shows one
    /// calm line.
    private func switchPerson(key: String, email: String?) {
        guard leaving == nil, switchingTo == nil else { return }
        Haptics.selection()
        leaveFailed = false
        accountError = nil
        #if DEBUG
        // Sample workspace: shown, not sent to Clerk.
        if SampleWorkspace.isEnabled { return }
        #endif
        leaving = key
        Task {
            do {
                try await AccountSwitch.switchPerson(email: email, authManager: authManager)
                leaving = nil
                dismiss()
            } catch {
                leaving = nil
                leaveFailed = true
                Haptics.warning()
            }
        }
    }

    /// An organization's initial on a graphite disc.
    private func orgBadge(_ name: String) -> some View {
        Text(String(name.prefix(1)).uppercased())
            .font(.system(size: 15, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .frame(width: 36, height: 36)
            .background(Circle().fill(Color("TextPrimary")))
            .accessibilityHidden(true)
    }

    private var accountName: String {
        guard authManager.isSignedIn else { return "Account" }
        return authManager.accountName
    }

    /// The card's rows: the person ("Personal · <plan>", else the email) and
    /// each organization. The active row reads the live plan; the others the
    /// plan label cached while they were active (D14). No other account's
    /// data is ever fetched.
    private var accountRowModels: [AccountRowModel] {
        let userId = authManager.currentUserId
        return accountRows(
            personName: accountName,
            personFallbackDetail: authManager.accountEmail,
            organizations: organizationList,
            activeOrgId: activeOrgId,
            livePlanName: plan.planName ?? usageStore.planLabel,
            cachedPlan: { orgId in
                guard let userId else { return nil }
                return AccountPlanLabels.read(userId: userId, orgId: orgId)
            }
        )
    }

    /// The person's organizations (sample ones under `-sample-workspace`).
    private var organizationList: [(id: String, name: String)] {
        #if DEBUG
        if SampleWorkspace.isEnabled { return SettingsAccountsSample.organizations }
        #endif
        return memberships.map { (id: $0.organization.id, name: $0.organization.name) }
    }

    /// Saves the active account's plan label for its row (D14).
    private func rememberActivePlan() {
        guard authManager.isSignedIn, let userId = authManager.currentUserId,
              let name = plan.planName ?? usageStore.planLabel else { return }
        AccountPlanLabels.remember(userId: userId, orgId: activeOrgId, planName: name)
        // The remembered-accounts row shows it after a person change (I14b).
        RememberedAccounts.setPlan(userId: userId, planLabel: name)
    }

    @MainActor
    private func loadOrganizations() async {
        #if DEBUG
        if SampleWorkspace.isEnabled {
            SettingsAccountsSample.seedPlanLabels()
            return
        }
        #endif
        guard authManager.isSignedIn, authManager.isClerkConfigured, let user = Clerk.shared.user else { return }
        activeOrgId = Clerk.shared.session?.lastActiveOrganizationId
        if let cached = user.organizationMemberships, !cached.isEmpty { memberships = cached }
        // A failed refresh keeps the cached list; the person row still shows.
        if let fresh = try? await user.getOrganizationMemberships(page: 1, pageSize: 50).data {
            memberships = fresh
        }
    }

    @MainActor
    private func switchAccount(to orgId: String?) async {
        #if DEBUG
        // Sample workspace: the switch is shown, not sent to Clerk.
        if SampleWorkspace.isEnabled, orgId != activeOrgId {
            activeOrgId = orgId
            Haptics.selection()
            return
        }
        #endif
        guard authManager.isClerkConfigured, orgId != activeOrgId,
              let sessionId = Clerk.shared.session?.id else { return }
        switchingTo = .some(orgId)
        accountError = nil
        defer { switchingTo = nil }
        do {
            activeOrgId = try await AccountSwitch.activate(sessionId: sessionId, orgId: orgId, authManager: authManager)
        } catch {
            accountError = "Couldn't switch accounts. \(plainErrorText(error))"
        }
    }

    // MARK: - Advanced

    /// Advanced ›: every page that is not on the root, in the web's
    /// sections, plus the native rows that used to sit on the root.
    private var advancedScreen: some View {
        ScrollView {
            VStack(spacing: 4) {
                ForEach(SettingsAdvancedSection.allCases, id: \.self) { section in
                    VStack(spacing: 0) {
                        SectionLabel(section.label).padding(EdgeInsets(top: 18, leading: 2, bottom: 8, trailing: 2))
                        PhoneSettingsCard {
                            rows(SettingsRoot.advancedItems(section))
                            advancedExtras(section)
                        }
                    }
                }
                PhoneSettingsCard {
                    actionRow("Open on the web", systemImage: "arrow.up.right.square") {
                        openURL(URL(string: "https://ai.allternit.com")!)
                    }
                    PhoneCardDivider()
                    actionRow("Courses", systemImage: "graduationcap") {
                        openURL(URL(string: "https://labs.allternit.com")!)
                    }
                }
                .padding(.top, 18)
            }
            .padding(EdgeInsets(top: 8, leading: 16, bottom: 32, trailing: 16))
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Advanced")
        .navigationBarTitleDisplayMode(.inline)
        .accessibilityIdentifier("settingsAdvanced")
    }

    /// Rows that are not `SettingsPageID` pages, kept from the old root.
    @ViewBuilder
    private func advancedExtras(_ section: SettingsAdvancedSection) -> some View {
        switch section {
        case .assistant:
            if meetings.isEnabled {
                PhoneCardDivider()
                NavigationLink { MeetingsSettingsView() } label: {
                    PhoneSettingsRowContent(title: "Meetings", systemImage: "video",
                                            value: Self.meetingsRuleLabel(meetings.settings?.autoJoin)) { PhoneRowChevron() }
                }
                .buttonStyle(SettingsRowPress())
                .accessibilityIdentifier("settingsMeetingsRow")
            }
            PhoneCardDivider()
            PhoneToggleRow(title: "Save chat drafts", systemImage: "square.and.arrow.down", isOn: $settings.autoSave)
                .onChange(of: settings.autoSave) { _, _ in Haptics.selection() }
            PhoneCardDivider()
            PhoneToggleRow(title: "Waiting games", systemImage: "gamecontroller", isOn: $waitingGames)
                .onChange(of: waitingGames) { _, _ in Haptics.selection() }
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
            .buttonStyle(SettingsRowPress())
            if filesDrive.isEnabled {
                // Files drive dashboard: devices + wake setting, shares, audit.
                PhoneCardDivider()
                NavigationLink { FilesDashboardView() } label: {
                    PhoneSettingsRowContent(title: "Files", systemImage: "folder",
                                            value: filesDrive.computers.count == 1 ? "1 computer" : "\(filesDrive.computers.count) computers") { PhoneRowChevron() }
                }
                .buttonStyle(SettingsRowPress())
                .accessibilityIdentifier("settingsFilesRow")
            }
        case .developer:
            #if DEBUG
            PhoneCardDivider()
            actionRow("Brain Spike (debug)", systemImage: "arrow.triangle.branch") { isBrainSpikePresented = true }
            #else
            EmptyView()
            #endif
        case .about:
            PhoneCardDivider()
            systemStatusRow
        default:
            EmptyView()
        }
    }

    /// "System status": opens status.allternit.com; a small live line sits
    /// under the title when the fetch worked, nothing when it didn't.
    private var systemStatusRow: some View {
        Button(action: { openURL(SystemStatusClient.pageURL) }) {
            HStack(spacing: 14) {
                Image(systemName: "waveform.path.ecg")
                    .font(.system(size: 19, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 22, height: 22)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("System status")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                    if let line = systemStatus?.line, let level = systemStatus {
                        HStack(spacing: 4) {
                            Image(systemName: level.symbol)
                                .font(.system(size: 12))
                                .accessibilityHidden(true)
                            Text(line)
                                .font(.system(size: 13))
                        }
                        .foregroundColor(Color("TextSecondary"))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                PhoneRowChevron()
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .frame(minHeight: 50)
            .contentShape(Rectangle())
        }
        .buttonStyle(SettingsRowPress())
        .accessibilityLabel("System status")
        .accessibilityValue(systemStatus?.line ?? "")
        .accessibilityHint("Opens status.allternit.com")
        .task { systemStatus = await SystemStatusClient.fetch() }
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
        case .profile: ProfileSettingsPage(onClose: { dismiss() })
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
        case .subscriptions: ConnectionsSettingsView()
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
        case .deviceAccess: DeviceAccessSettingsView()
        case .selfHosted: SelfHostedSettingsView()
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

#if DEBUG
/// `-calm-accounts` with `-sample-workspace` (phone calm step 31): sample
/// organizations for the Accounts card, with cached plan labels for the
/// inactive rows. Pass `-calm-settings` too so Settings opens; the card is
/// the first thing on its root.
enum SettingsAccountsSample {
    static var organizations: [(id: String, name: String)] {
        guard launchArgumentEnabled(LaunchArgument.calmAccounts) else { return [] }
        return [(id: "org_sample_harbor", name: "Harbor Bakery"),
                (id: "org_sample_pier", name: "Pier Street Cafe")]
    }

    /// `-calm-signin-prefill`: the email the sign-in screen fills in.
    static let prefillEmail = "sam@pierstreet.example"

    /// Accounts used on this device before (I14b), shown under the card.
    static var rememberedAccounts: [RememberedAccount] {
        guard launchArgumentEnabled(LaunchArgument.calmAccounts) else { return [] }
        let now = Date()
        return [
            RememberedAccount(userId: "user_sample_sam", name: "Sam Rivera", email: prefillEmail,
                              imageUrl: nil, planLabel: "Pro", lastUsedAt: now.addingTimeInterval(-3_600)),
            RememberedAccount(userId: "user_sample_lee", name: "Lee Park", email: "lee@harborbakery.example",
                              imageUrl: nil, planLabel: nil, lastUsedAt: now.addingTimeInterval(-86_400)),
        ]
    }

    /// Cached labels as if each row had been active once (D14).
    static func seedPlanLabels() {
        guard launchArgumentEnabled(LaunchArgument.calmAccounts) else { return }
        let userId = SampleWorkspace.userId
        AccountPlanLabels.remember(userId: userId, orgId: nil, planName: "Pro")
        AccountPlanLabels.remember(userId: userId, orgId: "org_sample_harbor", planName: "Team")
    }
}
#endif

/// A remembered account's photo (its saved Clerk image URL), else its
/// initial on a graphite disc, like `AccountAvatar`.
struct RememberedAccountAvatar: View {
    let account: RememberedAccount
    var size: CGFloat = 36

    var body: some View {
        Group {
            if let string = account.imageUrl, let url = URL(string: string) {
                AsyncImage(url: url) { image in
                    image.resizable().scaledToFill()
                } placeholder: {
                    initial
                }
            } else {
                initial
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }

    private var initial: some View {
        Text(account.initial)
            .font(.system(size: size * 0.4, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .frame(width: size, height: size)
            .background(Color("TextPrimary"))
    }
}

/// Root and Advanced rows: the neutral highlight plus a .98 press (PolishTokens.pressRow).
private struct SettingsRowPress: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? Color("FillNeutral") : Color.clear)
            .scaleEffect(configuration.isPressed ? PolishTokens.pressRow : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
            .environment(\.settingsRowPressed, configuration.isPressed)
    }
}

// MARK: - Flat rows (Claude polish i2)

/// The root's row group, matching the 390 px web frame
/// (`shots/S-after-phone-root-s0.png`): no card fill, label left and
/// control right, a hairline between rows (`PhoneCardDivider`).
private struct FlatSettingsGroup<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: 0) { content }
    }
}

/// The two account rows' pages (same URLs as the web `SettingsDrilldown`).
enum SettingsAccountLinks {
    static let feedback = URL(string: "https://allternit.com/feedback")!
    static let changelog = URL(string: "https://docs.allternit.com/changelog")!
}
