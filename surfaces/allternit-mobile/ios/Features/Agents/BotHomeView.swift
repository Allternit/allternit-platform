import SwiftUI

/// Single-scroll Bot Home dashboard for an agent/bot.
///
/// Matches the web's `BotHomeView` but adapted to iOS as a vertically stacked
/// card layout (no horizontal tabs). The surface is intentionally dense: hero,
/// quick actions, stats, and section cards all live on one scrollable screen.
struct BotHomeView: View {
    let initialAgent: AgentRecord

    @StateObject private var hubStore = AgentHubStore.shared
    @StateObject private var modelStore = ModelStore.shared
    @StateObject private var artifactStore = ArtifactLibraryStore.shared
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @EnvironmentObject private var modeStore: AppModeStore

    // MARK: - Sessions
    @State private var sessions: [AgentSession] = []
    @State private var isLoadingSessions = false
    @State private var sessionsError: String? = nil

    // MARK: - Threads (P9.2)
    @State private var threads: [BotThread] = []

    // MARK: - Artifacts
    @State private var selectedArtifact: ArtifactRecord? = nil

    // MARK: - Automation
    @State private var loopCount = 0
    @State private var routineCount = 0
    @State private var cronCount = 0
    @State private var automationLoading = false
    @State private var automationError: String? = nil

    // MARK: - Workspace files
    @State private var workspaceFileCount = 0
    @State private var workspaceFilesLoading = false

    // MARK: - Webhooks
    @State private var webhookCount = 0
    @State private var webhooksLoading = false

    // MARK: - Sheets / navigation flags
    @State private var isModelPickerPresented = false
    @State private var isPromptEditorPresented = false
    @State private var isGreetingEditorPresented = false
    @State private var isAvatarEditorPresented = false
    @State private var isTaskComposerPresented = false
    @State private var isWorkspaceFilesSheetPresented = false
    @State private var isBehaviorSheetPresented = false
    @State private var isPublishSheetPresented = false
    @State private var isWebhooksSheetPresented = false
    @State private var desktopURL: URL? = nil
    @State private var taskInput = ""
    @State private var isStartingTask = false
    @State private var startTaskError: String? = nil

    // MARK: - API clients
    private let chatClient = AgentChatClient()
    private let agentClient = AgentClient()
    private let webhookClient = WebhookClient.shared

    /// The store's row when present (reflects edits made on this screen),
    /// falling back to the record the list pushed with.
    private var agent: AgentRecord {
        hubStore.agent(withId: initialAgent.id) ?? initialAgent
    }

    private var displayName: String { agent.botDisplayName }
    private var tagline: String { agent.botTagline }
    private var accentColor: Color {
        if let hex = agent.botAccentColor {
            return Color(hex: hex)
        }
        return Color("AccentPrimary")
    }

    private var botSessions: [AgentSession] {
        sessions.filter { session in
            session.agentId == agent.id
                || (session.agentId == nil && session.name?.localizedCaseInsensitiveContains(agent.name) == true)
        }
    }

    private var botSessionIds: Set<String> {
        Set(botSessions.compactMap(\.id))
    }

    private var botArtifacts: [SavedArtifact] {
        artifactStore.artifacts.filter { saved in
            guard let sessionId = saved.sessionId else { return false }
            return botSessionIds.contains(sessionId)
        }
    }

    private var hasMissingSecrets: Bool {
        agent.secretRefs.contains { $0.required && $0.vaultRef == nil }
    }

    private var automationTotal: Int {
        loopCount + routineCount + cronCount
    }

    private var welcomeMessage: String? {
        let fromProfile = agent.botProfile?.welcomeMessage?.trimmingCharacters(in: .whitespacesAndNewlines)
        let fromConfig = agent.greeting?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (fromProfile?.isEmpty == false ? fromProfile : fromConfig)
    }

