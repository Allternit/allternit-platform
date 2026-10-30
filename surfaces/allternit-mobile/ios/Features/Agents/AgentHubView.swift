import SwiftUI

/// agent | bot hub landing surface: the sidebar's Agents tab, now a Bot Home
/// parity view. It lists created agents/bots, surfaces their sessions and
/// workspace, and exposes runtime/configuration actions. The composer pill
/// reads the same registry through AgentModeStore's cache; after every hub
/// mutation we force-refresh it so the two never disagree.
struct AgentHubView: View {
    @Binding var isSidebarOpen: Bool

    @StateObject private var hubStore = AgentHubStore.shared
    @StateObject private var botStatusStore = BotStatusStore.shared
    @EnvironmentObject private var agentModeStore: AgentModeStore

    @State private var searchText = ""
    /// Category chip selection for the bot grid (nil = All), mirroring
    /// BotHubHomeTab.tsx's `categoryFilter`.
    @State private var botCategoryFilter: BotCategory? = nil
    @State private var selectedTab: HubTab = .home
    /// Pushed detail (nil = list). Item-driven so the template flow can land
    /// on the new agent's detail without tap injection.
    @State private var detailAgent: AgentRecord? = nil
    @State private var isTemplateSheetPresented = false
    @State private var isBotSelectionPresented = false
    @State private var agentPendingDeletion: AgentRecord? = nil
    @State private var isDeleteConfirmPresented = false
    @State private var actionError: String? = nil

    // Sessions state (bot sessions live here, not in Home recents).
    @State private var botSessions: [AgentSession] = []
    @State private var isLoadingSessions = false
    @State private var sessionsError: String? = nil

    #if DEBUG
    /// One-shot latch for the `-open-agent-detail` launch arg.
    @State private var didApplyDebugArgs = false
    #endif

