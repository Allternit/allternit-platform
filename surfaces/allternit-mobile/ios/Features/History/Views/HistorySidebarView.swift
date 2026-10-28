import SwiftUI
import UIKit

/// A dated bucket of sessions (Today / Yesterday / Previous 7 Days / Older).
struct HistoryGroup: Identifiable {
    let id: String
    let title: String
    let sessions: [AgentSession]
}

/// Recents status filter (PhoneDrawer STATUS_OPTIONS). "All" hides archived
/// sessions (`active == false`), as on the web.
enum RecentsStatusFilter: String, CaseIterable {
    case all = "All"
    case active = "Active"
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
/// the active mode's rail, laid out like Claude iOS. Top line: the mode dial,
/// the mode's wordmark and a round search button. Then the mode's rail rows
/// (Projects, Artifacts, Automation Tasks, …), Pinned, and Recents with a
/// filter — swipe a recent left to delete it, right to pin it. Footer: the
/// account avatar (opens Settings) and the New pill (tap for the mode's new
/// item, press and hold for more ways to start).
///
/// Bots mode shows the Bots rail (lane L4 owns its contents).
struct HistorySidebarView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool
    /// Drawer width — 92% of the screen, set by MainWorkspaceView.
    var width: CGFloat = 320

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @EnvironmentObject private var authManager: AuthManager
    @StateObject private var pinsStore = RailPinsStore.shared

    @State private var sessions: [AgentSession] = []
    @State private var hasLoaded = false
    @State private var isLoading = false
    @State private var loadError: String? = nil
    @State private var actionError: String? = nil

    @State private var statusFilter: RecentsStatusFilter = .all
    @State private var dateFilter: RecentsDateFilter = .all

    @State private var isDialExpanded = false
    @State private var isSettingsPresented = false
    @State private var isSearchPresented = false
    @State private var isQuickTaskPresented = false
    @State private var isScheduledTaskPresented = false
    @State private var isMiniAppsPresented = false
    @State private var isDecisionsPresented = false

