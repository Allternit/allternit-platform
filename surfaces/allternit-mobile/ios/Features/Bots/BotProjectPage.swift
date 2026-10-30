import SwiftUI

/// A bot project's page (views/bots/ProjectHomeView.tsx in BaseProjectView's
/// launch layout), part for part in the web's order: breadcrumb, serif title,
/// actions, chips, the To: row and composer to the coordinator, the bot
/// launcher, the Team · Campaign · Delegate · Threads · Artifacts · Routines ·
/// Status · Memory · Context cards, then the conversation with Al. Polls every
/// 5 s like the web. The web's phone squeezes the title beside the actions;
/// here the title gets the full width and the actions their own row.
struct BotProjectPage: View {
    private let projectId: String

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var backdrops = StageBackdropStore.shared
    @ObservedObject private var botColors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var dictation = DictationController()
    @StateObject private var attachments = AttachmentStore()

    // The list row captured on push; edits land here at once.
    @State private var title: String
    @State private var details: String?
    @State private var favorite: Bool
    @State private var archived: Bool
    @State private var botIds: [String]

    @State private var threads: [BotThread] = []
    @State private var messages: [ProjectMessage] = []
    @State private var artifacts: [ArtifactV2Summary] = []
    @State private var isLoading = true
    @State private var loadError: String?
    @State private var status: String?
    @State private var draft = ""
    @State private var sending = false
    @State private var dictationBase = ""
    @State private var isDictating = false
    @State private var startingBotId: String?
    @State private var sheet: PageSheet?
    @State private var confirmDelete = false

    private enum PageSheet: Identifiable {
        case appearance, settings, edit, template, graph, memory, needsYou, addBot, statusPage
        case artifact(ArtifactV2Summary)

        var id: String {
            switch self {
            case .appearance: return "appearance"
            case .settings: return "settings"
            case .edit: return "edit"
            case .template: return "template"
            case .graph: return "graph"
            case .memory: return "memory"
            case .needsYou: return "needsYou"
            case .addBot: return "addBot"
            case .statusPage: return "statusPage"
            case .artifact(let item): return "artifact-\(item.id)"
            }
        }
    }

    init(project: BotProjectSummary) {
        projectId = project.id
        _title = State(initialValue: project.title)
        _details = State(initialValue: project.description)
        _favorite = State(initialValue: project.favorite)
        _archived = State(initialValue: project.archived)
        _botIds = State(initialValue: project.botIds)
    }

    private var context: ProjectPaneContext { ProjectPaneContext(id: projectId, title: title, botIds: botIds) }
    private var team: [AgentRecord] { botIds.compactMap { store.bot($0) } }
    private var otherBots: [AgentRecord] { store.bots.filter { !botIds.contains($0.id) } }
    private var waiting: [BotThread] {
        threads.filter { $0.group == "waiting" }.sorted { $0.lastActivityAt > $1.lastActivityAt }
    }