    private var starterPrompts: [String] {
        let fromProfile = agent.botProfile?.starterPrompts.filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        let fromConfig = agent.suggestedPrompts.filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return (fromProfile?.isEmpty == false ? fromProfile! : fromConfig)
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                heroSection

                if welcomeMessage != nil || !starterPrompts.isEmpty {
                    welcomeSection
                }

                actionButtons
                statsSection
                tasksCard
                artifactsCard
                automationCard
                runtimeCard
                desktopCard
                configCard
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(agent.name)
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $isModelPickerPresented) {
            BotBrainPickerSheet(agent: agent)
        }
        .sheet(isPresented: $isPromptEditorPresented) {
            SystemPromptEditorSheet(agentId: agent.id, initialPrompt: agent.systemPrompt ?? "")
        }
        .sheet(isPresented: $isGreetingEditorPresented) {
            GreetingEditorSheet(agent: agent)
        }
        .sheet(isPresented: $isAvatarEditorPresented) {
            AgentAvatarEditorSheet(agent: agent)
        }
        .sheet(isPresented: $isTaskComposerPresented) {
            taskComposerSheet
        }
        .sheet(isPresented: $isWorkspaceFilesSheetPresented) {
            BotWorkspaceFilesSheet(agent: agent)
        }
        .sheet(isPresented: $isBehaviorSheetPresented) {
            behaviorSheet
        }
        .sheet(isPresented: $isPublishSheetPresented) {
            PublishAgentSheet(sourceAgent: agent)
        }
        .sheet(isPresented: $isWebhooksSheetPresented) {
            BotWebhooksSheet()
        }
        .sheet(item: $selectedArtifact) { artifact in
            ArtifactDetailsView(artifact: artifact)
        }
        .sheet(isPresented: Binding<Bool>(
            get: { desktopURL != nil },
            set: { if !$0 { desktopURL = nil } }
        )) {
            if let url = desktopURL {
                ACIWebBrowserView(initialURL: url, onExit: { desktopURL = nil })
            }
        }
        .task {
            modelStore.fetchModelsIfNeeded()
            await loadAll()
        }
    }

    // MARK: - Hero

    private var heroSection: some View {
        VStack(spacing: 10) {
            Button(action: {
                hapticLight()
                isAvatarEditorPresented = true
            }) {
                ZStack(alignment: .bottomTrailing) {
                    AgentAvatarView(agent: agent, size: 72)
                    Image(systemName: "pencil.circle.fill")
                        .font(.system(size: 20))
                        .foregroundColor(accentColor)
                        .background(Color("BgPrimary").clipShape(Circle()))
                        .offset(x: 2, y: 2)
                }
            }
            .buttonStyle(.plain)

            VStack(spacing: 4) {
                Text(displayName)
                    .font(.system(.title2, design: .serif))
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))

                Text(tagline)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
            }

            FlowLayout(spacing: 6) {
                BotBadge(text: agent.isBot ? "Bot" : agent.type.capitalized, color: accentColor)
                if let mode = agent.harness?.mode, !mode.isEmpty {
                    BotBadge(text: "\(mode) harness", style: .subdued)
                }
                if agent.vmOperator?.enabled == true {
                    BotBadge(text: "VM Operator", style: .accent)
                }
                if hasMissingSecrets {
                    BotBadge(text: "Missing secrets", style: .warning)
                }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
    }

    // MARK: - Welcome

    private var welcomeSection: some View {
        BotCard(title: "Welcome", icon: "hand.wave", accent: accentColor) {
            VStack(alignment: .leading, spacing: 10) {
                if let welcomeMessage {
                    Text(welcomeMessage)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.leading)
                }

                if !starterPrompts.isEmpty {
                    FlowLayout(spacing: 8) {
                        ForEach(starterPrompts, id: \.self) { prompt in
                            Button(action: {
                                hapticLight()
                                taskInput = prompt
                                isTaskComposerPresented = true
                            }) {
                                Text(prompt)
                                    .font(.caption.weight(.medium))
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 6)
                                    .background(accentColor.opacity(0.12))
                                    .foregroundColor(accentColor)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: Theme.radiusSM)
                                            .stroke(accentColor.opacity(0.35), lineWidth: 1)
                                    )
                                    .cornerRadius(Theme.radiusSM)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
    }

    // MARK: - Actions

    private var actionButtons: some View {
        HStack(spacing: 10) {
            Button(action: {
                hapticLight()
                startChatSession()
            }) {
                Label("Chat", systemImage: "message")
                    .font(.subheadline.weight(.semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                    .background(Color("BgPanel"))
                    .foregroundColor(Color("TextPrimary"))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                    .cornerRadius(Theme.radiusMD)
            }
            .buttonStyle(.plain)

            Button(action: {
                hapticLight()
                isTaskComposerPresented = true
            }) {
                Label("Run Task", systemImage: "play.fill")
                    .font(.subheadline.weight(.semibold))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                    .background(accentColor)
                    .foregroundColor(.white)
                    .cornerRadius(Theme.radiusMD)
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - Stats

    private var statsSection: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
            StatTile(label: "Tasks", value: botSessions.count, icon: "message", accent: accentColor)
            StatTile(label: "Artifacts", value: botArtifacts.count, icon: "doc", accent: accentColor)
            StatTile(label: "Connectors", value: agent.connectorBindings.count, icon: "network", accent: accentColor)
            StatTile(label: "Secrets", value: agent.secretRefs.count, icon: "key", accent: hasMissingSecrets ? Theme.statusWarning : accentColor, warning: hasMissingSecrets)
        }
    }

    // MARK: - Tasks card

    private var tasksCard: some View {
        BotCard(title: threads.isEmpty ? "Tasks" : "Threads", icon: "message", accent: accentColor) {
            VStack(alignment: .leading, spacing: 0) {
                if !threads.isEmpty {
                    VStack(spacing: 0) {
                        ForEach(threads.prefix(8)) { thread in
                            threadRow(thread)
                            if thread.id != threads.prefix(8).last?.id {
                                Divider()
                                    .background(Theme.borderWarmSubtle)
                            }
                        }
                    }
                } else if isLoadingSessions && botSessions.isEmpty {
                    ProgressView()
                        .frame(maxWidth: .infinity, alignment: .center)
                        .padding(.vertical, 20)
                } else if let sessionsError, botSessions.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Couldn't load tasks")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text(sessionsError)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        Button("Retry") {
                            Task { await loadSessions() }
                        }
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                    }
                    .padding(.vertical, 8)
                } else if botSessions.isEmpty {
                    emptyTasksView
                } else {
                    VStack(spacing: 0) {
                        ForEach(botSessions.prefix(5)) { session in
                            taskRow(session)
                            if session.id != botSessions.prefix(5).last?.id {
                                Divider()
                                    .background(Theme.borderWarmSubtle)
                            }
                        }
                    }
                }
            }
        }
    }

    private var emptyTasksView: some View {
        VStack(spacing: 8) {
            Image(systemName: "message")
                .font(.system(size: 28))
                .foregroundColor(Color("TextSecondary").opacity(0.6))
            Text("No tasks yet")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("Delegate a task to this bot and it will run in its workspace or sandbox.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
            Button {
                isTaskComposerPresented = true
            } label: {
                Label("Run First Task", systemImage: "play.fill")
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(accentColor)
                    .foregroundColor(.white)
                    .cornerRadius(Theme.radiusMD)
            }
            .padding(.top, 4)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 16)
    }

    private func threadColor(_ group: String) -> Color {
        switch group {
        case "waiting": return .orange
        case "working": return .blue
        case "resolved": return .green
        default: return Color("TextSecondary")
        }
    }

    private func threadRow(_ thread: BotThread) -> some View {
        Button(action: {
            hapticLight()
            if let sid = thread.currentSessionId { openChatSession(sid) }
        }) {
            HStack(spacing: 12) {
                Circle()
                    .fill(threadColor(thread.group))
                    .frame(width: 7, height: 7)
                VStack(alignment: .leading, spacing: 2) {
                    Text(thread.title)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text([thread.statusLine ?? thread.status, relativeTime(thread.lastActivityAt)].joined(separator: " · "))
                        .font(.caption)
                        .foregroundColor(thread.group == "waiting" ? .orange : Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer()
                if let p = thread.progressLabel {
                    Text(p)
                        .font(.caption.monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(thread.currentSessionId == nil)
    }

    private func taskRow(_ session: AgentSession) -> some View {
        Button(action: {
            hapticLight()
            openChatSession(session.id)
        }) {
            HStack(spacing: 12) {
                Image(systemName: "message")
                    .font(.system(size: 14))
                    .foregroundColor(accentColor)
                    .frame(width: 28, height: 28)
                    .background(accentColor.opacity(0.12))
                    .cornerRadius(8)

                VStack(alignment: .leading, spacing: 2) {
                    Text(session.name ?? "Untitled task")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)

                    Text("\(session.messageCount) message\(session.messageCount == 1 ? "" : "s") · \(relativeTime(session.updatedAt))")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }

                Spacer()

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Artifacts card

    private var artifactsCard: some View {
        BotCard(title: "Artifacts", icon: "doc.text", accent: accentColor) {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("\(botArtifacts.count) saved")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    if !botArtifacts.isEmpty {
                        Button("View library") {
                            hapticLight()
                            modeStore.selectBarItem(.artifacts)
                        }
                        .font(.caption)
                        .foregroundColor(accentColor)
                    }
                }

                if botArtifacts.isEmpty {
                    Text("Artifacts produced by this bot in chat will appear here.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.top, 4)
                } else {
                    VStack(spacing: 0) {
                        ForEach(botArtifacts.prefix(3)) { saved in
                            Button(action: {
                                hapticLight()
                                selectedArtifact = saved.record
                            }) {
                                HStack(spacing: 12) {
                                    Image(systemName: saved.record.isPreviewable ? "safari" : "doc.text")
                                        .font(.system(size: 14))
                                        .foregroundColor(accentColor)
                                        .frame(width: 28, height: 28)
                                        .background(accentColor.opacity(0.12))
                                        .cornerRadius(8)

                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(saved.record.title)
                                            .font(.subheadline)
                                            .foregroundColor(Color("TextPrimary"))
                                            .lineLimit(1)
                                        Text(saved.record.fileType.uppercased())
                                            .font(.caption)
                                            .foregroundColor(Color("TextSecondary"))
                                    }

                                    Spacer()

                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 11, weight: .bold))
                                        .foregroundColor(Color("TextSecondary"))
                                }
                                .padding(.vertical, 10)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)

                            if saved.id != botArtifacts.prefix(3).last?.id {
                                Divider()
                                    .background(Theme.borderWarmSubtle)
                            }
                        }
                    }
                    .padding(.top, 8)
                }
            }
        }
    }

    // MARK: - Automation summary card

    private var automationCard: some View {
        BotCard(title: "Automation", icon: "clock.arrow.circlepath", accent: accentColor) {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    if automationLoading && automationTotal == 0 {
                        ProgressView()
                            .scaleEffect(0.8)
                    } else {
                        Text("\(automationTotal) scheduled")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    Button("View all") {
                        hapticLight()
                        modeStore.selectBarItem(.automation)
                    }
                    .font(.caption)
                    .foregroundColor(accentColor)
                }

                if automationTotal == 0 && !automationLoading {
                    Text("No cron jobs, routines, or loops for this bot yet.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.top, 4)
                } else {
                    LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                        automationPill(label: "Loops", value: loopCount)
                        automationPill(label: "Routines", value: routineCount)
                        automationPill(label: "Cron", value: cronCount)
                    }
                    .padding(.top, 10)
                }
            }
        }
    }

    private func automationPill(label: String, value: Int) -> some View {
        VStack(spacing: 4) {
            Text("\(value)")
                .font(.system(size: 20, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            Text(label)
                .font(.system(size: 10, weight: .semibold))
                .textCase(.uppercase)
                .tracking(0.5)
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 10)
        .background(Color("BgPanel"))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmSubtle, lineWidth: 1)
        )
        .cornerRadius(Theme.radiusMD)
    }

    // MARK: - Runtime card

    private var runtimeCard: some View {
        BotCard(title: "Runtime", icon: "bolt.fill", accent: accentColor) {
            VStack(alignment: .leading, spacing: 16) {
                connectorsSection
                secretsSection
                webhooksSection
                vmSection
            }
        }
    }

    private var connectorsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Connectors")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if agent.connectorBindings.isEmpty {
                    Text("None")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            if agent.connectorBindings.isEmpty {
                Text("No connectors bound yet. Connect apps so this bot can act on your behalf.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                FlowLayout(spacing: 8) {
                    ForEach(agent.connectorBindings) { binding in
                        ConnectorChip(binding: binding, accent: accentColor)
                    }
                }
            }
        }
    }

    private var secretsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Secrets")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if hasMissingSecrets {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }
            }

            if agent.secretRefs.isEmpty {
                Text("No secrets declared.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                VStack(spacing: 6) {
                    ForEach(agent.secretRefs) { secret in
                        HStack {
                            Text(secret.displayName)
                                .font(.caption)
                                .foregroundColor(Color("TextPrimary"))
                            Spacer()
                            if secret.vaultRef != nil {
                                Text(masked(secret.vaultRef!))
                                    .font(.caption.monospaced())
                                    .foregroundColor(Color("TextSecondary"))
                            } else {
                                Text(secret.required ? "Required" : "Not set")
                                    .font(.caption)
                                    .foregroundColor(secret.required ? Theme.statusWarning : Color("TextSecondary"))
                            }
                        }
                    }
                }
            }
        }
    }

    private var webhooksSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Webhooks")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if webhooksLoading && webhookCount == 0 {
                    ProgressView()
                        .scaleEffect(0.7)
                } else {
                    Text("\(webhookCount)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            Button(action: {
                hapticLight()
                isWebhooksSheetPresented = true
            }) {
                HStack {
                    Text(webhookCount == 0 ? "No org subscriptions" : "Manage org subscriptions")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .buttonStyle(.plain)
        }
    }

    private var vmSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Virtual Computer")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(agent.vmOperator?.enabled == true ? "Enabled" : "Disabled")
                    .font(.caption)
                    .foregroundColor(agent.vmOperator?.enabled == true ? Theme.statusSuccess : Color("TextSecondary"))
            }

            if let vm = agent.vmOperator, vm.enabled {
                FlowLayout(spacing: 6) {
                    if let provider = vm.provider {
                        BotBadge(text: provider, color: accentColor)
                    }
                    if let image = vm.image {
                        BotBadge(text: image, style: .subdued)
                    }
                    if let network = vm.networkPolicy {
                        BotBadge(text: network, style: .subdued)
                    }
                    if let persistence = vm.persistence {
                        BotBadge(text: persistence, style: .subdued)
                    }
                    if vm.vncEnabled {
                        BotBadge(text: "VNC", style: .accent)
                    }
                }

                if let resources = resourcesLabel(vm.resources) {
                    Text(resources)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                Text("Enable a virtual computer to let this bot run tasks inside an isolated sandbox.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    // MARK: - Desktop preview card

    private var desktopCard: some View {
        BotCard(title: "Desktop", icon: "desktopcomputer", accent: accentColor) {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(agent.vmOperator?.enabled == true ? "Sandbox ready" : "No sandbox")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if let vm = agent.vmOperator, vm.enabled, vm.vncEnabled {
                        BotBadge(text: "VNC", style: .accent)
                    }
                }

                if let vm = agent.vmOperator, vm.enabled {
                    if let vncUrl = vm.vncUrl, let url = URL(string: vncUrl) {
                        Button(action: {
                            hapticLight()
                            desktopURL = url
                        }) {
                            HStack(spacing: 8) {
                                Image(systemName: "eye")
                                    .font(.system(size: 13))
                                Text("Open desktop preview")
                                    .font(.subheadline.weight(.semibold))
                                Spacer()
                                Image(systemName: "arrow.up.right")
                                    .font(.system(size: 12, weight: .bold))
                            }
                            .foregroundColor(.white)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 10)
                            .background(accentColor)
                            .cornerRadius(Theme.radiusMD)
                        }
                        .buttonStyle(.plain)
                    } else {
                        Text(vm.vncEnabled
                             ? "VNC is enabled, but no viewer URL was returned by the runtime."
                             : "The virtual computer is enabled. VNC preview is turned off.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                } else {
                    Text("Turn on the virtual computer in Runtime to stream the bot's desktop.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
    }

    // MARK: - Config card

    private var configCard: some View {
        BotCard(title: "Config", icon: "gearshape.fill", accent: accentColor) {
            VStack(spacing: 0) {
                Button(action: {
                    hapticLight()
                    isModelPickerPresented = true
                }) {
                    configRow(icon: "cpu", title: "Brain", value: agent.model.isEmpty ? "Default" : "\(agent.provider)/\(agent.model)")
                }

                Divider().background(Theme.borderWarmSubtle)

                Button(action: {
                    hapticLight()
                    isPromptEditorPresented = true
                }) {
                    configRow(icon: "text.quote", title: "System prompt", value: agent.systemPrompt?.isEmpty == false ? "Edit" : "Add")
                }

                Divider().background(Theme.borderWarmSubtle)

                Button(action: {
                    hapticLight()
                    isGreetingEditorPresented = true
                }) {
                    configRow(icon: "hand.wave", title: "Greeting", value: agent.greeting?.isEmpty == false ? "Edit" : "Add")
                }

                Divider().background(Theme.borderWarmSubtle)

                Button(action: {
                    hapticLight()
                    isWorkspaceFilesSheetPresented = true
                }) {
                    configRow(
                        icon: "doc.text",
                        title: "Workspace files",
                        value: workspaceFilesLoading && workspaceFileCount == 0 ? "Loading" : "\(workspaceFileCount) files"
                    )
                }

                Divider().background(Theme.borderWarmSubtle)

                Button(action: {
                    hapticLight()
                    isBehaviorSheetPresented = true
                }) {
                    configRow(icon: "slider.horizontal.3", title: "Behavior", value: "View")
                }

                Divider().background(Theme.borderWarmSubtle)

                Button(action: {
                    hapticLight()
                    isPublishSheetPresented = true
                }) {
                    configRow(icon: "storefront", title: "Publish", value: "Marketplace")
                }
            }
            .buttonStyle(.plain)
        }
    }

    private func configRow(icon: String, title: String, value: String) -> some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 14))
                .foregroundColor(accentColor)
                .frame(width: 28, height: 28)
                .background(accentColor.opacity(0.12))
                .cornerRadius(8)

            Text(title)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))

            Spacer()

            Text(value)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)

            Image(systemName: "chevron.right")
                .font(.system(size: 11, weight: .bold))
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.vertical, 10)
        .contentShape(Rectangle())
    }

    // MARK: - Behavior sheet

    private var behaviorSheet: some View {
        NavigationStack {
            List {
                Section {
                    behaviorRow("Type", value: agent.type.capitalized)
                    behaviorRow("Trust tier", value: agent.trustTier.capitalized)
                    behaviorRow("Mode", value: agent.mode.capitalized)
                    behaviorRow("Surfaces", value: agent.enabledModes.map { $0.capitalized }.joined(separator: ", ").isEmpty ? "None" : agent.enabledModes.map { $0.capitalized }.joined(separator: ", "))
                } header: {
                    Text("Behavior")
                } footer: {
                    Text("These values are set when the agent is created. Edit them from the agent registry on the web.")
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Behavior")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { isBehaviorSheetPresented = false }
                }
            }
        }
    }

    private func behaviorRow(_ key: String, value: String) -> some View {
        HStack {
            Text(key)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.trailing)
        }
    }

    // MARK: - Task composer sheet

    private var taskComposerSheet: some View {
        NavigationStack {
            VStack(spacing: 16) {
                if let startTaskError {
                    Text(startTaskError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                        .padding(.horizontal, 16)
                }

                if !starterPrompts.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Quick start")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        FlowLayout(spacing: 8) {
                            ForEach(starterPrompts, id: \.self) { prompt in
                                Button(action: {
                                    taskInput = prompt
                                }) {
                                    Text(prompt)
                                        .font(.caption.weight(.medium))
                                        .padding(.horizontal, 10)
                                        .padding(.vertical, 6)
                                        .background(accentColor.opacity(0.12))
                                        .foregroundColor(accentColor)
                                        .overlay(
                                            RoundedRectangle(cornerRadius: Theme.radiusSM)
                                                .stroke(accentColor.opacity(0.35), lineWidth: 1)
                                        )
                                        .cornerRadius(Theme.radiusSM)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    .padding(.horizontal, 16)
                }

                ZStack(alignment: .topLeading) {
                    TextEditor(text: $taskInput)
                        .font(.body)
                        .foregroundColor(Color("TextPrimary"))
                        .scrollContentBackground(.hidden)
                        .padding(12)
                        .accessibilityLabel("Task prompt")
                        .accessibilityHint("Describe the task you want this bot to run")

                    if taskInput.isEmpty {
                        Text("Describe the task you want this bot to run…")
                            .font(.body)
                            .foregroundColor(Color("TextSecondary").opacity(0.7))
                            .padding(20)
                            .allowsHitTesting(false)
                            .accessibilityHidden(true)
                    }
                }
                .background(Color("BgPanel"))
                .cornerRadius(Theme.radiusMD)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Theme.borderWarmDefault, lineWidth: 1)
                )
                .frame(minHeight: 120)
                .padding(.horizontal, 16)

                Spacer()
            }
            .padding(.top, 12)
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Run Task")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { isTaskComposerPresented = false }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: submitTask) {
                        if isStartingTask {
                            ProgressView()
                        } else {
                            Text("Run")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(taskInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isStartingTask)
                }
            }
        }
    }

    // MARK: - Helpers

    private func loadAll() async {
        await withTaskGroup(of: Void.self) { group in
            group.addTask { await loadSessions() }
            group.addTask { await loadThreads() }
            group.addTask { await loadArtifacts() }
            group.addTask { await loadAutomation() }
            group.addTask { await loadWorkspaceFileCount() }
            group.addTask { await loadWebhookCount() }
        }
    }

    private func loadThreads() async {
        do {
            threads = try await ThreadsClient.shared.listThreads(botId: agent.id)
                .sorted { ($0.groupOrder, $1.lastActivityAt) < ($1.groupOrder, $0.lastActivityAt) }
        } catch {
            // Older servers have no threads; the card shows sessions instead.
            threads = []
        }
    }

    private func loadSessions() async {
        isLoadingSessions = true
        sessionsError = nil
        do {
            sessions = try await chatClient.listSessions()
        } catch is CancellationError {
            // no-op
        } catch {
            sessionsError = error.localizedDescription
        }
        isLoadingSessions = false
    }

    private func loadArtifacts() async {
        await artifactStore.refreshFromBackend()
    }

    private func loadAutomation() async {
        automationLoading = true
        automationError = nil
        do {
            async let loopsTask = LoopsClient.shared.listLoops()
            async let routinesTask = RoutinesClient.shared.listRoutines()
            async let cronTask = CronClient.shared.listJobs()
            let (loops, routines, cron) = try await (loopsTask, routinesTask, cronTask)
            loopCount = loops.filter { $0.agentId == agent.id }.count
            routineCount = routines.filter { $0.agentId == agent.id }.count
            cronCount = cron.filter { $0.agentConfig?.agentId == agent.id }.count
        } catch is CancellationError {
            // no-op
        } catch {
            automationError = error.localizedDescription
        }
        automationLoading = false
    }

    private func loadWorkspaceFileCount() async {
        workspaceFilesLoading = true
        do {
            let files = try await agentClient.listWorkspaceFiles(agentId: agent.id)
            workspaceFileCount = files.count
        } catch {
            workspaceFileCount = 0
        }
        workspaceFilesLoading = false
    }

    private func loadWebhookCount() async {
        webhooksLoading = true
        do {
            let subs = try await webhookClient.listSubscriptions()
            webhookCount = subs.count
        } catch {
            webhookCount = 0
        }
        webhooksLoading = false
    }

    private func startChatSession() {
        Task {
            do {
                let session = try await chatClient.createSession(
                    name: "\(displayName) chat",
                    originSurface: "chat",
                    sessionMode: "agent",
                    agentId: agent.id,
                    agentName: agent.name
                )
                openChatSession(session.id)
                await loadSessions()
            } catch {
                sessionsError = error.localizedDescription
            }
        }
    }

    private func submitTask() {
        let prompt = taskInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { return }
        isStartingTask = true
        startTaskError = nil
        Task {
            do {
                let session = try await chatClient.createSession(
                    name: String(prompt.prefix(40)).appending(prompt.count > 40 ? "…" : ""),
                    originSurface: "chat",
                    sessionMode: "agent",
                    agentId: agent.id,
                    agentName: agent.name
                )
                openChatSession(session.id)
                await loadSessions()
                isTaskComposerPresented = false
                taskInput = ""
            } catch {
                startTaskError = error.localizedDescription
            }
            isStartingTask = false
        }
    }

    private func openChatSession(_ sessionId: String) {
        NotificationCenter.default.post(
            name: .openChatSession,
            object: nil,
            userInfo: ["sessionId": sessionId, "agentId": agent.id]
        )
    }

    private func hapticLight() {
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
    }

    private func masked(_ value: String) -> String {
        if value.count <= 8 { return "••••••" }
        return "\(value.prefix(3))…\(value.suffix(3))"
    }

    private func resourcesLabel(_ resources: VMResources) -> String? {
        let parts = [resources.cpu, resources.memory, resources.disk].compactMap { $0 }
        return parts.isEmpty ? nil : "Resources: \(parts.joined(separator: " · "))"
    }
}