    private var visibleAgents: [AgentRecord] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return hubStore.agents }
        return hubStore.agents.filter {
            $0.name.localizedCaseInsensitiveContains(query)
        }
    }

    /// Standalone agents and crew orchestrators (mode "primary" /
    /// "orchestrator" / "council") — the top-level picks. Packaged bots are
    /// excluded: they surface in the grid above the list instead.
    private var primaryAgents: [AgentRecord] {
        visibleAgents.filter { $0.mode != "subagent" && !$0.isBot }
    }

    /// Subagents, shown as their own section with their orchestrator's
    /// name — a flat list hides the crew structure the registry carries.
    private var crewAgents: [AgentRecord] {
        visibleAgents.filter { $0.mode == "subagent" && !$0.isBot }
    }

    /// Packaged bots in the registry (`getBots()` in lib/bots/bot-profile.ts).
    private var bots: [AgentRecord] {
        hubStore.agents.filter(\.isBot)
    }

    /// Bots after the home-tab search + category filter, mirroring
    /// BotHubHomeTab.tsx's `filteredBots`: the query hits display name,
    /// `@` handle, tagline, and description.
    private var filteredBots: [AgentRecord] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return bots.filter { bot in
            if let filter = botCategoryFilter, bot.botProfile?.botCategory != filter { return false }
            guard !query.isEmpty else { return true }
            return bot.botDisplayName.localizedCaseInsensitiveContains(query)
                || bot.name.localizedCaseInsensitiveContains(query)
                || (bot.botProfile?.tagline ?? "").localizedCaseInsensitiveContains(query)
                || (bot.description ?? "").localizedCaseInsensitiveContains(query)
        }
    }

    /// Per-bot session counts from the loaded bot sessions — the
    /// counterpart of BotHubHomeTab.tsx's `sessionCountByBotId`.
    private var sessionCountByBotId: [String: Int] {
        var counts: [String: Int] = [:]
        for session in botSessions {
            guard let agentId = session.agentId else { continue }
            counts[agentId, default: 0] += 1
        }
        return counts
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                headerBar
                Divider().background(Color("BorderSubtle"))
                hubTabs
                Divider().background(Color("BorderSubtle"))
                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailAgent) { agent in
                AgentDetailView(initialAgent: agent)
            }
        }
        .sheet(isPresented: $isTemplateSheetPresented) {
            AgentTemplateSheet(onConfirm: createFromTemplate)
        }
        .sheet(isPresented: $isBotSelectionPresented) {
            BotSelectionSheet()
        }
        .confirmationDialog(
            "Delete \(agentPendingDeletion?.name ?? "this agent")?",
            isPresented: $isDeleteConfirmPresented,
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let agent = agentPendingDeletion {
                    deleteAgent(agent)
                }
            }
            Button("Cancel", role: .cancel) { agentPendingDeletion = nil }
        } message: {
            Text("This permanently deletes the agent. This can't be undone.")
        }
        .task {
            hubStore.fetchAgentsIfNeeded()
            hubStore.fetchTemplatesIfNeeded()
            await loadBotSessions()
        }
        .onChange(of: selectedTab) { _, newTab in
            if newTab == .sessions {
                Task { await loadBotSessions() }
            }
        }
        .onChange(of: hubStore.agents) { _, agents in
            #if DEBUG
            // `-open-agent-detail` (DEBUG only): drill straight into an
            // agent's detail for screenshots once the registry resolves
            // (simctl has no tap injection). Accepts a full id or a unique
            // prefix, e.g. the first 8 hex chars.
            guard !didApplyDebugArgs,
                  let raw = UserDefaults.standard.string(forKey: "open-agent-detail"),
                  let agent = agents.first(where: { $0.id == raw || $0.id.hasPrefix(raw) })
            else { return }
            didApplyDebugArgs = true
            detailAgent = agent
            #endif
        }
    }

    // MARK: - Header

    private var headerBar: some View {
        HStack {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .medium)
                generator.impactOccurred()
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen.toggle()
                }
            }) {
                Image(systemName: "line.3.horizontal")
                    .font(.title3)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Open sidebar")

            Text("agent | bot hub")
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))

            Spacer()

            NavigationLink(destination: MarketplaceView()) {
                Image(systemName: "storefront")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Discover agents and bots")

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isTemplateSheetPresented = true
            }) {
                Image(systemName: "plus")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Create new agent")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
        .background(Color("BgPrimary"))
    }

    // MARK: - Tabs

    private var hubTabs: some View {
        HStack(spacing: 0) {
            tabButton(.home)
            tabButton(.sessions)
            tabButton(.workspace)
            tabButton(.config)
        }
        .padding(.horizontal, 4)
        .background(Color("BgPrimary"))
    }

    private func tabButton(_ tab: HubTab) -> some View {
        let isSelected = selectedTab == tab
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            selectedTab = tab
        }) {
            Text(tab.label)
                .font(.system(size: 13, weight: isSelected ? .semibold : .medium))
                .foregroundColor(isSelected ? Color("TextPrimary") : Color("TextSecondary"))
                .frame(maxWidth: .infinity, minHeight: 36)
                .overlay(
                    Rectangle()
                        .fill(isSelected ? Color("AccentPrimary") : Color.clear)
                        .frame(height: 2)
                        .padding(.horizontal, 8),
                    alignment: .bottom
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(tab.label)
        .accessibilityIdentifier("hubTab\(tab.rawValue)")
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        Group {
            switch selectedTab {
            case .home:
                homeContent
            case .sessions:
                sessionsContent
            case .workspace:
                workspaceContent
            case .config:
                configContent
            }
        }
        .id("hubContent_\(selectedTab.rawValue)")
    }

    // MARK: - Home tab

    private var homeContent: some View {
        ScrollView {
            VStack(spacing: 16) {
                if hubStore.isLoading && hubStore.agents.isEmpty {
                    Spacer()
                    ProgressView()
                        .padding(.top, 40)
                    Spacer()
                } else if let loadError = hubStore.loadError, hubStore.agents.isEmpty {
                    Spacer()
                    FriendlyStateView(
                        style: .offline,
                        icon: "wifi.slash",
                        title: "Couldn't load agents",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: {
                            hubStore.fetchAgentsIfNeeded(force: true)
                            hubStore.fetchTemplatesIfNeeded(force: true)
                            Task { await loadBotSessions() }
                        }
                    )
                    Spacer()
                } else {
                    heroSection
                    if let actionError {
                        Text(actionError)
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                            .padding(.horizontal, 20)
                    }
                    if hubStore.agents.isEmpty {
                        emptyState
                    } else {
                        botsListSection
                    }
                }
            }
            .padding(.vertical, 12)
        }
        .scrollDismissesKeyboard(.interactively)
        .refreshable {
            hubStore.fetchAgentsIfNeeded(force: true)
            hubStore.fetchTemplatesIfNeeded(force: true)
            agentModeStore.fetchAgentsIfNeeded(force: true)
            await loadBotSessions()
        }
    }

    private var heroSection: some View {
        VStack(spacing: 12) {
            HStack(spacing: 12) {
                botTogglePill
                Spacer()
            }

            modeSelector

            HStack(spacing: 12) {
                statCard(
                    icon: "cpu",
                    label: "Agents | Bots",
                    value: "\(hubStore.agents.count)",
                    subtitle: "Created"
                )
                statCard(
                    icon: "bubble.left",
                    label: "Sessions",
                    value: "\(botSessions.count)",
                    subtitle: "Bot runs"
                )
            }
            .padding(.horizontal, 20)
        }
        .padding(.vertical, 8)
    }

    /// Bot on/off pill. Turning it on opens a populated bot selection modal
    /// and binds the chosen bot as the active agent for chat surfaces.
    private var botTogglePill: some View {
        let isOn = agentModeStore.isAgentEnabled(for: .chat)
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            if !isOn {
                agentModeStore.setAgentEnabled(true, for: .chat)
                agentModeStore.fetchAgentsIfNeeded()
                isBotSelectionPresented = true
            } else {
                agentModeStore.setAgentEnabled(false, for: .chat)
            }
        }) {
            HStack(spacing: 6) {
                Image(systemName: "cpu")
                    .font(.system(size: 11, weight: .semibold))
                Text(isOn ? "Bot on" : "Bot off")
                    .font(.system(size: 12, weight: .semibold))
                if isOn, let name = agentModeStore.selectedAgent(for: .chat)?.name {
                    Text("· \(name)")
                        .font(.system(size: 12, weight: .medium))
                        .lineLimit(1)
                }
            }
            .foregroundColor(isOn ? Color("AccentPrimary") : Color("TextSecondary"))
            .padding(.horizontal, 12)
            .frame(height: 32)
            .background(isOn ? Color("AccentPrimary").opacity(0.12) : Color("BgPanel"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(isOn ? Color("AccentPrimary").opacity(0.35) : Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 20)
    }

    /// Mode selector with pipe separators, supporting multiple runtime modes
    /// (agent/bot swarm, deep research, etc.). The selected mode is persisted
    /// per surface through AgentModeStore's tile selection.
    private var modeSelector: some View {
        let modes = AgentModeTile.visibleTiles(for: .chat)
        let selected = agentModeStore.selectedTile(for: .chat)
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array(modes.enumerated()), id: \.offset) { index, mode in
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        agentModeStore.selectTile(mode, for: .chat)
                    }) {
                        HStack(spacing: 4) {
                            if let deckIcon = mode.deckIcon {
                                Image(deckIcon)
                                    .resizable()
                                    .frame(width: 14, height: 14)
                            } else {
                                Image(systemName: mode.icon)
                                    .font(.system(size: 10, weight: .semibold))
                            }
                            Text(mode.label)
                                .font(.system(size: 12, weight: selected == mode ? .semibold : .medium))
                        }
                        .foregroundColor(selected == mode ? Color("AccentPrimary") : Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .background(selected == mode ? Color("AccentPrimary").opacity(0.12) : Color.clear)
                        .clipShape(Capsule())
                    }
                    .buttonStyle(.plain)

                    if index < modes.count - 1 {
                        Text("|")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("BorderSubtle"))
                            .padding(.horizontal, 6)
                    }
                }
            }
            .padding(.horizontal, 20)
        }
    }

    private var botsListSection: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search agents | bots", text: $searchText)
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

            if !bots.isEmpty {
                botGridSection
            }

            if !primaryAgents.isEmpty || !crewAgents.isEmpty {
                LazyVStack(spacing: 10) {
                    if !primaryAgents.isEmpty {
                        Section {
                            ForEach(primaryAgents) { agent in
                                agentRow(agent)
                                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                        Button(role: .destructive) {
                                            agentPendingDeletion = agent
                                            isDeleteConfirmPresented = true
                                        } label: {
                                            Label("Delete", systemImage: "trash")
                                        }
                                    }
                            }
                        } header: {
                            sectionLabel(bots.isEmpty ? "Your agents | bots" : "Your agents")
                                .padding(.horizontal, 20)
                                .padding(.bottom, 4)
                        }
                    }

                    if !crewAgents.isEmpty {
                        Section {
                            ForEach(crewAgents) { agent in
                                agentRow(agent)
                                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                        Button(role: .destructive) {
                                            agentPendingDeletion = agent
                                            isDeleteConfirmPresented = true
                                        } label: {
                                            Label("Delete", systemImage: "trash")
                                        }
                                    }
                            }
                        } header: {
                            sectionLabel("Crew")
                                .padding(.horizontal, 20)
                                .padding(.bottom, 4)
                        }
                    }

                    Section {
                        templatesRow
                    } header: {
                        sectionLabel("Templates")
                            .padding(.horizontal, 20)
                            .padding(.bottom, 4)
                    }
                }
                .padding(.horizontal, 20)
            } else if !bots.isEmpty {
                // Registry holds only bots — keep template creation reachable.
                LazyVStack(spacing: 10) {
                    Section {
                        templatesRow
                    } header: {
                        sectionLabel("Templates")
                            .padding(.horizontal, 20)
                            .padding(.bottom, 4)
                    }
                }
                .padding(.horizontal, 20)
            } else if visibleAgents.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "magnifyingglass",
                    title: "No matches",
                    message: "No agents or bots match your search."
                )
                .padding(.horizontal, 20)
            }
        }
    }

    // MARK: - Bot grid

    /// Bot discovery section, mirroring views/agent-hub/main/BotHubHomeTab.tsx:
    /// a heading, category filter chips, and a two-column LazyVGrid of cards.
    private var botGridSection: some View {
        VStack(spacing: 0) {
            sectionLabel("Bots")
                .padding(.horizontal, 20)
                .padding(.bottom, 8)

            botCategoryChips
                .padding(.bottom, 12)

            if filteredBots.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "magnifyingglass",
                    title: "No matches",
                    message: "No bots match your search."
                )
                .padding(.horizontal, 20)
            } else {
                LazyVGrid(
                    columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)],
                    spacing: 10
                ) {
                    ForEach(filteredBots) { bot in
                        botCard(bot)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 16)
            }
        }
    }

    /// "All" + one chip per BOT_CATEGORIES entry (bot-profile.ts:202-235),
    /// styled like the bot toggle pill.
    private var botCategoryChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                botCategoryChip(label: "All", isActive: botCategoryFilter == nil) {
                    botCategoryFilter = nil
                }
                ForEach(BOT_CATEGORIES) { category in
                    botCategoryChip(label: category.label, isActive: botCategoryFilter == category) {
                        botCategoryFilter = category
                    }
                }
            }
            .padding(.horizontal, 20)
        }
    }

    private func botCategoryChip(label: String, isActive: Bool, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            Text(label)
                .font(.system(size: 12, weight: isActive ? .semibold : .medium))
                .foregroundColor(isActive ? Color("AccentPrimary") : Color("TextSecondary"))
                .padding(.horizontal, 12)
                .frame(height: 30)
                .background(isActive ? Color("AccentPrimary").opacity(0.12) : Color("BgPanel"))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(isActive ? Color("AccentPrimary").opacity(0.35) : Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    /// One grid card, mirroring BotHubCard.tsx: avatar + display name +
    /// tagline, category chip and session count, and the bot's accent color
    /// as a low-opacity bar along the bottom (falling back to the platform
    /// accent, as the web card does). Taps open the same agent detail as
    /// the list rows.
    private func botCard(_ bot: AgentRecord) -> some View {
        let accent = bot.botAccentColor.map { Color(hex: $0) } ?? Color("AccentPrimary")
        let sessionCount = sessionCountByBotId[bot.id] ?? 0
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailAgent = bot
        }) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .top, spacing: 10) {
                    AgentAvatarView(agent: bot, size: 44)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(bot.botDisplayName)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Text(bot.botTagline ?? "No description")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 0)
                }

                HStack(spacing: 6) {
                    botStatusPill(bot)
                    if let category = bot.botProfile?.botCategory {
                        Text(category.label)
                            .font(.system(size: 9.5, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2.5)
                            .background(Color("BgSecondary"))
                            .clipShape(Capsule())
                            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                    Text(sessionCount == 0 ? "No sessions" : "\(sessionCount) session\(sessionCount == 1 ? "" : "s")")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundColor(Color("TextSecondary").opacity(0.85))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }

                Capsule()
                    .fill(accent.opacity(0.4))
                    .frame(height: 4)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.045), radius: 5, x: 0, y: 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onAppear {
            // Live status for visible bots only (the projection store shares
            // one SSE subscription per bot, so re-appearing cards are free).
            botStatusStore.subscribe(botId: bot.id)
        }
    }

    /// Live operational-status pill for bot cards, driven by BotStatusStore's
    /// SSE-folded projection (web: `useBotStatus`, bot-operational-state
    /// .store.ts:405-413). Replaces the hub's old two-state registry dot for
    /// bots — idle/working/attention states get distinct colors from
    /// `BotOperationalStatus.color`.
    private func botStatusPill(_ bot: AgentRecord) -> some View {
        let status = botStatusStore.status(for: bot.id)
        return HStack(spacing: 4) {
            Circle()
                .fill(status.color)
                .frame(width: 7, height: 7)
            Text(status.label)
                .font(.system(size: 9.5, weight: .semibold))
                .foregroundColor(status.color)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 2.5)
        .background(status.color.opacity(0.12))
        .clipShape(Capsule())
        .overlay(Capsule().stroke(status.color.opacity(0.35), lineWidth: 1))
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "cpu",
            title: "No agents yet",
            message: "Agents and bots run tasks on your behalf — create one from a template to get started.",
            actionTitle: "New from template",
            action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isTemplateSheetPresented = true
            }
        )
    }

    // MARK: - Sessions tab

    private var sessionsContent: some View {
        ScrollView {
            VStack(spacing: 0) {
                if isLoadingSessions && botSessions.isEmpty {
                    ProgressView()
                        .padding(.top, 40)
                } else if let sessionsError, botSessions.isEmpty {
                    FriendlyStateView(
                        style: .offline,
                        icon: "wifi.slash",
                        title: "Couldn't load bot sessions",
                        message: FriendlyErrorMessage.from(sessionsError),
                        actionTitle: "Retry",
                        action: { Task { await loadBotSessions() } }
                    )
                } else if botSessions.isEmpty {
                    FriendlyStateView(
                        style: .empty,
                        icon: "bubble.left",
                        title: "No bot sessions yet",
                        message: "Start a bot to see its runs here."
                    )
                } else {
                    LazyVStack(spacing: 10) {
                        Section {
                            ForEach(botSessions) { session in
                                sessionRow(session)
                            }
                        } header: {
                            sectionLabel("Bot sessions")
                                .padding(.horizontal, 20)
                                .padding(.bottom, 4)
                        }
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 12)
                }
            }
        }
        .refreshable {
            await loadBotSessions()
        }
    }

    private func sessionRow(_ session: AgentSession) -> some View {
        HStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Color("AccentPrimary").opacity(0.12))
                Image(systemName: "bubble.left")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
            }
            .frame(width: 40, height: 40)

            VStack(alignment: .leading, spacing: 2) {
                Text(session.name ?? "Untitled Session")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text("\(session.messageCount) message\(session.messageCount == 1 ? "" : "s") · \(session.updatedAt.prefix(10))")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer(minLength: 0)
        }
        .padding(12)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Workspace tab

    private var workspaceContent: some View {
        ScrollView {
            workspacePrompt
                .padding(.vertical, 12)
        }
    }

    private var workspacePrompt: some View {
        VStack(spacing: 12) {
            if hubStore.isLoading && hubStore.agents.isEmpty {
                ProgressView()
                    .padding(.top, 40)
            } else if let error = hubStore.loadError, hubStore.agents.isEmpty {
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load workspace agents",
                    message: FriendlyErrorMessage.from(error),
                    actionTitle: "Retry",
                    action: { hubStore.fetchAgentsIfNeeded(force: true) }
                )
            } else if hubStore.agents.isEmpty {
                FriendlyStateView(
                    style: .empty,
                    icon: "folder.badge.person.crop",
                    title: "No agents to inspect",
                    message: "Create an agent or bot first, then select it here to browse its workspace files.",
                    actionTitle: "New from template",
                    action: { isTemplateSheetPresented = true }
                )
            } else {
                Text("Select an agent or bot to inspect its workspace")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 40)
                    .padding(.top, 40)

                LazyVStack(spacing: 10) {
                    ForEach(hubStore.agents) { agent in
                        Button(action: {
                            let generator = UIImpactFeedbackGenerator(style: .light)
                            generator.impactOccurred()
                            detailAgent = agent
                        }) {
                            HStack(spacing: 12) {
                                AgentAvatarView(agent: agent, size: 40)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(agent.name)
                                        .font(.system(size: 14, weight: .semibold))
                                        .foregroundColor(Color("TextPrimary"))
                                        .lineLimit(1)
                                    Text(agent.description ?? "No description")
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                        .lineLimit(1)
                                }
                                Spacer(minLength: 0)
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .padding(12)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
                            )
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 20)
            }
        }
    }

    // MARK: - Config tab

    private var configContent: some View {
        ScrollView {
            VStack(spacing: 12) {
                configCard(
                    icon: "sparkles",
                    title: "New from template",
                    subtitle: "Research, review, write, and more"
                ) {
                    isTemplateSheetPresented = true
                }

                configCard(
                    icon: "storefront",
                    title: "Discover agents | bots",
                    subtitle: "Browse the marketplace"
                ) {
                    // Marketplace is a navigation destination; the header link
                    // handles it. Here we just open the detail if needed.
                }

                configCard(
                    icon: "gearshape.2",
                    title: "Response style",
                    subtitle: PreferencesStore.shared.responseStyle.label
                ) {
                    // No-op: a future settings deep-link can go here.
                }

                if let selected = agentModeStore.selectedAgent(for: .chat) {
                    configCard(
                        icon: "cpu",
                        title: "Active bot",
                        subtitle: selected.name
                    ) {
                        detailAgent = selected
                    }
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
    }

    // MARK: - Rows

    private func agentRow(_ agent: AgentRecord) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailAgent = agent
        }) {
            HStack(alignment: .top, spacing: 11) {
                AgentAvatarView(agent: agent, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 7) {
                        Text(agent.name)
                            .font(.system(size: 14.5, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if agent.isBot {
                            botStatusPill(agent)
                                .onAppear { botStatusStore.subscribe(botId: agent.id) }
                        } else {
                            statusDot(agent)
                        }
                        if agent.isPrimary {
                            Text("PRIMARY")
                                .font(.system(size: 9.5, weight: .bold))
                                .tracking(0.6)
                                .foregroundColor(Color("AccentPrimary"))
                                .padding(.horizontal, 7)
                                .padding(.vertical, 2.5)
                                .background(Color("AccentPrimary").opacity(0.14))
                                .clipShape(Capsule())
                        }
                    }
                    if let description = agent.description, !description.isEmpty {
                        Text(description)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        if !agent.model.isEmpty {
                            Text(agent.model)
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundColor(Color("TextSecondary"))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 2.5)
                                .background(Color("BgSecondary"))
                                .clipShape(Capsule())
                                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                        }
                        if let parentName = crewParentName(for: agent) {
                            Text("crew of \(parentName)")
                                .font(.system(size: 10, weight: .medium))
                                .foregroundColor(Color("TextSecondary").opacity(0.85))
                                .lineLimit(1)
                        }
                    }
                    .padding(.top, 5)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 4)
            }
            .padding(12)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.045), radius: 5, x: 0, y: 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func crewParentName(for agent: AgentRecord) -> String? {
        guard agent.mode == "subagent", let parentId = agent.parentAgentId else { return nil }
        return hubStore.agent(withId: parentId)?.name
    }

    private var templatesRow: some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            isTemplateSheetPresented = true
        }) {
            HStack(spacing: 11) {
                Image(systemName: "sparkles")
                    .font(.system(size: 17, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Color("AccentPrimary").opacity(0.12))
                    .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text("New from template…")
                        .font(.system(size: 14.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(hubStore.templates.isEmpty
                         ? "Research, review, write, and more"
                         : hubStore.templates.map(\.name).joined(separator: " · "))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(12)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func statusDot(_ agent: AgentRecord) -> some View {
        let isActive = ["active", "running", "busy"].contains(agent.status.lowercased())
        return HStack(spacing: 4) {
            Circle()
                .fill(isActive ? Theme.statusSuccess : Color("TextSecondary").opacity(0.6))
                .frame(width: 7, height: 7)
            if isActive {
                Text("Active")
                    .font(.system(size: 9.5, weight: .semibold))
                    .foregroundColor(Theme.statusSuccess)
            }
        }
    }

    private func sectionLabel(_ title: String) -> some View {
        HStack {
            Text(title.uppercased())
                .font(.system(size: 11, weight: .bold))
                .tracking(1)
                .foregroundColor(Color("TextSecondary"))
                .textCase(nil)
            Spacer()
        }
    }

    private func statCard(icon: String, label: String, value: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("AccentPrimary"))
                Text(label)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
            Text(value)
                .font(.system(.title2, design: .serif))
                .fontWeight(.semibold)
                .foregroundColor(Color("TextPrimary"))
            Text(subtitle)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func configCard(icon: String, title: String, subtitle: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Color("AccentPrimary").opacity(0.12))
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 14.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(subtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(12)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Actions

    private func deleteAgent(_ agent: AgentRecord) {
        agentPendingDeletion = nil
        Task {
            do {
                try await hubStore.deleteAgent(id: agent.id)
                agentModeStore.fetchAgentsIfNeeded(force: true)
            } catch {
                actionError = "Couldn't delete the agent: \(error.localizedDescription)"
            }
        }
    }

    private func createFromTemplate(_ template: AgentTemplate) {
        Task {
            do {
                let agent = try await hubStore.createFromTemplate(templateId: template.id)
                agentModeStore.fetchAgentsIfNeeded(force: true)
                detailAgent = agent
            } catch {
                actionError = "Couldn't create the agent: \(error.localizedDescription)"
            }
        }
    }

    @MainActor
    private func loadBotSessions() async {
        isLoadingSessions = true
        sessionsError = nil
        defer { isLoadingSessions = false }
        do {
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            botSessions = envelope.sessions.filter { $0.agentId != nil }
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            sessionsError = error.localizedDescription
        }
    }

}

// MARK: - Hub tabs

private enum HubTab: String, CaseIterable, Identifiable {
    case home, sessions, workspace, config

    var id: String { rawValue }

    var label: String {
        switch self {
        case .home: return "Home"
        case .sessions: return "Sessions"
        case .workspace: return "Workspace"
        case .config: return "Config"
        }
    }
}

/// Extracted tab button to avoid closure-capture ambiguity inside `ForEach`;
/// each button has a stable identity and an explicit tab value.
private struct HubTabButton: View {
    let tab: HubTab
    let isSelected: Bool
    let select: (HubTab) -> Void

    var body: some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            select(tab)
        }) {
            Text(tab.label)
                .font(.system(size: 13, weight: isSelected ? .semibold : .medium))
                .foregroundColor(isSelected ? Color("TextPrimary") : Color("TextSecondary"))
                .frame(maxWidth: .infinity, minHeight: 36)
                .overlay(
                    Rectangle()
                        .fill(isSelected ? Color("AccentPrimary") : Color.clear)
                        .frame(height: 2)
                        .padding(.horizontal, 8),
                    alignment: .bottom
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .id("hubTabButton_\(tab.rawValue)")
        .accessibilityLabel(tab.label)
        .accessibilityIdentifier("hubTab\(tab.rawValue)")
    }
}

// MARK: - Bot selection sheet

/// Populated bot/agent selection modal shown when the bot toggle is turned on.
/// Mirrors the composer AgentSelectionSheet but lives in the hub so the choice
/// is visually tied to the bot on/off pill.
struct BotSelectionSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var agentModeStore: AgentModeStore

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(spacing: 8) {
                    HStack {
                        Text("CHOOSE AGENT | BOT")
                            .font(.system(size: 11, weight: .bold))
                            .tracking(1)
                            .foregroundColor(Color("TextSecondary"))
                        Spacer()
                    }
                    .padding(.horizontal, 4)
                    .padding(.bottom, 4)

                    botCard(agent: nil)
                    ForEach(agentModeStore.agentsForSurface(.chat)) { agent in
                        botCard(agent: agent)
                    }

                    if agentModeStore.isLoadingAgents, agentModeStore.agents.isEmpty {
                        HStack {
                            Spacer()
                            ProgressView()
                            Spacer()
                        }
                        .padding(.vertical, 16)
                    } else if let error = agentModeStore.agentsError {
                        FriendlyInlineStateView(
                            style: .offline,
                            icon: "wifi.slash",
                            title: "Couldn't load agents",
                            message: FriendlyErrorMessage.from(error),
                            actionTitle: "Retry",
                            action: { agentModeStore.fetchAgentsIfNeeded(force: true) }
                        )
                        .padding(.vertical, 8)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
            }
            .background(Color("BgSecondary"))
            .navigationTitle("Select Bot")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear { agentModeStore.fetchAgentsIfNeeded() }
    }

    @ViewBuilder
    private func botCard(agent: AgentRecord?) -> some View {
        let isSelected = agentModeStore.selectedAgentId(for: .chat) == agent?.id
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            agentModeStore.selectAgent(agent, for: .chat)
            dismiss()
        }) {
            HStack(spacing: 12) {
                if let agent {
                    AgentAvatarView(agent: agent, size: 40)
                } else {
                    ZStack {
                        RoundedRectangle(cornerRadius: 40 * 0.325, style: .continuous)
                            .fill(Color("AccentPrimary").opacity(0.14))
                        Image(systemName: "sparkles")
                            .font(.system(size: 40 * 0.42, weight: .medium))
                            .foregroundColor(Color("AccentPrimary"))
                    }
                    .frame(width: 40, height: 40)
                }

                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(agent?.name ?? "Default agent/bot")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if agent?.isPrimary == true {
                            Text("PRIMARY")
                                .font(.system(size: 8, weight: .bold))
                                .tracking(0.6)
                                .foregroundColor(Color("AccentPrimary"))
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Color("AccentPrimary").opacity(0.14))
                                .clipShape(Capsule())
                        }
                    }
                    Text(agent?.description ?? "The platform's built-in behavior")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }

                Spacer(minLength: 8)

                if isSelected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 17))
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(isSelected ? Color("AccentPrimary").opacity(0.05) : Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(isSelected ? Color("AccentPrimary") : Theme.borderWarmDefault,
                            lineWidth: isSelected ? 1.5 : 1)
            )
            .shadow(color: .black.opacity(0.045), radius: 5, x: 0, y: 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Avatar

/// Agent avatar — the agent's ID card, three looks:
/// 1. `avatar` set to "grid:<seed>" → a shuffled identicon variant (iOS
///    convention from the avatar editor; the web hub renders the raw
///    string until it learns it).
/// 2. `avatar` set to anything else → the glyph itself (custom emoji /
///    character the user typed) on the type-tinted tile.
/// 3. unset → the deterministic identicon from the agent id — every agent
///    has a distinctive mark out of the box, no generic icons.
struct AgentAvatarView: View {
    let agent: AgentRecord
    var size: CGFloat = 40

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.325, style: .continuous)
                .fill(tileBackground)
            if let avatar = agent.avatar, !avatar.isEmpty {
                if let seed = Self.gridSeed(avatar) {
                    AgentIdenticonView(seed: seed, size: size * 0.62)
                } else {
                    Text(avatar)
                        .font(.system(size: size * 0.475))
                }
            } else {
                AgentIdenticonView(seed: agent.id, size: size * 0.62)
            }
        }
        .frame(width: size, height: size)
    }

    /// "grid:<seed>" → the seed, nil otherwise.
    static func gridSeed(_ avatar: String) -> String? {
        avatar.hasPrefix("grid:") ? String(avatar.dropFirst(5)) : nil
    }

    private var tileBackground: Color {
        if let avatar = agent.avatar, !avatar.isEmpty, Self.gridSeed(avatar) == nil {
            // Custom glyph keeps the type-tinted tile.
            return Self.tint(for: agent).opacity(0.14)
        }
        // Identicon paper — the warm cream of the logo's tile bed.
        return Color(red: 0.93, green: 0.90, blue: 0.84)
    }

    /// Per-kind tile tint behind custom glyphs (mockup's avatar color
    /// coding); the accent stays the default so unknown kinds read as
    /// platform-native.
    static func tint(for agent: AgentRecord) -> Color {
        let type = agent.type.lowercased()
        if type.contains("research") { return Color(red: 0.36, green: 0.51, blue: 0.74) }
        if type.contains("code") { return Color(red: 0.24, green: 0.48, blue: 0.32) }
        if type.contains("trad") { return Color(red: 0.58, green: 0.42, blue: 0.66) }
        if type.contains("write") { return Color(red: 0.72, green: 0.52, blue: 0.24) }
        return Color("AccentPrimary")
    }
}

