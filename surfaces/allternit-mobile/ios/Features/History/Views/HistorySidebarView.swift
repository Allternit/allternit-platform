import SwiftUI

/// A dated bucket of sessions (Today / Yesterday / Previous 7 Days / Older).
struct HistoryGroup: Identifiable {
    let id: String
    let title: String
    let sessions: [AgentSession]
}

struct HistorySidebarView: View {
    @Binding var selectedSessionId: String?
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @EnvironmentObject private var authManager: AuthManager

    @State private var historyGroups: [HistoryGroup] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil
    /// The settings hub (Phase 4) behind the footer's gear button.
    @State private var isSettingsPresented = false
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Wordmark
            HStack {
                WordmarkView(height: 18)
                Spacer()
            }
            .padding(.horizontal, 20)
            // The VStack now respects the safe area itself (only the
            // background bleeds edge-to-edge, see the .background modifier
            // below), so this just needs normal header spacing below the
            // status bar — not the ~60pt that used to compensate for an
            // ignored top inset.
            .padding(.top, 16)
            .padding(.bottom, 12)

            tabList

            // History Group List
            ScrollView {
                historyContent
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await loadSessions()
            }

            Spacer()

            footer
        }
        .frame(width: 280)
        // Only the background bleeds edge-to-edge (behind the status bar /
        // home indicator) so the open drawer spans the full screen height;
        // the VStack's own layout stays safe-area-respecting so header and
        // footer clear the island and home indicator. The closed drawer is
        // hidden by the content pane's opaque edge-to-edge backdrop
        // (MainWorkspaceView), not by clipping.
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.vertical))
        .sheet(isPresented: $isSettingsPresented) {
            SettingsView()
        }
        .task {
            await loadSessions()
            #if DEBUG
            // `-open-settings` / `-open-settings-memory` / `-open-usage`
            // (DEBUG only): jump straight to the settings sheet (the memory
            // arg deep-links into the Memory section, handled inside
            // SettingsView; `-open-usage` targets the Phase-5 Usage section,
            // near the top) for UI testing/screenshots — no tap injection
            // in simctl.
            if launchArgumentEnabled("open-settings")
                || launchArgumentEnabled("open-settings-memory")
                || launchArgumentEnabled("open-settings-brain-spike")
                || launchArgumentEnabled("open-usage") {
                isSettingsPresented = true
            }
            #endif
        }
        .onChange(of: isSidebarOpen) { _, isOpen in
            // Refresh each time the drawer is revealed so new chats show up.
            if isOpen {
                Task { await loadSessions() }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .historyMutated)) { _ in
            Task { await loadSessions() }
        }
    }

    // MARK: - Sections

    /// Tab list as labeled nav rows (icon + text, full-width tap target):
    /// Chats / Projects / Artifacts Library / Agents / Automation Tasks /
    /// Code / ACI. The active tab is shown by highlight only — no chevrons
    /// or other trailing accessories on these rows.
    private var tabList: some View {
        VStack(spacing: 2) {
            ForEach(ModeBarItem.allCases, id: \.self) { item in
                tabRow(for: item)
            }
        }
        .padding(.horizontal, 8)
        .padding(.bottom, 12)
    }

    private func tabRow(for item: ModeBarItem) -> some View {
        let isActive = modeStore.activeTab == item
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            modeStore.selectBarItem(item)
            withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                isSidebarOpen = false
            }
        }) {
            HStack(spacing: 12) {
                Image(systemName: item.icon)
                    .font(.system(size: 15, weight: .semibold))
                    .frame(width: 20)
                Text(item.label)
                    .font(.subheadline)
                    .fontWeight(isActive ? .semibold : .regular)
                Spacer()
            }
            .foregroundColor(isActive ? modeStore.mode.theme.accent : Color("TextPrimary"))
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            // Accent-tinted active highlight (web --shell-item-active-bg).
            .background(isActive ? modeStore.mode.theme.accentSoft : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(item.label)
        .accessibilityIdentifier("sidebarTab\(item.rawValue)")
    }

    /// Footer: account avatar + "New chat" pill floating over the sidebar
    /// background — no separator bar (Claude-parity layout).
    private var footer: some View {
        HStack {
            Menu {
                Text(authManager.displayName)
                Button(role: .destructive, action: {
                    Task { try? await authManager.signOut() }
                }) {
                    Label("Sign Out", systemImage: "rectangle.portrait.and.arrow.right")
                }
            } label: {
                Text(authManager.avatarInitial)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("BgPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Color("TextSecondary"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Account, \(authManager.displayName)")

            // Settings hub (Phase 4) — gear row next to the avatar.
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isSettingsPresented = true
            }) {
                Image(systemName: "gearshape")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open settings")

            Spacer()

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                selectedSessionId = nil
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen = false
                }
            }) {
                HStack(spacing: 6) {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 13, weight: .semibold))
                    Text("New chat")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(height: 36)
                .background(Color("BgPanel"))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .accessibilityLabel("New chat")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 20)
    }

    // MARK: - Content

    @ViewBuilder
    private var historyContent: some View {
        if isLoading && historyGroups.isEmpty {
            HStack {
                Spacer()
                ProgressView()
                    .padding(.top, 40)
                Spacer()
            }
        } else if let loadError, historyGroups.isEmpty {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load conversations",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await loadSessions() } }
            )
            .padding(.horizontal, 20)
            .padding(.top, 24)
            .frame(maxWidth: .infinity)
        } else if historyGroups.isEmpty {
            FriendlyInlineStateView(
                style: .empty,
                icon: "bubble.left",
                title: "No conversations yet",
                message: "Start a new chat to see it here."
            )
            .padding(.horizontal, 20)
            .padding(.top, 24)
            .frame(maxWidth: .infinity)
        } else if visibleGroups.isEmpty, !showActiveBot {
            // History is loaded but the mode filter leaves nothing for this
            // surface — distinct from the "no chats yet" empty state.
            FriendlyInlineStateView(
                style: .empty,
                icon: "bubble.left.and.bubble.right",
                title: "No conversations here yet",
                message: "Switch modes or start a new chat to see it here."
            )
            .padding(.horizontal, 20)
            .padding(.top, 24)
            .frame(maxWidth: .infinity)
        } else {
            VStack(alignment: .leading, spacing: 28) {
                activeBotSection

                ForEach(visibleGroups) { group in
                    VStack(alignment: .leading, spacing: 10) {
                        Text(group.title)
                            .font(.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 20)

                        ForEach(group.sessions) { session in
                            Button(action: {
                                let generator = UIImpactFeedbackGenerator(style: .light)
                                generator.impactOccurred()
                                selectedSessionId = session.id
                                isSidebarOpen = false
                            }) {
                                HStack(spacing: 12) {
                                    Image(systemName: "bubble.left")
                                        .font(.subheadline)
                                        .foregroundColor(selectedSessionId == session.id ? Color("AccentPrimary") : Color("TextSecondary"))

                                    Text(session.name ?? "Untitled")
                                        .font(.subheadline)
                                        .foregroundColor(selectedSessionId == session.id ? Color("TextPrimary") : Color("TextPrimary").opacity(0.8))
                                        .lineLimit(1)

                                    Spacer()
                                }
                                .padding(.vertical, 8)
                                .padding(.horizontal, 20)
                                .background(selectedSessionId == session.id ? Color("BgTertiary").opacity(0.5) : Color.clear)
                                .cornerRadius(8)
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: - Active bot

    /// True when agent/bot mode is on for the current surface and a registry
    /// agent is selected. The rail then surfaces that bot as the mounted
    /// session instead of hiding it in recents.
    private var showActiveBot: Bool {
        agentModeStore.isAgentEnabled(for: modeStore.mode)
            && agentModeStore.selectedAgent(for: modeStore.mode) != nil
    }

    /// The mounted bot row: avatar + name, highlighted while bot mode is on.
    /// Tapping it mounts a fresh bot chat in the main view.
    @ViewBuilder
    private var activeBotSection: some View {
        if showActiveBot, let agent = agentModeStore.selectedAgent(for: modeStore.mode) {
            VStack(alignment: .leading, spacing: 10) {
                Text("ACTIVE BOT")
                    .font(.system(size: 11, weight: .bold))
                    .tracking(1)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 20)

                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    selectedSessionId = nil
                    modeStore.selectBarItem(.chats)
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                        isSidebarOpen = false
                    }
                }) {
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
                    .padding(.vertical, 8)
                    .padding(.horizontal, 20)
                    .background(Color("BgTertiary").opacity(0.5))
                    .cornerRadius(8)
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: - Filtering

    /// History filtered by active mode (ShellRail mode isolation). Bot/agent
    /// sessions (those with an `agent_id`) live in the agent | bot hub, not in
    /// Home recents. Code sessions live in Code mode. Incognito (ephemeral)
    /// sessions are excluded server-side.
    private var visibleGroups: [HistoryGroup] {
        let activeMode = modeStore.mode
        return historyGroups.compactMap { group -> HistoryGroup? in
            let modeSessions = group.sessions.filter { session in
                guard session.active else { return false }
                let surface = session.originSurface ?? ""
                // Bot sessions belong in the agent | bot hub regardless of origin surface.
                guard session.agentId == nil else { return false }
                switch activeMode {
                case .chat, .cowork:
                    return surface == "chat" || surface == "cowork" || surface.isEmpty
                case .code:
                    return surface == "code"
                case .browser:
                    return surface == "browser"
                }
            }
            return modeSessions.isEmpty ? nil : HistoryGroup(id: group.id, title: group.title, sessions: modeSessions)
        }
    }

    // MARK: - Loading & grouping

    @MainActor
    private func loadSessions() async {
        if historyGroups.isEmpty {
            isLoading = true
        }
        loadError = nil

        do {
            // GET /api/v1/agent-sessions → { sessions: [...], count } — the
            // production sidebar's history source.
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            historyGroups = Self.group(envelope.sessions)
        } catch is CancellationError {
            // View disappeared mid-flight — keep the current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    /// Buckets records by `updated_at` (falling back to `created_at`), newest
    /// first, into Today / Yesterday / Previous 7 Days / Older. Empty groups
    /// are dropped.
    private static func group(_ records: [AgentSession]) -> [HistoryGroup] {
        let sorted = records.sorted { timestamp(of: $0) > timestamp(of: $1) }
        let calendar = Calendar.current
        let weekAgo = Date().addingTimeInterval(-7 * 24 * 60 * 60)

        var today: [AgentSession] = []
        var yesterday: [AgentSession] = []
        var previousWeek: [AgentSession] = []
        var older: [AgentSession] = []

        for record in sorted {
            let date = timestamp(of: record)
            if calendar.isDateInToday(date) {
                today.append(record)
            } else if calendar.isDateInYesterday(date) {
                yesterday.append(record)
            } else if date > weekAgo {
                previousWeek.append(record)
            } else {
                older.append(record)
            }
        }

        return [
            HistoryGroup(id: "today", title: "Today", sessions: today),
            HistoryGroup(id: "yesterday", title: "Yesterday", sessions: yesterday),
            HistoryGroup(id: "previousWeek", title: "Previous 7 Days", sessions: previousWeek),
            HistoryGroup(id: "older", title: "Older", sessions: older)
        ].filter { !$0.sessions.isEmpty }
    }

    private static func timestamp(of record: AgentSession) -> Date {
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
}

extension Notification {
    /// The non-empty `sessionId` of an `.openChatSession` post, else nil.
    var openChatSessionId: String? {
        guard let id = userInfo?["sessionId"] as? String, !id.isEmpty else { return nil }
        return id
    }
}