// MARK: - BotCard

struct BotCard<Content: View>: View {
    let title: String
    let icon: String
    let accent: Color
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(accent)
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
            }
            content
        }
        .padding(14)
        .background(Color("BgPanel"))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmSubtle, lineWidth: 1)
        )
        .cornerRadius(Theme.radiusLG)
    }
}

// MARK: - StatTile

struct StatTile: View {
    let label: String
    let value: Int
    let icon: String
    let accent: Color
    var warning: Bool = false

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 4) {
                Text(label)
                    .font(.system(size: 10, weight: .semibold))
                    .textCase(.uppercase)
                    .tracking(0.5)
                    .foregroundColor(Color("TextSecondary"))
                Text("\(value)")
                    .font(.system(size: 24, weight: .medium))
                    .foregroundColor(warning ? Theme.statusWarning : Color("TextPrimary"))
            }
            Spacer()
            Image(systemName: icon)
                .font(.system(size: 18))
                .foregroundColor(accent)
                .frame(width: 36, height: 36)
                .background(accent.opacity(0.12))
                .cornerRadius(10)
        }
        .padding(12)
        .background(Color("BgPanel"))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmSubtle, lineWidth: 1)
        )
        .cornerRadius(Theme.radiusMD)
    }
}

// MARK: - BotBadge