// MARK: - Template sheet

/// The "+" sheet: the pattern-template catalog (`GET /api/v1/agent-templates`)
/// with a confirm step before `POST /agents/from-template` runs.
struct AgentTemplateSheet: View {
    /// Runs after the user confirms a template; the hub instantiates and
    /// navigates. The sheet dismisses itself first.
    let onConfirm: (AgentTemplate) -> Void

    @StateObject private var hubStore = AgentHubStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var pendingTemplate: AgentTemplate? = nil
    @State private var isConfirmPresented = false

    var body: some View {
        NavigationStack {
            content
                .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
                .navigationTitle("New from Template")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Close") { dismiss() }
                    }
                }
        }
        .confirmationDialog(
            "Create \(pendingTemplate?.name ?? "agent")?",
            isPresented: $isConfirmPresented,
            titleVisibility: .visible
        ) {
            Button("Create") {
                if let template = pendingTemplate {
                    pendingTemplate = nil
                    dismiss()
                    onConfirm(template)
                }
            }
            Button("Cancel", role: .cancel) { pendingTemplate = nil }
        } message: {
            Text("This creates a new agent from the template with your default model.")
        }
        .task {
            hubStore.fetchTemplatesIfNeeded()
        }
    }

    @ViewBuilder
    private var content: some View {
        if hubStore.isLoadingTemplates && hubStore.templates.isEmpty {
            ProgressView()
        } else if let templatesError = hubStore.templatesError, hubStore.templates.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load templates",
                message: FriendlyErrorMessage.from(templatesError),
                actionTitle: "Retry",
                action: { hubStore.fetchTemplatesIfNeeded(force: true) }
            )
        } else {
            List(hubStore.templates) { template in
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    pendingTemplate = template
                    isConfirmPresented = true
                }) {
                    HStack(spacing: 12) {
                        Image(systemName: "square.grid.2x2")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundColor(Color("AccentPrimary"))
                            .frame(width: 36, height: 36)
                            .background(Color("AccentPrimary").opacity(0.12))
                            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(template.name)
                                .font(.system(size: 14.5, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            if let description = template.description, !description.isEmpty {
                                Text(description)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(2)
                            }
                            Text(template.pattern ?? template.category)
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundColor(Color("TextSecondary"))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 2.5)
                                .background(Color("BgSecondary"))
                                .clipShape(Capsule())
                                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                                .padding(.top, 4)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(12)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 5, leading: 20, bottom: 5, trailing: 20))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
    }
}