    private var scope: BackdropScope { .project(projectId) }
    private var backdropVariant: String { backdrops.variant(for: .project, scope: scope) }
    private var sessionIds: Set<String> { Set(threads.compactMap(\.currentSessionId)) }
    private var sessionKey: String { sessionIds.sorted().joined(separator: "|") }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    breadcrumb
                    header
                    launchComposer
                    cards
                    conversation
                        .id("conversation")
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 32)
            }
            .onChange(of: messages.count) { before, after in
                guard after > before, sending else { return }
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("conversation", anchor: .bottom) }
            }
        }
        .background {
            if backdropVariant == "plain" {
                Color("BgPrimary").ignoresSafeArea()
            } else {
                StageBackdropView(variant: backdropVariant, colors: botIds.map { botColors.hex(for: store.bot($0)) })
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
            }
        }
        .backdropTouches(backdropVariant != "plain")
        .navigationTitle("Project")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await refreshAll() }
        .task { await pollLoop() }
        .task(id: sessionKey) { await loadArtifacts() }
        .onChange(of: dictation.transcript) { _, transcript in
            if isDictating { draft = dictationBase + transcript }
        }
        .onChange(of: dictation.isRecording) { _, recording in
            if !recording && isDictating {
                draft = dictationBase + dictation.transcript
                isDictating = false
            }
        }
        .alert("Delete project", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive) { deleteProject() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Delete \"\(title)\"? Its threads aren't deleted: they stay with their bots, outside any project. This can't be undone.")
        }
        .sheet(item: $sheet) { which in
            sheetContent(which)
        }
    }

    // MARK: - Header

    private var breadcrumb: some View {
        HStack(spacing: 6) {
            Button("Projects") { dismiss() }
                .buttonStyle(.plain)
                .foregroundColor(Color("TextSecondary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(Color("TextTertiary"))
            Text(title)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
        }
        .font(.system(size: 13))
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { sheet = .edit } label: {
                Text(title)
                    .font(.system(size: 28, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .accessibilityHint("Rename project")
            if let details, !details.isEmpty {
                Text(details)
                    .font(.system(size: 13.5))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            actionsRow
            chips
            if let loadError {
                ProjectInlineError(text: loadError) { Task { await refreshAll() } }
            }
        }
    }

    private var actionsRow: some View {
        HStack(spacing: 2) {
            Button { sheet = .appearance } label: {
                HStack(spacing: 6) {
                    Image(systemName: "paintpalette").font(.system(size: 14))
                    Text("Customize appearance").font(.system(size: 13))
                }
                .foregroundColor(Color("TextSecondary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 4)
            ProjectIconButton(systemName: "gearshape", label: "Project settings") { sheet = .settings }
            ProjectIconButton(systemName: favorite ? "pin.fill" : "pin",
                              label: favorite ? "Unpin project" : "Pin project") { toggleFavorite() }
            Menu {
                Button { sheet = .edit } label: { Label("Edit details", systemImage: "pencil") }
                Button { sheet = .settings } label: { Label("Project settings", systemImage: "gearshape") }
                Button { toggleArchived() } label: {
                    Label(archived ? "Unarchive" : "Archive", systemImage: "archivebox")
                }
                Button { sheet = .template } label: { Label("Save as template", systemImage: "square.grid.2x2") }
                Button(role: .destructive) { confirmDelete = true } label: { Label("Delete", systemImage: "trash") }
            } label: {
                Image(systemName: "ellipsis")
                    .rotationEffect(.degrees(90))
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Project options")
        }
    }

    private var chips: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                AProtocolMark(height: 7, label: "Al")
                Text("Coordinator · Al")
            }
            .font(.system(size: 11.5))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            if !waiting.isEmpty {
                Button { openWaiting() } label: {
                    HStack(spacing: 6) {
                        Circle().fill(Theme.statusWarning).frame(width: 6, height: 6)
                        Text("Needs you · \(waiting.count)")
                    }
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 44)
                    .background(Capsule().fill(Color("BgPanel")))
                    .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: - Composer + launcher

    private var launchComposer: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("To:")
                HStack(spacing: 4) {
                    AProtocolMark(height: 5.5, label: "Al")
                    Text("Al · \(title)").lineLimit(1)
                }
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
            }
            .font(.system(size: 12.5))
            .foregroundColor(Color("TextTertiary"))
            Text("or use @ on a bot below to send it directly")
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextTertiary"))
            ComposerView(
                inputText: $draft,
                attachmentStore: attachments,
                isStreaming: false,
                isSendDisabled: sending,
                hasActiveSession: false,
                deck: .full,
                onSend: { _ in attachments.clear(); send() },
                onStop: {},
                onVoiceMode: toggleDictation,
                placeholderOverride: "Ask Al or start a task…",
                showsPermission: false,
                showsAttach: false
            )
            if let status {
                Text(status)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.statusError)
            }
            launcher
        }
    }

    private var launcher: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: 4) {
                ForEach(team) { bot in
                    Button { startThread(bot) } label: {
                        VStack(spacing: 2) {
                            BotAvatarView(bot: bot, size: 36)
                                .frame(width: 44, height: 44)
                            Text(startingBotId == bot.id ? "Starting…" : bot.botDisplayName)
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                        }
                        .frame(width: 64)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(startingBotId != nil)
                    .accessibilityLabel("Start \(bot.botDisplayName) on a new thread in this project")
                }
                if !otherBots.isEmpty {
                    Button { sheet = .addBot } label: {
                        VStack(spacing: 2) {
                            Circle()
                                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3]))
                                .frame(width: 36, height: 36)
                                .overlay(Image(systemName: "plus").font(.system(size: 13)).foregroundColor(Color("TextTertiary")))
                                .frame(width: 44, height: 44)
                            Text("Add a bot")
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextTertiary"))
                                .lineLimit(1)
                        }
                        .frame(width: 64)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(startingBotId != nil)
                    .accessibilityLabel("Add a bot to this project")
                }
            }
        }
    }

    // MARK: - Cards

    private var cards: some View {
        VStack(alignment: .leading, spacing: 12) {
            ProjectTeamCard(context: context, team: team, threads: threads,
                            onOpenThread: { navigator.openThread($0) },
                            onOpenPane: { navigator.open(.projectPane(context, $0)) },
                            onOpenNode: { navigator.open(.projectNode(dagId: $0.dagId, nodeId: $0.nodeId, title: $0.title)) })
            ProjectCampaignCard(projectId: projectId) { navigator.open(.projectPane(context, $0)) }
            ProjectCard(title: "Delegate") {
                ProjectTextButton("Open") { navigator.open(.projectPane(context, .delegate)) }
            } content: {
                ProjectCardNote("List tasks and hand each one to a bot, or turn on Auto to give them to whoever is free first.")
            }
            threadsCard
            ProjectArtifactsCard(items: artifacts, onOpen: { sheet = .artifact($0) },
                                 onLibrary: { navigator.open(.projectPane(context, .library)) })
            ProjectRoutinesCard(projectId: projectId) { navigator.open(.projectPane(context, .routines)) }
            ProjectStatusCard(projectId: projectId) { sheet = .statusPage }
            ProjectCard(title: "Memory") {
                ProjectTextButton("View memory") { sheet = .memory }
            } content: {
                ProjectCardNote("Project memory will show here after a few chats.")
            }
            ProjectContextFilesCard(projectId: projectId)
        }
    }

    private var threadsCard: some View {
        ProjectCard(title: "Threads") {
            Button { sheet = .graph } label: {
                HStack(spacing: 4) {
                    Image(systemName: "point.3.connected.trianglepath.dotted").font(.system(size: 12))
                    Text("Graph").font(.system(size: 12.5))
                }
                .foregroundColor(threads.isEmpty ? Color("TextTertiary") : Color("TextPrimary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(threads.isEmpty)
        } content: {
            if threads.isEmpty {
                if isLoading {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 8)
                } else {
                    ProjectCardNote("Threads show here once Al starts them.")
                }
            } else {
                BotThreadsPanel(
                    botId: nil,
                    greeting: waiting.isEmpty ? "All on track" : "Needs you",
                    threadsOverride: threads,
                    onOpen: { navigator.openThread($0) },
                    onReopen: { reopen($0) }
                )
                .padding(.horizontal, -8)
            }
        }
    }

    private var conversation: some View {
        ProjectConversationView(
            messages: messages,
            threads: threads,
            artifacts: artifacts,
            welcomeTeam: team.map(\.botDisplayName),
            onOpenThread: { navigator.openThread($0) },
            onOpenArtifact: { sheet = .artifact($0) }
        )
    }

    // MARK: - Sheets

    @ViewBuilder
    private func sheetContent(_ which: PageSheet) -> some View {
        switch which {
        case .appearance:
            StageBackdropScopeSheet(scope: scope, title: "Customize appearance",
                                    colors: botIds.map { botColors.hex(for: store.bot($0)) })
        case .settings:
            ProjectSettingsSheet(projectId: projectId, scope: scope,
                                 colors: botIds.map { botColors.hex(for: store.bot($0)) },
                                 note: "Appearance is saved on this device. Instructions are saved with the project. Al and its threads run on the team bots' own models, so there are no model or permission settings here.",
                                 placeholder: "How should Al manage this project?")
        case .edit:
            ProjectEditDetailsSheet(title: title, details: details ?? "") { newTitle, newDetails in
                try await BotProjectClient.shared.saveDetails(projectId: projectId, title: newTitle, description: newDetails)
                title = newTitle
                details = newDetails.isEmpty ? nil : newDetails
                await reloadLists()
            }
        case .template:
            ProjectSaveTemplateSheet(projectId: projectId, title: title, details: details, botIds: botIds, threads: threads)
        case .graph:
            ProjectThreadGraphSheet(threads: threads) { thread in
                sheet = nil
                navigator.openThread(thread)
            }
        case .memory:
            ProjectMemorySheet(sessionIds: sessionIds)
        case .needsYou:
            ProjectNeedsYouSheet(threads: waiting) { thread in
                sheet = nil
                navigator.openThread(thread)
            }
        case .addBot:
            ProjectAddBotSheet(bots: otherBots) { bot in
                sheet = nil
                startThread(bot)
            }
        case .statusPage:
            ProjectStatusSheet(projectId: projectId)
        case .artifact(let item):
            V2ArtifactView(artifactId: item.id, initialTitle: item.title) {
                artifacts.removeAll { $0.id == item.id }
            }
        }
    }

    // MARK: - Loading

    private func pollLoop() async {
        hub.fetchAgentsIfNeeded()
        await loadSummary()
        await refresh()
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            if Task.isCancelled { break }
            await refresh()
        }
    }

    private func refreshAll() async {
        await loadSummary()
        await refresh()
        await loadArtifacts()
    }

    /// The project's own row, as the web reads it on mount (not every poll,
    /// so a flag just written isn't overwritten mid-flight).
    private func loadSummary() async {
        guard let all = try? await BotModeClient.shared.listBotProjects(),
              let row = all.first(where: { $0.id == projectId }) else { return }
        title = row.title
        details = row.description
        favorite = row.favorite
        archived = row.archived
        botIds = row.botIds
    }

    private func refresh() async {
        defer { isLoading = false }
        do {
            let loadedMessages = try await BotProjectClient.shared.messages(projectId: projectId)
            let loadedThreads = try await BotModeClient.shared.listProjectThreads(projectId: projectId)
            messages = loadedMessages
            threads = loadedThreads
            loadError = nil
        } catch is CancellationError {
        } catch {
            loadError = "Couldn't load this project. Check the connection and try again."
        }
    }

    /// What the project's threads made (ProjectArtifacts.tsx reads each
    /// thread's window); here the artifact rows made in those sessions.
    private func loadArtifacts() async {
        let sessions = sessionIds
        guard !sessions.isEmpty else {
            artifacts = []
            return
        }
        var query = ArtifactV2ListQuery()
        query.limit = 100
        guard let page = try? await ArtifactsV2Client.shared.list(query) else { return }
        artifacts = page.items.filter { $0.origin?.sessionId.map(sessions.contains) ?? false }
    }

    private func reloadLists() async {
        await store.refreshProjects()
        await ProjectsLibraryStore.shared.loadBotProjects()
    }

    // MARK: - Actions

    private func decorated(_ text: String) -> String {
        let surface = modeStore.mode
        guard agentModeStore.isAgentEnabled(for: surface),
              let mode = ChatDeckMode.forTile(agentModeStore.selectedTile(for: surface)) else { return text }
        return "[Execute in \(mode.label) mode]\n\n\(text)"
    }

    private func send() {
        let raw = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty, !sending else { return }
        let text = decorated(raw)
        sending = true
        status = nil
        draft = ""
        let pending = ProjectMessage(id: "pending-\(UUID().uuidString)", role: "user", text: text,
                                     createdAt: ISO8601DateFormatter().string(from: Date()))
        messages.append(pending)
        Task {
            defer { sending = false }
            do {
                try await BotModeClient.shared.sendToCoordinator(projectId: projectId, text: text)
                await refresh()
            } catch {
                messages.removeAll { $0.id == pending.id }
                draft = raw
                status = "Al didn't get that. Try sending it again."
            }
        }
    }

    private func toggleDictation() {
        if isDictating {
            dictation.stop()
            return
        }
        var base = draft
        if !base.isEmpty, !base.hasSuffix(" ") { base += " " }
        dictationBase = base
        isDictating = true
        Task { await dictation.start() }
    }

    /// ProjectBotLauncher + createBotThread: a new session, adopted as a
    /// server thread in this project, then its chat.
    private func startThread(_ bot: AgentRecord) {
        guard startingBotId == nil else { return }
        startingBotId = bot.id
        status = nil
        Task {
            defer { startingBotId = nil }
            do {
                let sessionId = try await store.newThreadSession(for: bot)
                // As on the web, the chat still opens if the server thread can't be made.
                try? await BotProjectClient.shared.adoptThread(botId: bot.id, title: "New thread", sessionId: sessionId, projectId: projectId)
                navigator.open(.chat(botId: bot.id, sessionId: sessionId))
                await refresh()
            } catch {
                status = "Couldn't start \(bot.botDisplayName) on a new thread. Try again."
            }
        }
    }

    private func openWaiting() {
        if waiting.count == 1, let only = waiting.first {
            navigator.openThread(only)
        } else {
            sheet = .needsYou
        }
    }

    private func reopen(_ thread: BotThread) {
        Task {
            do {
                try await BotProjectClient.shared.reopenThread(thread)
                await refresh()
            } catch {
                status = "Couldn't reopen \(thread.title). Try again."
            }
        }
    }

    private func toggleFavorite() {
        let next = !favorite
        favorite = next
        Task {
            do {
                try await BotProjectClient.shared.setFlags(projectId: projectId, favorite: next)
                await reloadLists()
            } catch {
                favorite = !next
                status = "Couldn't update the project. Try again."
            }
        }
    }

    private func toggleArchived() {
        let next = !archived
        archived = next
        Task {
            do {
                try await BotProjectClient.shared.setFlags(projectId: projectId, archived: next)
                await reloadLists()
            } catch {
                archived = !next
                status = "Couldn't update the project. Try again."
            }
        }
    }

    private func deleteProject() {
        Task {
            do {
                try await ProjectStore.shared.deleteProject(id: projectId)
                await reloadLists()
                dismiss()
            } catch {
                status = "Couldn't delete the project. Try again."
            }
        }
    }
}

extension BackdropScope {
    /// A project's own look (web appearanceKey), kept apart from the Settings default.
    static func project(_ id: String) -> BackdropScope { BackdropScope(raw: "project:\(id)") }
}