    private let spring = Animation.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)

    var body: some View {
        ZStack(alignment: .topLeading) {
            VStack(alignment: .leading, spacing: 0) {
                header

                if modeStore.shellMode == .bots {
                    botsRail
                } else {
                    railList
                }

                footer
            }

            if isDialExpanded {
                // Tap away closes the quarter-moon.
                Color.black.opacity(0.06)
                    .ignoresSafeArea()
                    .onTapGesture {
                        withAnimation(spring) { isDialExpanded = false }
                    }
                    .accessibilityHidden(true)
                ModeDialMenu(isExpanded: $isDialExpanded)
                    .padding(.leading, 12)
                    .padding(.top, 8)
            }
        }
        .frame(width: width)
        // Only the background bleeds edge-to-edge (behind the status bar /
        // home indicator); the layout stays safe-area-respecting so header
        // and footer clear the island and home indicator. The closed drawer
        // is hidden by the content pane's opaque backdrop (MainWorkspaceView).
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.vertical))
        .sheet(isPresented: $isSettingsPresented) {
            SettingsView()
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
        .sheet(isPresented: $isDecisionsPresented) {
            DecisionsScreen()
        }
        .fullScreenCover(isPresented: $isSearchPresented) {
            ShellSearchView(
                sessions: sessions,
                onOpenSession: { open($0) },
                onOpenProject: { project in
                    ProjectStore.shared.selectedProjectId = project.id
                    go(.projects)
                },
                onOpenBot: { bot in
                    go(.agents)
                    NotificationCenter.default.post(name: .openBot, object: nil, userInfo: ["agentId": bot.id])
                }
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
            await loadSessions()
            #if DEBUG
            // `-open-settings` / `-open-settings-memory` / `-open-usage`
            // (DEBUG only): jump straight to the settings sheet for UI
            // testing/screenshots — no tap injection in simctl.
            if launchArgumentEnabled("open-settings")
                || launchArgumentEnabled("open-settings-memory")
                || launchArgumentEnabled("open-settings-brain-spike")
                || launchArgumentEnabled("open-usage") {
                isSettingsPresented = true
            }
            if launchArgumentEnabled("open-search") { isSearchPresented = true }
            if launchArgumentEnabled("open-quick-task") { isQuickTaskPresented = true }
            if launchArgumentEnabled("open-mode-dial") { isDialExpanded = true }
            #endif
        }
        .onChange(of: isSidebarOpen) { _, isOpen in
            // Refresh each time the drawer is revealed so new chats show up;
            // a closing drawer folds the dial.
            if isOpen {
                Task { await loadSessions() }
            } else {
                isDialExpanded = false
            }
        }
        .onChange(of: modeStore.shellMode) { _, _ in
            // Filters belong to the mode's list; a mode switch starts clean.
            statusFilter = .all
            dateFilter = .all
        }
        .onReceive(NotificationCenter.default.publisher(for: .historyMutated)) { _ in
            Task { await loadSessions() }
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            ModeDialButton(isExpanded: $isDialExpanded)
            ShellWordmark(mode: modeStore.shellMode)
            Spacer(minLength: 8)
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                isSearchPresented = true
            } label: {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(Color("BgPanel")))
                    .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Search chats, projects and bots")
            .accessibilityIdentifier("drawerSearch")
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 8)
    }

    // MARK: - Rail rows

    private struct RailRow: Identifiable {
        let id: String
        let label: String
        let icon: String
        let isActive: Bool
        let action: () -> Void
    }

    /// The mode's rail (PhoneDrawer railRows). Shared sections keep the mode.
    private var railRows: [RailRow] {
        let tab = modeStore.activeTab
        let projects = RailRow(id: "projects", label: "Projects", icon: "folder", isActive: tab == .projects) { go(.projects) }
        let artifacts = RailRow(id: "artifacts", label: "Artifacts", icon: "doc.text", isActive: tab == .artifacts) { go(.artifacts) }
        let automation = RailRow(id: "automation", label: "Automation Tasks", icon: "clock", isActive: tab == .automation) { go(.automation) }
        let decisions = RailRow(id: "decisions", label: "Decisions", icon: "checkmark.seal", isActive: false) { isDecisionsPresented = true }
        switch modeStore.shellMode {
        case .home:
            return [
                projects, artifacts, automation, decisions,
                RailRow(id: "models", label: "Models", icon: "brain.head.profile", isActive: tab == .models) { go(.models) },
            ]
        case .code:
            return [projects, artifacts, automation, decisions]
        case .aci:
            return [
                RailRow(id: "mcp-apps", label: "MCP Apps", icon: "square.grid.2x2", isActive: false) {
                    isMiniAppsPresented = true
                },
            ]
        case .bots:
            return []
        }
    }

    private func railRowView(_ row: RailRow) -> some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            row.action()
        } label: {
            HStack(spacing: 14) {
                Image(systemName: row.icon)
                    .font(.system(size: 17, weight: row.isActive ? .semibold : .regular))
                    .frame(width: 24)
                Text(row.label)
                    .font(.body)
                    .fontWeight(row.isActive ? .semibold : .regular)
                Spacer()
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(row.isActive ? modeStore.shellMode.accent.opacity(0.14) : Color.clear)
        .accessibilityLabel(row.label)
        .accessibilityAddTraits(row.isActive ? [.isSelected] : [])
        .accessibilityIdentifier("drawerRow\(row.id)")
    }

    // MARK: - Home / Code / ACI list

    private var railList: some View {
        List {
            Section {
                ForEach(railRows) { railRowView($0) }
            }

            activeBotSection

            if !pinnedSessions.isEmpty {
                Section {
                    ForEach(pinnedSessions) { recentRow($0) }
                } header: {
                    sectionLabel("Pinned")
                }
            }

            Section {
                recentsContent
            } header: {
                HStack {
                    sectionLabel("Recents")
                    Spacer()
                    recentsFilterMenu
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .environment(\.defaultMinListRowHeight, 44)
        .scrollDismissesKeyboard(.interactively)
        .refreshable { await loadSessions() }
    }

    @ViewBuilder
    private var recentsContent: some View {
        if isLoading && !hasLoaded {
            HStack {
                Spacer()
                ProgressView().padding(.vertical, 24)
                Spacer()
            }
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        } else if let loadError, sessions.isEmpty {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load conversations",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await loadSessions() } }
            )
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        } else if recentSessions.isEmpty {
            let filtered = statusFilter != .all || dateFilter != .all
            FriendlyInlineStateView(
                style: .empty,
                icon: Self.emptyIcon(for: modeStore.shellMode),
                title: filtered ? "Nothing matches" : Self.emptyTitle(for: modeStore.shellMode),
                message: filtered ? "Try another filter." : Self.emptyMessage(for: modeStore.shellMode),
                actionTitle: filtered ? "Clear filters" : newLabel,
                action: {
                    if filtered {
                        statusFilter = .all
                        dateFilter = .all
                    } else {
                        newItem()
                    }
                }
            )
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        } else {
            ForEach(recentSessions) { recentRow($0) }
        }
    }

    private var recentsFilterMenu: some View {
        let filtered = statusFilter != .all || dateFilter != .all
        return Menu {
            Picker("Status", selection: $statusFilter) {
                ForEach(RecentsStatusFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            Picker("Last activity", selection: $dateFilter) {
                ForEach(RecentsDateFilter.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
        } label: {
            ZStack(alignment: .topTrailing) {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 28)
                if filtered {
                    Circle().fill(modeStore.shellMode.accent).frame(width: 6, height: 6).offset(x: -4, y: 4)
                }
            }
        }
        .accessibilityLabel(filtered ? "Filter recents (filtered)" : "Filter recents")
        .accessibilityIdentifier("recentsFilter")
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
            .textCase(nil)
    }

    /// One recent: icon, title, relative time. Swipe left deletes, swipe
    /// right pins / unpins; a tap opens it.
    private func recentRow(_ session: AgentSession) -> some View {
        let isSelected = selectedSessionId == session.id
        let isPinned = pinsStore.isPinned(session.id)
        return Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            open(session)
        } label: {
            HStack(spacing: 12) {
                Image(systemName: ShellSearchView.icon(for: session))
                    .font(.subheadline)
                    .foregroundColor(isSelected ? modeStore.shellMode.accent : Color("TextSecondary"))
                    .frame(width: 20)
                Text(session.name ?? "Untitled")
                    .font(.subheadline)
                    .fontWeight(isSelected ? .semibold : .regular)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 6)
                if !session.active {
                    Text("Archived")
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }
                Text(Self.relativeTime(Self.timestamp(of: session)))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(minHeight: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(isSelected ? Color("BgTertiary").opacity(0.5) : Color.clear)
        .listRowSeparator(.hidden)
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            Button(role: .destructive) {
                delete(session)
            } label: {
                Label("Delete", systemImage: "trash")
            }
        }
        .swipeActions(edge: .leading, allowsFullSwipe: true) {
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                pinsStore.toggle(id: session.id, kind: Self.kind(of: session), mode: session.originSurface ?? "chat")
            } label: {
                Label(isPinned ? "Unpin" : "Pin", systemImage: isPinned ? "pin.slash" : "pin")
            }
            .tint(modeStore.shellMode.accent)
        }
        .accessibilityIdentifier("recentRow\(session.id)")
        .accessibilityHint("Swipe left to delete, right to \(isPinned ? "unpin" : "pin")")
    }

    // MARK: - Bots rail

    private var botsRail: some View {
        List {
            Section {
                railRowView(RailRow(id: "bots", label: "Bots", icon: "cpu", isActive: modeStore.activeTab == .agents) { go(.agents) })
                railRowView(RailRow(id: "projects", label: "Projects", icon: "folder", isActive: modeStore.activeTab == .projects) { go(.projects) })
                railRowView(RailRow(id: "artifacts", label: "Artifacts", icon: "doc.text", isActive: modeStore.activeTab == .artifacts) { go(.artifacts) })
                railRowView(RailRow(id: "automation", label: "Automation Tasks", icon: "clock", isActive: modeStore.activeTab == .automation) { go(.automation) })
                railRowView(RailRow(id: "decisions", label: "Decisions", icon: "checkmark.seal", isActive: false) { isDecisionsPresented = true })
            }
            activeBotSection
            if !recentSessions.isEmpty {
                Section {
                    ForEach(recentSessions) { recentRow($0) }
                } header: {
                    sectionLabel("Threads")
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .environment(\.defaultMinListRowHeight, 44)
        .refreshable { await loadSessions() }
    }

    // MARK: - Footer

    /// Account avatar (opens Settings) and the New pill floating over the
    /// drawer background — no separator bar.
    private var footer: some View {
        HStack(spacing: 10) {
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                isSettingsPresented = true
            } label: {
                Text(authManager.avatarInitial)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("BgPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(Color("TextSecondary")))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Settings, signed in as \(authManager.displayName)")
            .accessibilityIdentifier("drawerSettings")

            Spacer()

            newPill
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
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
    /// session" is left out: iOS has no native-session picker yet.
    private var newOptions: [NewOption] {
        let quickTask = NewOption(id: "quick-task", label: "Quick task", icon: "pip") { isQuickTaskPresented = true }
        let scheduled = NewOption(id: "scheduled", label: "New scheduled task", icon: "clock") { isScheduledTaskPresented = true }
        switch modeStore.shellMode {
        case .home:
            return [NewOption(id: "chat", label: "New chat", icon: "square.and.pencil") { newItem() }, scheduled, quickTask]
        case .code:
            return [NewOption(id: "thread", label: "New thread", icon: "terminal") { newItem() }, scheduled, quickTask]
        case .aci:
            return [NewOption(id: "session", label: "New session", icon: "globe") { newItem() }, quickTask]
        case .bots:
            return [NewOption(id: "thread", label: "New thread", icon: "bubble.left") { newItem() }, quickTask]
        }
    }

    private var newPill: some View {
        Menu {
            ForEach(newOptions) { option in
                Button(action: option.action) {
                    Label(option.label, systemImage: option.icon)
                }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "plus")
                    .font(.system(size: 14, weight: .bold))
                Text(newLabel)
                    .font(.system(size: 14, weight: .semibold))
            }
            .foregroundColor(Color("BgPrimary"))
            .padding(.horizontal, 16)
            .frame(height: 40)
            .background(Capsule().fill(Color("TextPrimary")))
        } primaryAction: {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            newItem()
        }
        .accessibilityLabel("\(newLabel). Press and hold for more ways to start")
        .accessibilityIdentifier("drawerNewPill")
    }

    // MARK: - Active bot

    /// True when agent/bot mode is on for the current surface and a registry
    /// agent is selected: the drawer surfaces that bot as the mounted session.
    private var showActiveBot: Bool {
        agentModeStore.isAgentEnabled(for: modeStore.mode)
            && agentModeStore.selectedAgent(for: modeStore.mode) != nil
    }

    @ViewBuilder
    private var activeBotSection: some View {
        if showActiveBot, let agent = agentModeStore.selectedAgent(for: modeStore.mode) {
            Section {
                Button {
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    selectedSessionId = nil
                    go(.chats)
                } label: {
                    HStack(spacing: 12) {
                        AgentAvatarView(agent: agent, size: 28)
                        Text(agent.name)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Spacer()
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 15))
                            .foregroundColor(Color("AccentPrimary"))
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .listRowBackground(Color.clear)
            } header: {
                sectionLabel("Active bot")
            }
        }
    }

    // MARK: - Actions

    private func closeDrawer() {
        withAnimation(spring) { isSidebarOpen = false }
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
            modeStore.selectBarItem(.aci)
        case .bots:
            modeStore.selectBarItem(.agents)
            // L4 hook: the Bots root starts a thread with the main bot.
            NotificationCenter.default.post(name: .botsNewThreadRequested, object: nil)
        }
        closeDrawer()
    }

    private func delete(_ session: AgentSession) {
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        let removed = sessions
        sessions.removeAll { $0.id == session.id }
        pinsStore.remove(id: session.id)
        if selectedSessionId == session.id {
            selectedSessionId = nil
            NotificationCenter.default.post(name: .shellNewChat, object: nil)
        }
        Task {
            do {
                // DELETE /api/v1/agent-sessions/:id (same call as Settings'
                // bulk delete and ChatViewModel's incognito purge).
                let escaped = session.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? session.id
                try await APIClient.shared.delete(path: "agent-sessions/\(escaped)")
            } catch {
                sessions = removed
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

    /// Sessions that belong to the current mode's drawer. Bot sessions (with
    /// an `agent_id`) live in Bots; Code and ACI keep their own surfaces;
    /// incognito sessions are excluded server-side.
    private func belongsToMode(_ session: AgentSession) -> Bool {
        let surface = session.originSurface ?? ""
        switch modeStore.shellMode {
        case .bots:
            return session.agentId != nil
        case .home:
            return session.agentId == nil && (surface == "chat" || surface == "cowork" || surface.isEmpty)
        case .code:
            return session.agentId == nil && surface == "code"
        case .aci:
            return session.agentId == nil && surface == "browser"
        }
    }

    private var pinnedSessions: [AgentSession] {
        let byId = Dictionary(sessions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return pinsStore.pins
            .sorted { $0.pinnedAt > $1.pinnedAt }
            .compactMap { byId[$0.id] }
            .filter { belongsToMode($0) }
            .prefix(10)
            .map { $0 }
    }

    private var recentSessions: [AgentSession] {
        let pinnedIds = Set(pinnedSessions.map(\.id))
        let calendar = Calendar.current
        let startOfToday = calendar.startOfDay(for: Date())
        let weekAgo = calendar.date(byAdding: .day, value: -7, to: startOfToday) ?? startOfToday
        let monthAgo = calendar.date(byAdding: .month, value: -1, to: startOfToday) ?? startOfToday
        return sessions
            .filter { session in
                guard belongsToMode(session), !pinnedIds.contains(session.id) else { return false }
                switch statusFilter {
                case .all, .active: if !session.active { return false }
                case .archived: if session.active { return false }
                }
                let date = Self.timestamp(of: session)
                switch dateFilter {
                case .all: return true
                case .today: return date >= startOfToday
                case .week: return date >= weekAgo
                case .month: return date >= monthAgo
                }
            }
            .prefix(60)
            .map { $0 }
    }

    // MARK: - Loading

    @MainActor
    private func loadSessions() async {
        if !hasLoaded { isLoading = true }
        loadError = nil
        do {
            // GET /api/v1/agent-sessions → { sessions: [...], count }
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            sessions = envelope.sessions.sorted { Self.timestamp(of: $0) > Self.timestamp(of: $1) }
            hasLoaded = true
        } catch is CancellationError {
            // View disappeared mid-flight — keep the current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    // MARK: - Helpers

    private static func kind(of session: AgentSession) -> String {
        if session.agentId != nil { return "agent" }
        switch session.originSurface ?? "" {
        case "code": return "code"
        case "browser": return "browser"
        default: return "chat"
        }
    }

    private static func emptyIcon(for mode: ShellMode) -> String {
        switch mode {
        case .home: return "bubble.left"
        case .bots: return "cpu"
        case .code: return "terminal"
        case .aci: return "globe"
        }
    }

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

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    private static func relativeTime(_ date: Date) -> String {
        guard date != .distantPast else { return "" }
        if Date().timeIntervalSince(date) < 60 { return "now" }
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    static func timestamp(of record: AgentSession) -> Date {
        parseTimestamp(record.updatedAt) ?? parseTimestamp(record.createdAt) ?? .distantPast
    }

    /// Backend timestamps are RFC-3339 (chrono `to_rfc3339`), with or without
    /// fractional seconds.
    private static func parseTimestamp(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
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
    /// Search picked a bot (userInfo: agentId). The Bots root (lane L4)
    /// opens that bot's page.
    static let openBot = Notification.Name("com.allternit.openBot")
    /// The drawer's New thread in Bots mode. The Bots root (lane L4) starts
    /// a thread with the main bot.
    static let botsNewThreadRequested = Notification.Name("com.allternit.botsNewThreadRequested")
}

extension Notification {
    /// The non-empty `sessionId` of an `.openChatSession` post, else nil.
    var openChatSessionId: String? {
        guard let id = userInfo?["sessionId"] as? String, !id.isEmpty else { return nil }
        return id
    }
}
