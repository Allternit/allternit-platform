import SwiftUI
import UIKit

/// Recents status filter (PhoneDrawer STATUS_OPTIONS). "All" hides archived
/// sessions (`active == false`), as on the web; "Completed" has no wire
/// signal for chats yet, so it reads like Archived.
enum RecentsStatusFilter: String, CaseIterable {
    case all = "All"
    case active = "Active"
    case completed = "Completed"
    case archived = "Archived"
}

/// Recents last-activity filter (PhoneDrawer DATE_OPTIONS).
enum RecentsDateFilter: String, CaseIterable {
    case all = "Any time"
    case today = "Today"
    case week = "Last 7 days"
    case month = "Last 30 days"
}

/// The phone drawer (allternit-ai src/fabric-session/phone/PhoneDrawer.tsx):
/// the active mode's rail, laid out like Claude iOS. Top line: the mode orb
/// (tap opens the four-mode pill over the line; Shelves is the composer
/// orb's hold only), the mode's wordmark, Inbox and a round search button. Then the mode's rail
/// rows (Projects, Artifacts, Automation Tasks, Customize, More), Teammates,
/// Pinned, and Recents with See all + a filter — swipe a recent left to
/// delete it, long-press to drag it into your own order. The list fades
/// under a floating footer: the account avatar in its usage ring (opens
/// Settings), Phones, and the New pill (tap for the mode's new item, press
/// and hold for more ways to start).
///
/// Bots mode shows the Bots rail (lane L4 owns its contents).
struct HistorySidebarView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool
    /// Drawer width — min(92% of the screen, 440), set by MainWorkspaceView.
    var width: CGFloat = 320

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var authManager: AuthManager
    @StateObject private var pinsStore = RailPinsStore.shared
    @ObservedObject private var store = ShellSessionsStore.shared
    @ObservedObject private var usage = UsageStore.shared

    @State private var actionError: String? = nil
    /// The header orb's four-mode pill is open over the top line.
    @State private var drawerModesOpen = false
    @ReducedMotion private var drawerModesReduced

    @State private var statusFilter: RecentsStatusFilter = .all
    @State private var dateFilter: RecentsDateFilter = .all
    /// Home's type filter (PhoneDrawer TYPE_OPTIONS; tasks aren't recents here).
    @State private var typeFilter: Set<RecentType> = []
    /// Long-press reorder (phone-ui-store manualOrder): session ids first, in
    /// this order; the rest follow by recency.
    @AppStorage("allternit-phone-ui-manual-order") private var manualOrderRaw = ""

    enum RecentType: String, CaseIterable {
        case chat = "Chat", task = "Task", agent = "Agent"

        /// Old cowork sessions are chats (Cowork folded into Chat).
        static func of(_ session: AgentSession) -> RecentType {
            if let agentId = session.agentId, !agentId.isEmpty { return .agent }
            return .chat
        }
    }

    private var isRecentsFiltered: Bool {
        statusFilter != .all || dateFilter != .all || !typeFilter.isEmpty
    }

    @State private var isSettingsPresented = false
    @State private var isSearchPresented = false
    @State private var isQuickTaskPresented = false
    @State private var isScheduledTaskPresented = false
    @State private var isMiniAppsPresented = false
    @State private var isComputersPresented = false
    @State private var isInboxPresented = false
    @State private var isPhonesPresented = false
    @State private var isMeetingsPresented = false
    @State private var isCustomizePresented = false
    @State private var isConnectorsPresented = false
    /// "Continue CLI session": the surface the picked-up session opens in.
    @State private var cliPickerSurface: CLIPickerSurface? = nil
    @ObservedObject private var railTabs = RailTabsStore.shared
    @ObservedObject private var phones = BotPhonesStore.shared

    struct CLIPickerSurface: Identifiable {
        let id: String
    }
    @ObservedObject private var inbox = InboxStore.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @ObservedObject private var botStatus = BotStatusStore.shared

    private let spring = Animation.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)

    var body: some View {
        ZStack(alignment: .topLeading) {
            ZStack(alignment: .bottom) {
                VStack(alignment: .leading, spacing: 0) {
                    header

                    if modeStore.shellMode == .bots {
                        botsRail
                    } else {
                        railList
                    }
                }
                // Content is proposed exactly the drawer width, so nothing
                // wider than the panel can push the rows past its edges.
                .frame(width: width, alignment: .leading)

                footer
                    .frame(width: width)
            }
            .frame(width: width, alignment: .leading)
        }
        // Pinned to the leading edge: content wider than the panel overflows
        // trailing (clipped) instead of spilling off the left of the screen.
        .frame(width: width, alignment: .topLeading)
        .clipped()
        // The layout stays safe-area-respecting so header and footer clear
        // the island and home indicator; MainWorkspaceView paints the
        // panel (rounded trailing corners) edge to edge behind it.
        .sheet(isPresented: $isSettingsPresented) {
            SettingsView()
                // Passed in, not inherited: Sign in closes this sheet and opens the
                // root sign-in sheet in the same tick, and SettingsView redraws
                // once more after its presenter's environment is gone (crash 11-22).
                .environmentObject(authManager)
                .environmentObject(modeStore)
        }
        // Connections › Runtimes "New chat on …" closes Settings onto the chat.
        .onReceive(NotificationCenter.default.publisher(for: .shellNewChatOnRuntime)) { _ in
            isSettingsPresented = false
        }
        .sheet(isPresented: $isQuickTaskPresented) {
            QuickTaskSheet()
        }
        .sheet(isPresented: $isScheduledTaskPresented) {
            CreateAutomationTaskSheet { name, prompt, schedule in
                createScheduledTask(name: name, prompt: prompt, schedule: schedule)
            }
        }
        .sheet(isPresented: $isMiniAppsPresented) {
            MiniAppsStoreView()
        }
        .sheet(isPresented: $isComputersPresented) {
            ConnectComputerSheet()
        }
        .sheet(isPresented: $isInboxPresented) {
            InboxSheet()
        }
        .sheet(isPresented: $isPhonesPresented) {
            PhonesSheet { botId in openBotPhone(botId) }
        }
        .sheet(isPresented: $isMeetingsPresented) {
            MeetingsView()
        }
        .sheet(isPresented: $isCustomizePresented) {
            CustomizeSheet()
        }
        .sheet(isPresented: $isConnectorsPresented) {
            ConnectorsListView()
        }
        .sheet(item: $cliPickerSurface) { picker in
            NativeSessionPickerSheet(surface: picker.id) { sessionId in
                openPickedUpSession(sessionId, surface: picker.id)
            }
        }
        .task {
            // Inbox count, Teammates and the Bots badge read these.
            AgentHubStore.shared.fetchAgentsIfNeeded()
            async let inboxLoad: Void = inbox.refresh()
            async let threadsLoad: Void = botStore.refreshThreads()
            _ = await (inboxLoad, threadsLoad)
            // The Phones badge: unread across the bots' phones.
            await phones.refresh()
        }
        .fullScreenCover(isPresented: $isSearchPresented) {
            ShellSearchView(
                sessions: store.sessions,
                onOpenSession: { open($0) },
                onOpenProject: { project in
                    ProjectStore.shared.selectedProjectId = project.id
                    go(.projects)
                },
                onOpenBot: { bot in openBotChat(bot.id) }
            )
        }
        .alert("Something went wrong", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("OK", role: .cancel) { actionError = nil }
        } message: {
            Text(actionError ?? "")
        }
        .task {
            await store.load()
            #if DEBUG
            // `-open-settings` / `-open-settings-memory` / `-open-usage`
            // (DEBUG only): jump straight to the settings sheet for UI
            // testing/screenshots — no tap injection in simctl.
            if launchArgumentEnabled("open-settings")
                || launchArgumentEnabled("open-settings-memory")
                || launchArgumentEnabled("open-settings-connections")
                || launchArgumentEnabled("open-settings-brain-spike")
                || launchArgumentEnabled("open-settings-meetings")
                || launchArgumentEnabled(LaunchArgument.calmSettings)
                || launchArgumentEnabled(LaunchArgument.calmSettingsAdvanced)
                || launchArgumentEnabled(LaunchArgument.calmSettingsInfo)
                || launchArgumentEnabled(LaunchArgument.calmUsage)
                || launchArgumentEnabled("open-usage") {
                isSettingsPresented = true
            }
            // Meetings (lane 2.11): the sheet, its live page / recap / paste
            // sheet, or the Connectors calendar prompt.
            if launchArgumentEnabled("open-meetings") || launchArgumentEnabled("open-meeting-live")
                || launchArgumentEnabled("open-meeting-recap") || launchArgumentEnabled("open-meetings-paste") {
                isMeetingsPresented = true
            }
            if launchArgumentEnabled("open-connectors-meetings") { isConnectorsPresented = true }
            if launchArgumentEnabled("open-search") || launchArgumentEnabled(LaunchArgument.calmSearch) { isSearchPresented = true }
            if launchArgumentEnabled(LaunchArgument.calmRecentsSelect) || launchArgumentEnabled(LaunchArgument.calmOverviewMenu) {
                // The shell presents the overview; give it a beat to subscribe.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                    NotificationCenter.default.post(name: .shellOpenOverview, object: nil)
                }
            }
            if launchArgumentEnabled("open-quick-task") { isQuickTaskPresented = true }
            #endif
        }
        .onChange(of: isSidebarOpen) { _, isOpen in
            // Refresh each time the drawer is revealed so new chats show up.
            if isOpen {
                Task { await store.load() }
                Task { await phones.refresh() }
            }
        }
        .onChange(of: modeStore.shellMode) { _, _ in
            // Filters belong to the mode's list; a mode switch starts clean.
            statusFilter = .all
            dateFilter = .all
            typeFilter = []
        }
        .onReceive(NotificationCenter.default.publisher(for: .historyMutated)) { _ in
            Task { await store.load() }
        }
    }

    // MARK: - Header

    /// `phone-drawer-top-modes`: mode orb · wordmark · Inbox · search. The orb
    /// is the regular switcher: tap opens the four-mode pill over the line
    /// (the rest of the line fades under it); a pick swaps the rail in place.
    private var header: some View {
        ZStack(alignment: .leading) {
            HStack(spacing: 8) {
                // The orb's slot; the orb floats above so its pill can cover the line.
                Color.clear.frame(width: 40, height: 44)
                ShellWordmark(mode: modeStore.shellMode)
                    .padding(.leading, 6)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .clipped()
                roundButton(
                    systemName: "tray",
                    label: inboxCount == 0 ? "Needs you" : "Needs you, \(inboxCount) waiting",
                    id: "drawerInbox",
                    badge: inboxCount
                ) {
                    isInboxPresented = true
                }
                roundButton(systemName: "magnifyingglass", label: "Search chats, projects and bots", id: "drawerSearch") {
                    isSearchPresented = true
                }
            }
            .opacity(drawerModesOpen ? 0 : 1)
            .allowsHitTesting(!drawerModesOpen)
            .accessibilityHidden(drawerModesOpen)
            ShellModeOrb(size: 40, opensShelves: false, onOpenChange: { drawerModesOpen = $0 })
                .overlay(alignment: .topTrailing) {
                    if !drawerModesOpen && botStore.waitingCount > 0 {
                        Text(botStore.waitingCount > 9 ? "9+" : "\(botStore.waitingCount)")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 4)
                            .frame(minWidth: 16, minHeight: 16)
                            .background(Capsule().fill(Theme.statusError))
                            .offset(x: 4, y: -4)
                            .allowsHitTesting(false)
                            .accessibilityHidden(true)
                    }
                }
                .accessibilityIdentifier("drawerModeOrb")
        }
        .frame(minHeight: 44)
        .animation(drawerModesReduced ? nil : PolishTokens.base, value: drawerModesOpen)
        .padding(.leading, 12)
        .padding(.trailing, 14)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }

    private var inboxCount: Int { inbox.items.count }

    /// 40pt glass circle (GlassCircleButton), red count top-right.
    private func roundButton(systemName: String, label: String, id: String, badge: Int = 0, action: @escaping () -> Void) -> some View {
        GlassCircleButton(systemImage: systemName, label: label, size: 40, action: action)
            .overlay(alignment: .topTrailing) {
                if badge > 0 {
                    Text(badge > 99 ? "99+" : "\(badge)")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundColor(.white)
                        .padding(.horizontal, 4)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(Capsule().fill(Theme.statusError))
                        .offset(x: 4, y: -4)
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                }
            }
            .accessibilityIdentifier(id)
    }

    // MARK: - Rail rows

    private struct RailRow: Identifiable {
        let id: String
        let label: String
        let icon: String
        /// The active row's filled glyph (Phosphor weight="fill").
        var activeIcon: String? = nil
        let isActive: Bool
        /// A count beside the label (Bots: threads waiting on you).
        var badge: Int = 0
        let action: () -> Void
    }

    /// The rail-tabs mode for "Show in rail" (Bots uses Home's, as on the web).
    private var railTabMode: RailTabsStore.TabMode {
        switch modeStore.shellMode {
        case .code: return .code
        case .aci: return .browser
        case .home, .bots: return .home
        }
    }

    /// The mode's rows the user chose to show (PhoneDrawer railRows).
    private var visibleRailRows: [RailRow] {
        let mode = railTabMode
        return railRows.filter { railTabs.isVisible($0.id, in: mode) }
    }

    /// The mode's rail; ids are the web's rail-tab ids. Shared sections keep the mode.
    private var railRows: [RailRow] {
        let tab = modeStore.activeTab
        let projects = RailRow(id: "projects", label: "Projects", icon: "folder", activeIcon: "folder.fill",
                               isActive: tab == .projects) { go(.projects) }
        let artifacts = RailRow(id: "artifacts-library", label: "Artifacts", icon: "doc.text", activeIcon: "doc.text.fill",
                                isActive: tab == .artifacts) { go(.artifacts) }
        switch modeStore.shellMode {
        case .home:
            return [
                projects, artifacts,
                RailRow(id: "automation-tasks", label: "Automation Tasks", icon: "clock", activeIcon: "clock.fill",
                        isActive: tab == .automation) { go(.automation) },
                RailRow(id: "customize", label: "Catalog", icon: "slider.horizontal.3",
                        isActive: tab == .customize) { go(.customize) },
            ]
        case .code:
            return [
                projects, artifacts,
                RailRow(id: "code-automations", label: "Automation Tasks", icon: "clock", activeIcon: "clock.fill",
                        isActive: tab == .automation) { go(.automation) },
            ]
        case .aci:
            // PhoneDrawer's ACI rail: each row opens that ACI view; Teach opens over Agent.
            let aci = ACINavStore.shared
            let onAci = tab == .aci
            func openAci(_ aciTab: ACITab) {
                aci.open(aciTab)
                go(.aci)
            }
            return [
                RailRow(id: "mcp-apps", label: "MCP Apps", icon: "macwindow", isActive: onAci && aci.tab == .apps) { openAci(.apps) },
                RailRow(id: "browser-extensions", label: "ACI Extensions", icon: "puzzlepiece.extension", isActive: onAci && aci.tab == .extensions) { openAci(.extensions) },
                RailRow(id: "computers", label: "Computers", icon: "desktopcomputer", isActive: onAci && aci.tab == .computers) { openAci(.computers) },
                RailRow(id: "site-apis", label: "Teach", icon: "record.circle", isActive: false) {
                    aci.openTeach()
                    go(.aci)
                },
            ]
        case .bots:
            return []
        }
    }

    /// `phone-nav-row` in the drawer: 46pt, 22pt glyph, 17pt label, 12pt
    /// rounded fill when it's the open section.
    private func navRowLabel(_ row: RailRow, trailingDot: Bool = false) -> some View {
        QuietSessionRow(
            title: row.label,
            glyph: row.isActive ? (row.activeIcon ?? row.icon) : row.icon,
            isSelected: row.isActive,
            metrics: .nav
        ) {
            if row.badge > 0 {
                Text(row.badge > 99 ? "99+" : "\(row.badge)")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(.white)
                    .padding(.horizontal, 6)
                    .frame(minHeight: 18)
                    .background(Capsule().fill(Theme.statusError))
            }
        }
        .overlay(alignment: .topLeading) {
            if trailingDot { PhoneRoundDot().offset(x: 32, y: 14) }
        }
    }

    private func railRowView(_ row: RailRow) -> some View {
        Button {
            Haptics.light()
            row.action()
        } label: {
            navRowLabel(row)
        }
        .buttonStyle(DrawerRowPressStyle())
        .drawerRow()
        .accessibilityLabel(row.badge > 0 ? "\(row.label), \(row.badge) waiting" : row.label)
        .accessibilityAddTraits(row.isActive ? [.isSelected] : [])
        .accessibilityIdentifier("drawerRow\(row.id)")
    }

    /// More: choose which tabs show (shared with desktop), Apps & Extensions.
    private var moreRow: some View {
        let mode = railTabMode
        return Menu {
            Section("Show in rail") {
                ForEach(RailTabsStore.tabs(for: mode)) { tab in
                    Button {
                        railTabs.toggle(tab.id, in: mode)
                    } label: {
                        Label(railTabs.isVisible(tab.id, in: mode) ? tab.label : "\(tab.label) · Hidden", systemImage: tab.icon)
                    }
                    .accessibilityIdentifier("railTab\(tab.id)")
                }
            }
            Button {
                isConnectorsPresented = true
            } label: {
                Label("Apps & Extensions", systemImage: "arrow.down.circle")
            }
        } label: {
            navRowLabel(
                RailRow(id: "more", label: "More", icon: "ellipsis", isActive: false) {},
                trailingDot: railTabs.anyHidden(in: mode)
            )
        }
        .buttonStyle(.plain)
        .drawerRow()
        .accessibilityLabel(railTabs.anyHidden(in: mode) ? "More, some tabs hidden" : "More")
        .accessibilityIdentifier("drawerRowmore")
    }

    // MARK: - Home / Code / ACI list

    private var railList: some View {
        List {
            ForEach(visibleRailRows) { railRowView($0) }
            moreRow

            if modeStore.shellMode == .home {
                teammatesSection
            }

            if !pinnedSessions.isEmpty {
                sectionLabel("Pinned")
                ForEach(pinnedSessions) { session in
                    Button {
                        Haptics.light()
                        open(session)
                    } label: {
                        recentRowBody(session)
                    }
                    .buttonStyle(DrawerRowPressStyle())
                    .drawerRow()
                    .accessibilityIdentifier("pinnedRow\(session.id)")
                }
            }

            recentsLabel
            recentsContent

            // Room for the floating footer.
            Color.clear.frame(height: 84).drawerRow()
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .environment(\.defaultMinListRowHeight, 0)
        .scrollDismissesKeyboard(.interactively)
        .refreshable { await store.load() }
    }

    /// Recents (a tap opens every Home session as List · Grid · Deck) · filter.
    private var recentsLabel: some View {
        HStack(spacing: 2) {
            Text("Recents")
                .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
                .accessibilityAddTraits(.isHeader)
            // "View all ›" opens the existing session overview (D7), whose ⋯
            // menu holds Select chats and the All / Pinned / Archived views.
            Button {
                Haptics.light()
                NotificationCenter.default.post(name: .shellOpenOverview, object: nil)
            } label: {
                HStack(spacing: 3) {
                    Text("View all")
                    Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                }
                .frame(minHeight: 32)
                .padding(.horizontal, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("recentsSeeAll")
            .accessibilityHint("Shows all your chats")
            recentsFilterMenu
        }
        .font(.system(size: 14))
        .foregroundColor(Color("TextSecondary"))
        .padding(.leading, 20)
        .padding(.trailing, 14)
        .padding(.top, 12)
        .padding(.bottom, 2)
        .frame(minHeight: 34)
        .drawerRow(inset: false)
    }

    @ViewBuilder
    private var recentsContent: some View {
        if store.isLoading && !store.hasLoaded {
            HStack {
                Spacer()
                ProgressView().padding(.vertical, 24)
                Spacer()
            }
            .drawerRow()
        } else if let loadError = store.loadError, store.sessions.isEmpty {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load conversations",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await store.load() } }
            )
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .drawerRow()
        } else if recentSessions.isEmpty && pinnedSessions.isEmpty {
            emptyState.drawerRow()
        } else {
            ForEach(recentSessions) { recentRow($0) }
                .onMove { from, to in
                    var ids = recentSessions.map(\.id)
                    ids.move(fromOffsets: from, toOffset: to)
                    manualOrderRaw = ids.joined(separator: ",")
                }
        }
    }

    /// The Arc empty state: icon tile, title, line, and the mode's New button.
    private var emptyState: some View {
        let filtered = isRecentsFiltered
        let mode = modeStore.shellMode
        return VStack(spacing: 0) {
            EmptyArt(kind: mode == .aci ? .bell : .envelope, size: 80)
            Text(filtered ? "Nothing matches" : Self.emptyTitle(for: mode))
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .padding(.top, 20)
            Text(filtered ? "Try another filter." : Self.emptyMessage(for: mode))
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .padding(.top, 8)
            Button {
                Haptics.light()
                newItem()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "square.and.pencil").font(.system(size: 13, weight: .medium))
                    Text(newLabel).font(.system(size: 13, weight: .medium))
                }
                .foregroundColor(Color("OnAccent"))
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("AccentPrimary")))
            }
            .buttonStyle(.plain)
            .padding(.top, 20)
            .accessibilityIdentifier("drawerEmptyNew")
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 40)
        .padding(.horizontal, 20)
    }

    private var recentsFilterMenu: some View {
        let filtered = isRecentsFiltered
        return Menu {
            if modeStore.shellMode == .home {
                Section("Type") {
                    ForEach(RecentType.allCases, id: \.self) { type in
                        Toggle(type.rawValue, isOn: Binding(
                            get: { typeFilter.contains(type) },
                            set: { on in
                                if on { typeFilter.insert(type) } else { typeFilter.remove(type) }
                            }
                        ))
                    }
                }
            }
            Picker("Status", selection: $statusFilter) {
                ForEach(RecentsStatusFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.inline)
            Picker("Last activity", selection: $dateFilter) {
                ForEach(RecentsDateFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.inline)
        } label: {
            Image(systemName: "slider.horizontal.3")
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 36, height: 36)
                .overlay(alignment: .topTrailing) {
                    if filtered { PhoneRoundDot().offset(x: -6, y: 6) }
                }
                .contentShape(Circle())
        }
        .accessibilityLabel(filtered ? "Filter recents (filtered)" : "Filter recents")
        .accessibilityIdentifier("recentsFilter")
    }

    /// Gray section label (`phone-rail-label`).
    private func sectionLabel(_ text: String, alert: String? = nil) -> some View {
        HStack(spacing: 6) {
            Text(text)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let alert {
                Text(alert)
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("BrandAccent"))
            }
        }
        .font(.system(size: 14))
        .foregroundColor(Color("TextSecondary"))
        .padding(.leading, 20)
        .padding(.trailing, 14)
        .padding(.top, 12)
        .padding(.bottom, 2)
        .frame(minHeight: 34)
        .drawerRow(inset: false)
        .accessibilityAddTraits(.isHeader)
    }

    /// Claude-iOS row body: icon, title, preview line, and time / Working /
    /// unread dot (PhoneDrawer RecentRowBody).
    private func recentRowBody(_ session: AgentSession) -> some View {
        let working = store.isStreaming(session.id)
        let unread = store.isUnread(session)
        let time = ShellSessionsStore.relativeTime(ShellSessionsStore.timestamp(of: session))
        // C6 (9a server flags): Needs input / Ready for review ride on the row.
        let codeStatus = codeSessionFilterStatus(session, streaming: working)
        let asks = codeStatus.asksForUser
        return QuietSessionRow(
            title: session.displayTitle,
            glyph: ShellSearchView.icon(for: session),
            time: time,
            isSelected: selectedSessionId == session.id,
            emphasized: unread
        ) {
            // Only what the data says: running, asking, or unread.
            if working {
                ProgressView()
                    .controlSize(.mini)
                    .tint(Self.workingCoral)
                    .accessibilityLabel("Working")
            } else if asks {
                CodeSessionStatusIcon(status: codeStatus)
                    .imageScale(.small)
                    .accessibilityLabel(codeStatus.label)
            } else if unread {
                Circle()
                    .fill(Color.blue)
                    .frame(width: 8, height: 8)
                    .accessibilityLabel("Unread")
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// The web's working accent (#D97757, `.phone-rail-working`).
    static let workingCoral = Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)

    /// One recent. Swipe left (a long swipe commits) deletes it; long-press
    /// lifts it to drag into your own order; a tap opens it.
    private func recentRow(_ session: AgentSession) -> some View {
        Button {
            Haptics.light()
            open(session)
        } label: {
            recentRowBody(session)
        }
        .buttonStyle(DrawerRowPressStyle())
        .drawerRow()
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            Button(role: .destructive) {
                delete(session)
            } label: {
                Label("Delete chat", systemImage: "trash")
            }
        }
        .accessibilityIdentifier("recentRow\(session.id)")
        .accessibilityHint("Swipe left to delete")
    }

    // MARK: - Bots rail

    /// The desktop Bots rail, touch-sized (PhoneDrawer BotRail).
    private var botsRail: some View {
        BotsDrawerRail(
            onOpen: { route in
                if let route {
                    botStore.pendingRoute = route
                } else {
                    botStore.pendingHome = true
                }
                go(.agents)
            },
            onOpenBot: { openBotChat($0) },
            onOpenProjects: { go(.projects) }
        )
    }

    // MARK: - Footer

    /// `phone-rail-foot`: avatar with the usage ring, Phones, and the New
    /// pill, floating over the list as it fades out underneath.
    private var footer: some View {
        HStack(spacing: 10) {
            accountAvatar
            PhonesDrawerButton { isPhonesPresented = true }
            MeetingsDrawerButton { isMeetingsPresented = true }
            Spacer(minLength: 0)
            newPill
        }
        .padding(.horizontal, 14)
        .padding(.top, 28)
        .padding(.bottom, 14)
        .background(
            LinearGradient(
                stops: [
                    .init(color: Color("BgPrimary").opacity(0), location: 0),
                    .init(color: Color("BgPrimary"), location: 0.42),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea(edges: .bottom)
            .allowsHitTesting(false)
        )
    }

    /// The account avatar inside the usage ring (PhoneAccountAvatar): 40pt
    /// ring, 2.5pt stroke, the plan's weekly use as the arc. Opens Settings.
    private var accountAvatar: some View {
        let ratio = min(1, max(0, (usage.percentUsed ?? 0) / 100))
        let stroke: Color = ratio >= 1 ? Theme.statusError : (ratio >= 0.8 ? Theme.statusWarning : Color("TextPrimary"))
        let usageText = usage.percentUsed.map { ", Allternit Cloud \(Int(($0).rounded()))% used" } ?? ""
        return Button {
            Haptics.light()
            isSettingsPresented = true
        } label: {
            ZStack {
                Circle()
                    .stroke(Color("BorderSubtle"), lineWidth: 2.5)
                    .frame(width: 37.5, height: 37.5)
                if usage.percentUsed != nil, ratio > 0 {
                    Circle()
                        .trim(from: 0, to: ratio)
                        .stroke(stroke, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        .frame(width: 37.5, height: 37.5)
                }
                Text(authManager.avatarInitial)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(Color("BorderStrong")))
            }
            .frame(width: 44, height: 44)
            .polishGlass(cornerRadius: 22)
            .contentShape(Circle())
        }
        .buttonStyle(DrawerCirclePressStyle())
        .accessibilityLabel("\(authManager.displayName). Settings\(usageText)")
        .accessibilityIdentifier("drawerSettings")
    }

    private struct NewOption: Identifiable {
        let id: String
        let label: String
        let icon: String
        let action: () -> Void
    }

    private var newLabel: String {
        switch modeStore.shellMode {
        case .home: return "New chat"
        case .bots, .code: return "New thread"
        case .aci: return "New session"
        }
    }

    /// Press-and-hold ways to start (PhoneDrawer newOptions). "Continue CLI
    /// session" picks up a Claude Code, Codex, Grok… session from your computer.
    private var newOptions: [NewOption] {
        let quickTask = NewOption(id: "quick-task", label: "Quick task", icon: "pip") { isQuickTaskPresented = true }
        let scheduled = NewOption(id: "scheduled", label: "New scheduled task", icon: "clock") { isScheduledTaskPresented = true }
        func continueCLI(_ surface: String) -> NewOption {
            NewOption(id: "continue-cli", label: "Continue CLI session", icon: "apple.terminal") {
                cliPickerSurface = CLIPickerSurface(id: surface)
            }
        }
        switch modeStore.shellMode {
        case .home:
            return [NewOption(id: "chat", label: "New chat", icon: "bubble.left.and.text.bubble.right") { newItem() },
                    continueCLI("chat"), scheduled, quickTask]
        case .code:
            return [NewOption(id: "thread", label: "New thread", icon: "chevron.left.forwardslash.chevron.right") { newItem() },
                    continueCLI("code"), scheduled, quickTask]
        case .aci:
            return [NewOption(id: "session", label: "New session", icon: "globe") { newItem() }, quickTask]
        case .bots:
            return [NewOption(id: "thread", label: "New thread", icon: "bubble.left.and.text.bubble.right") { newItem() }, quickTask]
        }
    }

    /// The floating New pill: 44pt, elevated surface, hairline and soft
    /// shadow. Tap starts the mode's new item; press and hold opens the menu.
    private var newPill: some View {
        Menu {
            ForEach(newOptions) { option in
                Button(action: option.action) {
                    Label(option.label, systemImage: option.icon)
                }
            }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "plus")
                    .font(.system(size: 17, weight: .semibold))
                Text(newLabel)
                    .font(.system(size: 16, weight: .medium))
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 18)
            .frame(height: 44)
            .polishGlass(cornerRadius: 22)
            .shadow(color: Color.black.opacity(0.10), radius: 8, x: 0, y: 4)
        } primaryAction: {
            Haptics.light()
            newItem()
        }
        .buttonStyle(DrawerCirclePressStyle(scale: PolishTokens.pressPill))
        .accessibilityLabel("\(newLabel). Press and hold for more ways to start")
        .accessibilityIdentifier("drawerNewPill")
    }

    // MARK: - Actions

    private func closeDrawer() {
        withAnimation(spring) { isSidebarOpen = false }
    }

    /// Opens a bot's main chat in Bots mode (search, Teammates).
    private func openBotChat(_ botId: String) {
        botStore.pendingOpenBotId = botId
        go(.agents)
    }

    /// Opens a bot's phone in Bots mode (the Phones list).
    private func openBotPhone(_ botId: String) {
        botStore.pendingOpenPhoneBotId = botId
        go(.agents)
    }

    /// A CLI session was picked up: open it where it belongs.
    private func openPickedUpSession(_ sessionId: String, surface: String) {
        modeStore.selectBarItem(surface == "code" ? .code : .chats)
        selectedSessionId = sessionId
        closeDrawer()
        Task { await store.load() }
    }

    // MARK: - Teammates

    /// Bots waiting on you, working, or active in the last 15 minutes, in
    /// that order (the PWA drawer's TeammatesStrip).
    private var teammates: [(bot: AgentRecord, needsYou: Bool, working: Bool)] {
        let recent = ISO8601DateFormatter()
        recent.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let cutoff = Date().addingTimeInterval(-15 * 60)
        let entries = botStore.bots.compactMap { bot -> (bot: AgentRecord, needsYou: Bool, working: Bool, rank: Int)? in
            let needsYou = botStore.waitingCount(for: bot.id) > 0 || botStatus.needsAttention(for: bot.id)
            let working = botStore.workingCount(for: bot.id) > 0 || botStatus.isWorking(for: bot.id)
            let active = botStore.threads(for: bot.id).contains {
                (recent.date(from: $0.lastActivityAt) ?? ISO8601DateFormatter().date(from: $0.lastActivityAt) ?? .distantPast) > cutoff
            }
            guard needsYou || working || active else { return nil }
            return (bot, needsYou, working, needsYou ? 0 : working ? 1 : 2)
        }
        return entries.sorted { $0.rank < $1.rank }.prefix(8).map { ($0.bot, $0.needsYou, $0.working) }
    }

    @ViewBuilder
    private var teammatesSection: some View {
        let list = teammates
        if !list.isEmpty {
            let needsYou = list.filter(\.needsYou).count
            sectionLabel("Teammates", alert: needsYou > 0 ? "\(needsYou) need\(needsYou == 1 ? "s" : "") you" : nil)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 10) {
                    ForEach(list, id: \.bot.id) { entry in
                        Button {
                            Haptics.light()
                            openBotChat(entry.bot.id)
                        } label: {
                            BotStatusAvatar(bot: entry.bot, size: 36, working: entry.working && !entry.needsYou, waiting: entry.needsYou)
                                .frame(width: 44, height: 44)
                                .contentShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(entry.bot.botDisplayName
                            + (entry.needsYou ? ", needs you" : entry.working ? ", working" : ""))
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .padding(.bottom, 2)
            }
            .drawerRow(inset: false)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Teammates")
        }
    }

    private func go(_ tab: ModeBarItem) {
        modeStore.selectBarItem(tab)
        closeDrawer()
    }

    /// Opens a session on its own surface: bot threads via the shared
    /// open-chat notification, code threads in Code, ACI runs in ACI, the
    /// rest in Home.
    private func open(_ session: AgentSession) {
        if session.agentId != nil {
            NotificationCenter.default.post(name: .openChatSession, object: nil, userInfo: [
                "sessionId": session.id,
                "agentId": session.agentId ?? "",
            ])
            closeDrawer()
            return
        }
        switch session.originSurface ?? "" {
        case "code": modeStore.selectBarItem(.code)
        case "browser": modeStore.selectBarItem(.aci)
        default: modeStore.selectBarItem(.chats)
        }
        selectedSessionId = session.id
        store.markSeen(session.id)
        closeDrawer()
    }

    /// The New pill's tap: the mode's new item.
    private func newItem() {
        switch modeStore.shellMode {
        case .home:
            selectedSessionId = nil
            modeStore.selectBarItem(.chats)
            NotificationCenter.default.post(name: .shellNewChat, object: nil)
        case .code:
            selectedSessionId = nil
            modeStore.selectBarItem(.code)
        case .aci:
            selectedSessionId = nil
            ACINavStore.shared.open(.agent)
            modeStore.selectBarItem(.aci)
        case .bots:
            modeStore.selectBarItem(.agents)
            botStore.pendingNewThread = true
        }
        closeDrawer()
    }

    private func delete(_ session: AgentSession) {
        Haptics.medium()
        let removed = store.remove(id: session.id)
        pinsStore.remove(id: session.id)
        manualOrderRaw = manualOrder.filter { $0 != session.id }.joined(separator: ",")
        if selectedSessionId == session.id {
            selectedSessionId = nil
            NotificationCenter.default.post(name: .shellNewChat, object: nil)
        }
        // Plan chats (pth_…) live in the cloud plan chat store (§9.10): delete
        // the stored thread, or it returns on the next list refresh.
        if PlanThreadClient.isThreadId(session.id) {
            Task {
                do {
                    try await PlanThreadClient.shared.delete(id: session.id)
                } catch {
                    store.restore(removed)
                    actionError = "Couldn't delete “\(session.name ?? "Untitled")”: \(FriendlyErrorMessage.from(error.localizedDescription))"
                }
            }
            return
        }
        Task {
            do {
                // DELETE /api/v1/agent-sessions/:id (same call as Settings'
                // bulk delete and ChatViewModel's incognito purge).
                let escaped = session.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? session.id
                try await APIClient.shared.delete(path: "agent-sessions/\(escaped)")
            } catch {
                store.restore(removed)
                actionError = "Couldn't delete “\(session.name ?? "Untitled")”: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func createScheduledTask(name: String, prompt: String, schedule: String) {
        Task {
            do {
                try await CronJobStore.shared.createAgentJob(name: name, prompt: prompt, schedule: schedule)
                modeStore.automationKind = .cron
                go(.automation)
            } catch {
                actionError = "Couldn't create the scheduled task: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    // MARK: - Filtering

    private func belongsToMode(_ session: AgentSession) -> Bool {
        ShellSessionsStore.belongsToRecents(session, mode: modeStore.shellMode)
    }

    /// Home's pins (desktop rail pins), newest first, at most 10.
    private var pinnedSessions: [AgentSession] {
        guard modeStore.shellMode == .home else { return [] }
        let byId = Dictionary(store.sessions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return pinsStore.pins
            .sorted { $0.pinnedAt > $1.pinnedAt }
            .compactMap { byId[$0.id] }
            .filter { belongsToMode($0) }
            .prefix(10)
            .map { $0 }
    }

    private var manualOrder: [String] {
        manualOrderRaw.split(separator: ",").map(String.init)
    }

    private var recentSessions: [AgentSession] {
        let pinnedIds = Set(pinnedSessions.map(\.id))
        let calendar = Calendar.current
        let startOfToday = calendar.startOfDay(for: Date())
        let weekAgo = calendar.date(byAdding: .day, value: -7, to: startOfToday) ?? startOfToday
        let monthAgo = calendar.date(byAdding: .month, value: -1, to: startOfToday) ?? startOfToday
        let filtered = store.sessions
            .filter { session in
                guard belongsToMode(session), !pinnedIds.contains(session.id) else { return false }
                if modeStore.shellMode == .home, !typeFilter.isEmpty, !typeFilter.contains(RecentType.of(session)) {
                    return false
                }
                let isOpen = session.active && !session.archived
                switch statusFilter {
                case .all, .active: if !isOpen { return false }
                case .completed, .archived: if isOpen { return false }
                }
                let date = ShellSessionsStore.timestamp(of: session)
                switch dateFilter {
                case .all: return true
                case .today: return date >= startOfToday
                case .week: return date >= weekAgo
                case .month: return date >= monthAgo
                }
            }
            .prefix(40)
        // orderRecents: manual keys first, in that order; the rest by recency.
        var rank: [String: Int] = [:]
        for (index, id) in manualOrder.enumerated() where rank[id] == nil { rank[id] = index }
        let manual = filtered.filter { rank[$0.id] != nil }.sorted { (rank[$0.id] ?? 0) < (rank[$1.id] ?? 0) }
        let rest = filtered.filter { rank[$0.id] == nil }
        return manual + rest
    }

    // MARK: - Helpers

    private static func emptyTitle(for mode: ShellMode) -> String {
        switch mode {
        case .home: return "No chats yet"
        case .bots: return "No threads yet"
        case .code: return "No code threads yet"
        case .aci: return "No runs yet"
        }
    }

    private static func emptyMessage(for mode: ShellMode) -> String {
        switch mode {
        case .home: return "Your chats show up here."
        case .bots: return "Your bot threads show up here."
        case .code: return "Your code threads show up here."
        case .aci: return "Your agent runs show up here."
        }
    }
}

private extension View {
    /// A drawer List row: no separator or background, 8pt side margins
    /// (`phone-nav` / `phone-rail-row`), or edge to edge for labels and strips.
    func drawerRow(inset: Bool = true) -> some View {
        self
            .listRowInsets(EdgeInsets(top: 0, leading: inset ? 8 : 0, bottom: 0, trailing: inset ? 8 : 0))
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
    }
}

/// Drawer row press: a rounded neutral highlight under the finger.
private struct DrawerRowPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(configuration.isPressed ? Color("FillNeutral") : Color.clear)
            )
    }
}

/// Press scale for the drawer's glass controls (off under Reduce Motion).
private struct DrawerCirclePressStyle: ButtonStyle {
    var scale: CGFloat = PolishTokens.pressButton
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
    }
}

extension Notification.Name {
    /// Posted by bulk data-control operations (archive/delete all chats) so
    /// the history sidebar refreshes instead of showing stale rows.
    static let historyMutated = Notification.Name("com.allternit.historyMutated")
    /// Posted by Bot Home to open a chat session (userInfo: sessionId, agentId).
    static let openChatSession = Notification.Name("com.allternit.openChatSession")
    /// The drawer's New chat: ChatView starts a fresh conversation even when
    /// the selection was already nil.
    static let shellNewChat = Notification.Name("com.allternit.shellNewChat")
    /// The header count button and the drawer's "See all": open the mode's
    /// session overview (MainWorkspaceView presents it).
    static let shellOpenOverview = Notification.Name("com.allternit.shellOpenOverview")
    /// Open the drawer from anywhere (overview ☰, section screens).
    static let shellOpenDrawer = Notification.Name("com.allternit.shellOpenDrawer")
}

extension Notification {
    /// The non-empty `sessionId` of an `.openChatSession` post, else nil.
    var openChatSessionId: String? {
        guard let id = userInfo?["sessionId"] as? String, !id.isEmpty else { return nil }
        return id
    }
}