struct BotBadge: View {
    enum Style {
        case filled, subdued, accent, warning
    }

    let text: String
    var color: Color? = nil
    var style: Style = .filled

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(background)
            .foregroundColor(foreground)
            .cornerRadius(12)
    }

    private var background: Color {
        switch style {
        case .filled:
            return (color ?? Color("AccentPrimary")).opacity(0.14)
        case .subdued:
            return Color("BgSecondary")
        case .accent:
            return Color("AccentPrimary").opacity(0.14)
        case .warning:
            return Theme.statusWarning.opacity(0.14)
        }
    }

    private var foreground: Color {
        switch style {
        case .filled:
            return color ?? Color("AccentPrimary")
        case .subdued:
            return Color("TextSecondary")
        case .accent:
            return Color("AccentPrimary")
        case .warning:
            return Theme.statusWarning
        }
    }
}

// MARK: - ConnectorChip

struct ConnectorChip: View {
    let binding: ConnectorBinding
    let accent: Color

    var body: some View {
        HStack(spacing: 6) {
            Text(binding.displayName)
                .font(.caption.weight(.medium))
            if binding.autonomous {
                Image(systemName: "checkmark.circle.fill")
                    .font(.caption2)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            binding.autonomous
                ? accent.opacity(0.14)
                : Color("BgSecondary")
        )
        .foregroundColor(binding.autonomous ? accent : Color("TextPrimary"))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusSM)
                .stroke(binding.autonomous ? accent.opacity(0.35) : Theme.borderWarmDefault, lineWidth: 1)
        )
        .cornerRadius(Theme.radiusSM)
    }
}

// MARK: - FlowLayout

private struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let result = FlowResult(in: proposal.width ?? 0, subviews: subviews, spacing: spacing)
        return result.size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = FlowResult(in: bounds.width, subviews: subviews, spacing: spacing)
        for (index, subview) in subviews.enumerated() {
            subview.place(at: CGPoint(x: bounds.minX + result.frames[index].minX, y: bounds.minY + result.frames[index].minY), proposal: .unspecified)
        }
    }

    private struct FlowResult {
        var size: CGSize = .zero
        var frames: [CGRect] = []

        init(in maxWidth: CGFloat, subviews: Subviews, spacing: CGFloat) {
            var x: CGFloat = 0
            var y: CGFloat = 0
            var rowHeight: CGFloat = 0
            for subview in subviews {
                let size = subview.sizeThatFits(.unspecified)
                if x + size.width > maxWidth, x > 0 {
                    x = 0
                    y += rowHeight + spacing
                    rowHeight = 0
                }
                frames.append(CGRect(x: x, y: y, width: size.width, height: size.height))
                rowHeight = max(rowHeight, size.height)
                x += size.width + spacing
            }
            self.size = CGSize(width: maxWidth, height: y + rowHeight)
        }
    }
}

// MARK: - Relative time

private func relativeTime(_ iso: String) -> String {
    let t = ISO8601DateParser.date(from: iso)?.timeIntervalSince1970 ?? 0
    if t <= 0 { return iso }
    let diff = Date().timeIntervalSince1970 - t
    let min = Int(diff / 60)
    if min < 1 { return "just now" }
    if min < 60 { return "\(min)m ago" }
    let hr = min / 60
    if hr < 24 { return "\(hr)h ago" }
    let day = hr / 24
    if day < 30 { return "\(day)d ago" }
    let mo = day / 30
    if mo < 12 { return mo == 1 ? "1 month ago" : "\(mo) months ago" }
    let yr = mo / 12
    return yr == 1 ? "1 year ago" : "\(yr) years ago"
}

private enum ISO8601DateParser {
    static func date(from string: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: string) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: string)
    }
}
