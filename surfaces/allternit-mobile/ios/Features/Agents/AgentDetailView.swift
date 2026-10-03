import SwiftUI

/// Agent detail (mockup A2, docs/agent-hub-options.html), pushed from the
/// Agent Hub list: identity editing (model, prompt file / system prompt),
/// the agent's workspace .md files, and a read-only behavior summary.
///
/// The live record comes from `AgentHubStore.shared` (the PUT answers only
/// `{"success": true}`, so the store re-fetches the row after each edit —
/// `initialAgent` is just the first paint).
struct AgentDetailView: View {
    let initialAgent: AgentRecord

    @StateObject private var hubStore = AgentHubStore.shared
    @StateObject private var modelStore = ModelStore.shared
    @StateObject private var botStatusStore = BotStatusStore.shared
    @StateObject private var webhookStore = WebhookTriggersStore.shared
    @StateObject private var botDesktopStore = BotDesktopStore.shared
    @EnvironmentObject private var agentModeStore: AgentModeStore

    @State private var workspaceFiles: [WorkspaceFileInfo] = []
    @State private var isLoadingFiles = false
    @State private var filesError: String? = nil
    @State private var isCreatingFile = false
    @State private var isNewFileSheetPresented = false
    /// Programmatic editor push after a file is created on this screen.
    @State private var fileToEdit: WorkspaceFileInfo? = nil
    #if DEBUG
    /// One-shot latch for the debug launch args — onAppear re-fires when
    /// the nav stack pops back here, and re-applying them would swallow
    /// the Back navigation.
    @State private var didApplyDebugArgs = false
    #endif
    @State private var isPromptEditorPresented = false
    @State private var isGreetingEditorPresented = false
    @State private var isAvatarEditorPresented = false
    @State private var isSavingModel = false
    @State private var saveError: String? = nil
    @State private var isPublishSheetPresented = false
    /// Pushed Webhook Triggers panel (Webhooks card, bots only).
    @State private var isWebhooksPresented = false
    /// Fullscreen native VNC viewer (Desktop card, bots only).
    @State private var isDesktopViewerPresented = false

    private let agentClient = AgentClient()

    /// The store's row when present (reflects edits made on this screen),
    /// falling back to the record the list pushed with.
    private var agent: AgentRecord {
        hubStore.agent(withId: initialAgent.id) ?? initialAgent
    }

    var body: some View {
        List {
            heroSection
            identitySection
            workspaceSections
            behaviorSection
            if agent.isBot {
                webhooksSection
                desktopSection
                activitySection
            }
            Section {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    isPublishSheetPresented = true
                }) {
                    Label("Publish to Marketplace", systemImage: "storefront")
                }
            } footer: {
                Text("Shares a snapshot of this agent so others can install their own copy.")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(agent.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if agent.isBot {
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        if botStatusStore.isPinned(botId: agent.id) {
                            botStatusStore.unpin()
                        } else {
                            botStatusStore.pin(botId: agent.id, displayName: agent.name)
                        }
                    }) {
                        Image(systemName: botStatusStore.isPinned(botId: agent.id) ? "pin.fill" : "pin")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("AccentPrimary"))
                    }
                    .accessibilityLabel(botStatusStore.isPinned(botId: agent.id) ? "Unpin from Dynamic Island" : "Pin to Dynamic Island")
                }
            }
        }
        .navigationDestination(item: $fileToEdit) { file in
            WorkspaceFileEditorView(agentId: agent.id, agentName: agent.name, file: file)
        }
        .navigationDestination(isPresented: $isWebhooksPresented) {
            WebhooksSettingsView()
        }
        .sheet(isPresented: $isNewFileSheetPresented) {
            NewWorkspaceFileSheet(
                agentId: agent.id,
                existingPaths: Set(workspaceFiles.map(\.path))
            ) { created in
                fileToEdit = created
            }
        }
        .sheet(isPresented: $isGreetingEditorPresented) {
            GreetingEditorSheet(agent: agent)
        }
        .sheet(isPresented: $isPromptEditorPresented) {
            SystemPromptEditorSheet(agentId: agent.id, initialPrompt: agent.systemPrompt ?? "")
        }
        .sheet(isPresented: $isAvatarEditorPresented) {
            AgentAvatarEditorSheet(agent: agent)
        }
        .sheet(isPresented: $isPublishSheetPresented) {
            PublishAgentSheet(sourceAgent: agent)
        }
        .fullScreenCover(isPresented: $isDesktopViewerPresented) {
            // The Desktop card only offers "View desktop" while a VNC
            // stream + wsURL exist, so the status is present here.
            if let status = botDesktopStore.entry(for: agent.id)?.status, status.wsURL != nil {
                BotDesktopView(agent: agent, status: status)
            }
        }
        .alert("Couldn't save changes", isPresented: Binding(
            get: { saveError != nil },
            set: { if !$0 { saveError = nil } }
        )) {
            Button("OK") { saveError = nil }
        } message: {
            Text(saveError ?? "")
        }
        .task {
            modelStore.fetchModelsIfNeeded()
            if agent.isBot {
                // The Webhooks card's count; deduped once per launch.
                webhookStore.fetchTriggersIfNeeded()
                // The Desktop card's status; deduped once per launch.
                botDesktopStore.fetchStatusIfNeeded(botId: agent.id)
            }
        }
        .onAppear {
            // Re-listed on every appear so sizes stay truthful after the
            // file editor saves (the editor PUTs directly).
            Task { await loadWorkspaceFiles() }
            #if DEBUG
            // One-shot (see didApplyDebugArgs) — otherwise popping back
            // from the pushed screen re-fires these and swallows Back.
            if !didApplyDebugArgs {
                // `-open-avatar-editor` (DEBUG only): open the avatar
                // editor on appear for screenshot verification.
                if launchArgumentEnabled("open-avatar-editor") {
                    isAvatarEditorPresented = true
                }
                // `-open-new-workspace-file` (DEBUG only): open the
                // new-file sheet on appear for screenshot verification.
                if launchArgumentEnabled("open-new-workspace-file") {
                    isNewFileSheetPresented = true
                }
                // `-open-workspace-file <path>` (DEBUG only): open a
                // workspace file straight in the editor for screenshots.
                if let raw = UserDefaults.standard.string(forKey: "open-workspace-file") {
                    fileToEdit = WorkspaceFileInfo(path: raw, sizeBytes: 0, modifiedAt: nil)
                }
                didApplyDebugArgs = true
            }
            #endif
        }
    }

    // MARK: - Hero

    private var heroSection: some View {
        Section {
            VStack(spacing: 8) {
                // The avatar is the agent's ID card — tap to customize it
                // (emoji stored on the backend `avatar` column, same as the
                // web hub).
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    isAvatarEditorPresented = true
                }) {
                    ZStack(alignment: .bottomTrailing) {
                        AgentAvatarView(agent: agent, size: 62)
                        Image(systemName: "pencil.circle.fill")
                            .font(.system(size: 18))
                            .foregroundColor(Color("AccentPrimary"))
                            .background(Color("BgPrimary").clipShape(Circle()))
                            .offset(x: 2, y: 2)
                    }
                }
                .buttonStyle(.plain)
                Text(agent.name)
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                Text(heroSubtitle)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .listRowBackground(Color.clear)
        }
    }

    /// "Created 2026-07-12" — created_at is the backend's SQLite timestamp
    /// string, shown date-only like the mockup's hero subtitle.
    private var heroSubtitle: String {
        var parts: [String] = []
        if agent.mode != "primary" {
            parts.append(agent.mode.capitalized)
        }
        if !agent.createdAt.isEmpty {
            parts.append("Created \(String(agent.createdAt.prefix(10)))")
        }
        return parts.joined(separator: " · ")
    }

    // MARK: - Identity

    private var identitySection: some View {
        Section {
            modelRow
            responseStyleRow
            promptFileRows
            // When a prompt file carries the instructions, the empty DB
            // field stays hidden — the file is the editing surface (the
            // row returns the moment a DB prompt exists: both are shown).
            if promptFiles.isEmpty || agent.systemPrompt?.isEmpty == false {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    isPromptEditorPresented = true
                }) {
                    HStack {
                        Text("System prompt")
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Text(agent.systemPrompt?.isEmpty == false ? "Edit" : "Add")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }

            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isGreetingEditorPresented = true
            }) {
                HStack {
                    Text("Greeting")
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(agent.greeting?.isEmpty == false ? "Edit" : "Add")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        } header: {
            Text("Identity")
        } footer: {
            if !promptFiles.isEmpty && agent.systemPrompt?.isEmpty == false {
                Text("Both prompt sources are active: the platform layers the persona and instruction files with the system prompt — neither overrides the other.")
            }
        }
    }

    /// Workspace files the platform composer layers into the agent's
    /// instructions (v1_routes.rs compose_system_instructions), split by
    /// kind. Persona first: SOUL.md → STYLE.md shape identity/voice. Then
    /// the instruction files, in canonical order: AGENTS.md first, then the
    /// tool-specific variants (gizzi-code's context pack reads AGENTS.md →
    /// GIZZI.md → .claude/CLAUDE.md → SYSTEM_LAW.md, pack.ts:28; root
    /// CLAUDE.md is the Claude-tool convention). There is NO backend
    /// precedence rule between a prompt file and the DB system_prompt —
    /// both are injected — so the files are simply surfaced first.
    private static let personaFileCandidates = ["SOUL.md", "STYLE.md"]
    private static let instructionFileCandidates = [
        "AGENTS.md", "GIZZI.md", "CLAUDE.md", ".claude/CLAUDE.md", "SYSTEM_LAW.md",
    ]

    /// The candidate files of each kind present in this agent's workspace,
    /// canonical order preserved.
    private var personaFiles: [WorkspaceFileInfo] {
        Self.personaFileCandidates.compactMap { candidate in
            workspaceFiles.first { $0.path == candidate }
        }
    }

    private var instructionFiles: [WorkspaceFileInfo] {
        Self.instructionFileCandidates.compactMap { candidate in
            workspaceFiles.first { $0.path == candidate }
        }
    }

    /// All surfaced prompt files: persona files first, then instruction
    /// files.
    private var promptFiles: [WorkspaceFileInfo] {
        personaFiles + instructionFiles
    }

    /// One row per existing prompt file (opens the file editor) — persona
    /// files labeled "Persona file" first, then instruction files labeled
    /// "Prompt file" — or, once the file list has loaded and no instruction
    /// file exists, an Add row that creates the canonical AGENTS.md and
    /// opens it (the PUT creates the file — agent_workspace_routes.rs).
    @ViewBuilder
    private var promptFileRows: some View {
        ForEach(personaFiles) { file in
            promptFileRow(file, label: "Persona file")
        }
        ForEach(instructionFiles) { file in
            promptFileRow(file, label: "Prompt file")
        }
        if instructionFiles.isEmpty && !isLoadingFiles && filesError == nil {
            Button(action: {
                Task { await createKnownDoc("AGENTS.md", in: AgentWorkspaceLayout.categories[0]) }
            }) {
                HStack {
                    Text("Prompt file")
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if isCreatingFile {
                        ProgressView()
                    } else {
                        Text("Add AGENTS.md")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .disabled(isCreatingFile)
        }
    }

    /// A single prompt-file row: fixed label left, workspace path right,
    /// tap opens the file editor.
    private func promptFileRow(_ file: WorkspaceFileInfo, label: String) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            fileToEdit = file
        }) {
            HStack {
                Text(label)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(file.path)
                    .font(.system(.subheadline, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    /// Model picker fed by the runtime catalog (ModelStore, same source as
    /// the composer model pill); a pick PUTs `{model, provider}`. Without a
    /// loaded catalog the row degrades to the stored value, read-only.
    @ViewBuilder
    private var modelRow: some View {
        if modelStore.models.isEmpty {
            HStack {
                Text("Model")
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(agent.model.isEmpty ? "Default" : agent.model)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        } else {
            Menu {
                ForEach(modelStore.models) { model in
                    Button(action: { updateModel(model) }) {
                        if model.provider == agent.provider && model.shortName == agent.model {
                            Label("\(model.provider)/\(model.shortName)", systemImage: "checkmark")
                        } else {
                            Text("\(model.provider)/\(model.shortName)")
                        }
                    }
                }
            } label: {
                HStack {
                    Text("Model")
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if isSavingModel {
                        ProgressView()
                    } else {
                        Text(agent.model.isEmpty ? "Default" : agent.model)
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
        }
    }

    /// Response-style picker — the same user-level preference as the
    /// Settings Agent section (one row, `GET/PUT /api/v1/agent-preferences`;
    /// response style is per-user, not per-agent, so both controls edit the
    /// same value). Saves optimistically via PreferencesStore; the PUT also
    /// syncs this agent's managed STYLE.md (agent_preferences_routes.rs).
    private var responseStyleRow: some View {
        Picker("Response style", selection: responseStyleBinding) {
            ForEach(ResponseStyle.allCases, id: \.self) { style in
                Text(style.label).tag(style)
            }
        }
        .foregroundColor(Color("TextPrimary"))
    }

    private var responseStyleBinding: Binding<ResponseStyle> {
        Binding(
            get: { PreferencesStore.shared.responseStyle },
            set: {
                PreferencesStore.shared.save(
                    style: $0,
                    instructions: PreferencesStore.shared.customInstructions
                )
            }
        )
    }

    // MARK: - Workspace files

    /// One presentation group per layout category: the files on disk under
    /// that prefix, then the category's platform docs NOT yet created
    /// (tappable ghosts). Categories always render — discovering the
    /// layout is half the point.
    private struct CategoryGroup: Identifiable {
        let category: AgentWorkspaceCategory
        var files: [WorkspaceFileInfo]
        var missingDocs: [String]
        var id: String { category.id }
    }

    private var categoryGroups: [CategoryGroup] {
        AgentWorkspaceLayout.categories.map { category in
            let prefix = category.directory.isEmpty ? "" : category.directory + "/"
            let files = workspaceFiles.filter { info in
                category.directory.isEmpty
                    ? !info.path.contains("/")
                    : info.path.hasPrefix(prefix)
            }
            let missing = category.knownDocs.filter { doc in
                !files.contains { $0.path == prefix + doc }
            }
            return CategoryGroup(category: category, files: files, missingDocs: missing)
        }
    }

    @ViewBuilder
    private var workspaceSections: some View {
        if isLoadingFiles && workspaceFiles.isEmpty {
            Section {
                HStack {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
            } header: {
                Text("Workspace files")
            }
        } else if let filesError, workspaceFiles.isEmpty {
            Section {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load workspace files",
                    message: FriendlyErrorMessage.from(filesError),
                    actionTitle: "Retry",
                    action: { Task { await loadWorkspaceFiles() } }
                )
            } header: {
                Text("Workspace files")
            }
        } else {
            // ONE section — the categories are sub-blocks inside it, not
            // seven gapped sections. The label + guidance keep explaining
            // what each group is for; the rows just stop drifting apart.
            Section {
                ForEach(Array(categoryGroups.enumerated()), id: \.element.id) { index, group in
                    // Category sub-block header.
                    VStack(alignment: .leading, spacing: 1) {
                        Text(group.category.label.uppercased())
                            .font(.system(size: 10, weight: .bold))
                            .tracking(0.8)
                            .foregroundColor(Color("TextSecondary"))
                        Text(group.category.guidance)
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary").opacity(0.8))
                    }
                    .padding(.top, index == 0 ? 0 : 10)
                    .padding(.bottom, 2)
                    .listRowSeparator(.hidden)

                    ForEach(group.files) { file in
                        // All editor navigation goes through the item
                        // binding (fileToEdit) — mixing NavigationLink rows
                        // with navigationDestination(item:) in one stack
                        // breaks the Back button (the nil item binding
                        // fights the link-pushed view).
                        Button(action: {
                            let generator = UIImpactFeedbackGenerator(style: .light)
                            generator.impactOccurred()
                            fileToEdit = file
                        }) {
                            HStack {
                                Text(file.path.split(separator: "/").last.map(String.init) ?? file.path)
                                    .font(.system(.subheadline, design: .monospaced))
                                    .foregroundColor(Color("TextPrimary"))
                                Spacer()
                                Text(file.sizeLabel)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    // Platform docs not yet created — one tap creates them
                    // with starter content and opens the editor.
                    ForEach(group.missingDocs, id: \.self) { doc in
                        Button(action: {
                            Task { await createKnownDoc(doc, in: group.category) }
                        }) {
                            HStack(spacing: 8) {
                                Image(systemName: "plus.circle")
                                    .font(.subheadline)
                                    .foregroundColor(Color("AccentPrimary"))
                                Text(doc)
                                    .font(.system(.subheadline, design: .monospaced))
                                    .foregroundColor(Color("TextSecondary"))
                                Spacer()
                                if isCreatingFile {
                                    ProgressView()
                                } else {
                                    Text("Create")
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(isCreatingFile)
                    }
                }

                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    isNewFileSheetPresented = true
                }) {
                    HStack(spacing: 10) {
                        Image(systemName: "doc.badge.plus")
                            .font(.subheadline)
                            .frame(width: 20)
                        Text("New context file…")
                            .font(.subheadline)
                        Spacer()
                    }
                    .foregroundColor(Color("AccentPrimary"))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.top, 6)
            } header: {
                Text("Workspace files")
            }
        }
    }

    /// Creates one of the category's platform docs with starter content,
    /// then opens it in the editor (the PUT creates parent directories —
    /// agent_workspace_routes.rs).
    private func createKnownDoc(_ doc: String, in category: AgentWorkspaceCategory) async {
        isCreatingFile = true
        let path = category.path(for: doc)
        let content = AgentWorkspaceLayout.starterContent(for: path)
        do {
            try await agentClient.writeWorkspaceFile(agentId: agent.id, path: path, content: content)
            await loadWorkspaceFiles()
            fileToEdit = WorkspaceFileInfo(
                path: path,
                sizeBytes: content.utf8.count,
                modifiedAt: nil
            )
        } catch {
            saveError = "Couldn't create \(doc): \(error.localizedDescription)"
        }
        isCreatingFile = false
    }

    // MARK: - Behavior (read-only)

    /// Display-only summary of the backend's behavior columns — no invented
    /// toggles (the mockup's switches have no backend wiring yet).
    private var behaviorSection: some View {
        Section {
            behaviorRow("Type", value: agent.type.capitalized)
            behaviorRow("Trust tier", value: agent.trustTier.capitalized)
            behaviorRow("Surfaces", value: agent.enabledModes.map { $0.capitalized }.joined(separator: ", "))
        } header: {
            Text("Behavior")
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
        }
    }

    // MARK: - Webhooks (bots, read-only)

    /// Read-only card for the triggers that wake this bot
    /// (`target_bot_id == agent.id`), mirroring the web bot home's
    /// `WebhooksCard` (surfaces/ai.allternit.com/src/views/bots/
    /// BotHomeView.tsx): "N triggers wake this bot", tap pushes the full
    /// Webhooks settings panel (management lives there, not here).
    private var webhooksSection: some View {
        Section {
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isWebhooksPresented = true
            }) {
                HStack {
                    Text("Webhook triggers")
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(webhooksSummary)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                        .multilineTextAlignment(.trailing)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        } header: {
            Text("Webhooks")
        }
    }

    /// "N triggers wake this bot (M active)" — copy parity with the web
    /// card, with the active subset called out when some are paused.
    private var webhooksSummary: String {
        let triggers = webhookStore.triggers(forBotId: agent.id)
        if webhookStore.isLoading && triggers.isEmpty { return "Loading…" }
        if triggers.isEmpty { return "No triggers wake this bot yet" }
        let activeCount = triggers.filter(\.active).count
        let base = "\(triggers.count) trigger\(triggers.count == 1 ? "" : "s") wake this bot"
        return activeCount == triggers.count ? base : "\(base) (\(activeCount) active)"
    }

    // MARK: - Desktop (bots)

    /// Status + control of the bot's persistent virtual computer (`/api/v1/
    /// bots/:id/desktop`, bot_desktop_routes.rs) — the iOS counterpart of
    /// the web's BotDesktopView (surfaces/ai.allternit.com/src/views/bots/
    /// BotDesktopView.tsx): provision when off, observe / take over / hand
    /// back when running, and — when the server offers a VNC stream the
    /// human may watch — a "View desktop" button presenting the native RFB
    /// viewer (Features/Agents/Desktop/BotDesktopView.swift).
    private var desktopSection: some View {
        Section {
            desktopContent
        } header: {
            Text("Desktop")
        }
    }

    @ViewBuilder
    private var desktopContent: some View {
        let entry = botDesktopStore.entry(for: agent.id)
        if let status = entry?.status {
            behaviorRow("Status", value: desktopStatusLabel(status.status))
            if status.status == .running {
                behaviorRow("Control", value: status.controlState.label)
                if !status.sandboxId.isEmpty {
                    HStack {
                        Text("Sandbox")
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Text(truncatedSandboxId(status.sandboxId))
                            .font(.system(.subheadline, design: .monospaced))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                desktopControlButtons(status: status)
                if status.wsURL != nil,
                   status.streamProtocol == .vnc,
                   status.controlState == .humanObserving || status.controlState == .humanControls {
                    // Native VNC viewer (the server only accepts the socket
                    // in observe/control states, bot_desktop_stream.rs:74-81).
                    desktopActionRow(
                        title: "View desktop",
                        systemImage: "display",
                        isDisabled: false,
                        showsSpinner: false
                    ) {
                        isDesktopViewerPresented = true
                    }
                } else if status.wsURL != nil {
                    Text("Live desktop viewing isn't available for this stream — use the web app to watch or drive it.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                // Off (or never provisioned): the only move is to provision.
                // The route is idempotent server-side, so re-tapping after a
                // previous provision returns the existing sandbox.
                desktopActionRow(
                    title: "Provision virtual computer",
                    systemImage: "desktopcomputer",
                    isDisabled: entry?.isProvisioning == true,
                    showsSpinner: entry?.isProvisioning == true
                ) {
                    await botDesktopStore.provision(botId: agent.id)
                }
            }
            if let error = entry?.error {
                // Server's own message (e.g. the 503 "No VM driver is
                // configured on this host").
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Desktop unavailable",
                    message: error
                )
            }
        } else if let error = entry?.error {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load desktop status",
                message: FriendlyErrorMessage.from(error),
                actionTitle: "Retry",
                action: { botDesktopStore.fetchStatusIfNeeded(botId: agent.id, force: true) }
            )
        } else {
            HStack {
                Spacer()
                ProgressView()
                Spacer()
            }
        }
    }

    /// Observe / take-over / hand-back rows reflecting the server's control
    /// state (web parity, BotDesktopView.tsx:266-301): while the human
    /// drives only "Hand back" is offered; otherwise Observe (disabled once
    /// observing) + Take over.
    @ViewBuilder
    private func desktopControlButtons(status: BotDesktopStatus) -> some View {
        let entry = botDesktopStore.entry(for: agent.id)
        let inFlight = entry?.isControlActionInFlight == true
        switch status.controlState {
        case .humanControls:
            desktopActionRow(
                title: "Hand back to bot",
                systemImage: "hand.raised",
                isDisabled: inFlight,
                showsSpinner: inFlight
            ) {
                await botDesktopStore.handBack(botId: agent.id)
            }
        default:
            desktopActionRow(
                title: status.controlState == .humanObserving ? "Observing" : "Observe",
                systemImage: "eye",
                isDisabled: inFlight || status.controlState == .humanObserving,
                showsSpinner: inFlight
            ) {
                await botDesktopStore.observe(botId: agent.id)
            }
            desktopActionRow(
                title: "Take over",
                systemImage: "play.fill",
                isDisabled: inFlight,
                showsSpinner: false
            ) {
                await botDesktopStore.takeOver(botId: agent.id)
            }
        }
    }

    /// One tappable action row (provision / control handoff), styled like
    /// the workspace section's "New context file…" row.
    private func desktopActionRow(title: String, systemImage: String,
                                  isDisabled: Bool, showsSpinner: Bool,
                                  action: @escaping () async -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            Task { await action() }
        }) {
            HStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.subheadline)
                    .frame(width: 20)
                Text(title)
                    .font(.subheadline)
                Spacer()
                if showsSpinner {
                    ProgressView()
                }
            }
            .foregroundColor(Color("AccentPrimary"))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(isDisabled)
    }

    private func desktopStatusLabel(_ status: BotDesktopStatus.Status) -> String {
        switch status {
        case .running: return "Running"
        case .off: return "Off"
        case .unknown: return "Unknown"
        }
    }

    /// "abcd1234efgh…" — sandbox ids are driver-native and long; the card
    /// shows the head only (the web shows the full id, BotDesktopView.
    /// tsx:388-390, but it has the width).
    private func truncatedSandboxId(_ id: String) -> String {
        id.count > 12 ? "\(id.prefix(12))…" : id
    }

    // MARK: - Activity (bots)

    /// Live per-bot activity feed — BotStatusStore's unified tail: server
    /// ledger history (`GET /api/v1/bots/:id/events`,
    /// bot_event_routes.rs) seeded on subscribe, then live SSE-folded
    /// ledger events (`GET /api/v1/agents/:id/events`,
    /// agent_routes.rs:84-138) appended. Newest first. The section
    /// subscribes while the detail is on screen and unsubscribes when it
    /// leaves.
    private var activitySection: some View {
        Section {
            if let entry = botStatusStore.entry(for: agent.id), !entry.recentEvents.isEmpty {
                ForEach(Array(entry.recentEvents.reversed().enumerated()), id: \.offset) { _, item in
                    activityRow(item)
                }
            } else {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "clock",
                    title: "No recent activity",
                    message: ""
                )
            }
        } header: {
            Text("Activity")
        }
        .onAppear { botStatusStore.subscribe(botId: agent.id) }
        .onDisappear { botStatusStore.unsubscribe(botId: agent.id) }
    }

    /// One feed row: label + relative timestamp, same styling for both
    /// server-history rows (goal/task events) and live run events.
    private func activityRow(_ item: BotStatusStore.FeedItem) -> some View {
        HStack {
            Text(item.label)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            if let timestamp = item.timestamp {
                Text(timestamp, style: .relative)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    // MARK: - Actions

    private func updateModel(_ runtimeModel: RuntimeModel) {
        isSavingModel = true
        Task {
            do {
                try await hubStore.updateAgent(
                    id: agent.id,
                    model: runtimeModel.shortName,
                    provider: runtimeModel.provider
                )
                agentModeStore.fetchAgentsIfNeeded(force: true)
            } catch {
                saveError = error.localizedDescription
            }
            isSavingModel = false
        }
    }

    /// `GET /agents/:id/workspace/files`; errors surface as the section's
    /// retry row only when there's nothing cached to show.
    private func loadWorkspaceFiles() async {
        isLoadingFiles = true
        filesError = nil
        do {
            workspaceFiles = try await agentClient.listWorkspaceFiles(agentId: agent.id)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            filesError = error.localizedDescription
        }
        isLoadingFiles = false
    }
}

/// System-prompt editor sheet (Identity section) — a plain TextEditor whose
/// Save PUTs `system_prompt` through the hub store (which re-fetches the
/// row so the detail reflects the saved value).
struct SystemPromptEditorSheet: View {
    let agentId: String
    let initialPrompt: String

    @StateObject private var hubStore = AgentHubStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var prompt: String
    @State private var isSaving = false
    @State private var saveError: String? = nil

    init(agentId: String, initialPrompt: String) {
        self.agentId = agentId
        self.initialPrompt = initialPrompt
        _prompt = State(initialValue: initialPrompt)
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                TextEditor(text: $prompt)
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .scrollContentBackground(.hidden)
                    .padding(12)
                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)
                }
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("System Prompt")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: save) {
                        if isSaving {
                            ProgressView()
                        } else {
                            Text("Save")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(isSaving)
                }
            }
        }
    }

    private func save() {
        isSaving = true
        saveError = nil
        Task {
            do {
                try await hubStore.updateAgent(id: agentId, systemPrompt: prompt)
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}

// MARK: - Greeting editor

/// Greeting + suggested-prompts editor (Identity section) — PocketPal-Pal-
/// inspired first-message quick-start, stored in the free-form `config` JSON
/// column (`greeting`/`suggested_prompts` keys; no dedicated backend
/// schema). Save sends the FULL merged config
/// (`AgentRecord.configReplacing`) since the backend replaces `config`
/// wholesale rather than merging it.
struct GreetingEditorSheet: View {
    let agent: AgentRecord

    @StateObject private var hubStore = AgentHubStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var greeting: String
    @State private var prompts: [String]
    @State private var isSaving = false
    @State private var saveError: String? = nil

    init(agent: AgentRecord) {
        self.agent = agent
        _greeting = State(initialValue: agent.greeting ?? "")
        // Always keep one trailing blank row to add a new prompt into.
        _prompts = State(initialValue: agent.suggestedPrompts + [""])
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextEditor(text: $greeting)
                        .frame(minHeight: 80)
                } header: {
                    Text("Greeting")
                } footer: {
                    Text("Shown at the top of a fresh chat when this agent is selected.")
                }

                Section {
                    ForEach(prompts.indices, id: \.self) { index in
                        TextField("Suggested prompt", text: promptBinding(index))
                    }
                    .onDelete { offsets in
                        prompts.remove(atOffsets: offsets)
                        ensureTrailingBlankRow()
                    }
                } header: {
                    Text("Suggested prompts")
                } footer: {
                    Text("Quick-start chips shown next to the greeting. Leave blank rows empty — they're dropped on save.")
                }

                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }
            }
            .navigationTitle("Greeting")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: save) {
                        if isSaving {
                            ProgressView()
                        } else {
                            Text("Save")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(isSaving)
                }
            }
        }
    }

    private func promptBinding(_ index: Int) -> Binding<String> {
        Binding(
            get: { prompts[index] },
            set: { newValue in
                prompts[index] = newValue
                ensureTrailingBlankRow()
            }
        )
    }

    /// Keeps exactly one empty row at the end so there's always somewhere to
    /// type the next prompt, without letting blank rows pile up in the middle.
    private func ensureTrailingBlankRow() {
        prompts.removeAll { $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        prompts.append("")
    }

    private func save() {
        isSaving = true
        saveError = nil
        Task {
            do {
                let config = agent.configReplacing(greeting: greeting, suggestedPrompts: prompts)
                try await hubStore.updateAgent(id: agent.id, config: config)
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}

// MARK: - Avatar editor

/// Avatar editor sheet (hero tap): the agent's ID-card logo. Three ways to
/// customize — shuffle GENERATED variants of the agent's pixel-mark
/// (stored as "grid:<seed>" in the backend `avatar` column), type any
/// glyph of your own, or Reset to the deterministic default. Save PUTs
/// through the hub store, which re-fetches the row so every surface (hub,
/// deck sheet, detail hero) repaints.
struct AgentAvatarEditorSheet: View {
    let agent: AgentRecord

    @StateObject private var hubStore = AgentHubStore.shared
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @Environment(\.dismiss) private var dismiss

    @State private var draft: String
    /// The current batch of shuffle candidates (regenerated on Shuffle).
    @State private var variants: [String] = []
    @State private var isSaving = false
    @State private var saveError: String? = nil

    init(agent: AgentRecord) {
        self.agent = agent
        _draft = State(initialValue: agent.avatar ?? "")
    }

    /// The tile preview: the draft as the avatar when non-empty, the
    /// deterministic default otherwise (what Reset produces).
    private var previewAgent: AgentRecord {
        AgentRecord(
            id: agent.id, name: agent.name, description: agent.description,
            type: agent.type, parentAgentId: agent.parentAgentId,
            model: agent.model, provider: agent.provider,
            systemPrompt: agent.systemPrompt, status: agent.status,
            workspaceId: agent.workspaceId,
            avatar: draft.trimmingCharacters(in: .whitespacesAndNewlines),
            trustTier: agent.trustTier, enabledModes: agent.enabledModes,
            category: agent.category, mode: agent.mode,
            isPrimary: agent.isPrimary, createdAt: agent.createdAt,
            updatedAt: agent.updatedAt, lastRunAt: agent.lastRunAt
        )
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 20) {
                    AgentAvatarView(agent: previewAgent, size: 72)
                        .padding(.top, 8)

                    // Generated variants: the deterministic default first,
                    // then this batch of shuffles. Selection ring = draft.
                    VStack(alignment: .leading, spacing: 10) {
                        HStack {
                            Text("GENERATED MARKS")
                                .font(.system(size: 11, weight: .bold))
                                .tracking(1)
                                .foregroundColor(Color("TextSecondary"))
                            Spacer()
                            Button(action: shuffle) {
                                Label("Shuffle", systemImage: "arrow.triangle.2.circlepath")
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundColor(Color("AccentPrimary"))
                            }
                            .buttonStyle(.plain)
                        }

                        LazyVGrid(
                            columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: 5),
                            spacing: 10
                        ) {
                            variantTile(seed: agent.id, storedAs: "")
                            ForEach(variants, id: \.self) { seed in
                                variantTile(seed: seed, storedAs: "grid:\(seed)")
                            }
                        }
                    }
                    .padding(.horizontal, 20)

                    // Free input — any emoji or short glyph; hard-capped so
                    // the tile never turns into a sentence.
                    TextField("Or type any emoji or character", text: $draft)
                        .font(.title3)
                        .multilineTextAlignment(.center)
                        .padding(.vertical, 10)
                        .padding(.horizontal, 20)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                        .onChange(of: draft) { _, newValue in
                            if newValue.count > 4, !newValue.hasPrefix("grid:") {
                                draft = String(newValue.prefix(4))
                            }
                        }
                        .padding(.horizontal, 20)

                    Button(action: { draft = "" }) {
                        Text("Reset to default mark")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    if let saveError {
                        Text(saveError)
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                            .padding(.horizontal, 20)
                    }
                }
                .padding(.bottom, 24)
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Agent Avatar")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: save) {
                        if isSaving {
                            ProgressView()
                        } else {
                            Text("Save")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(isSaving)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear { shuffle() }
    }

    /// One selectable mark tile. `storedAs` is what the draft becomes when
    /// picked ("" for the deterministic default, "grid:<seed>" for
    /// shuffles).
    private func variantTile(seed: String, storedAs: String) -> some View {
        let isSelected = draft.trimmingCharacters(in: .whitespacesAndNewlines) == storedAs
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            draft = storedAs
        }) {
            ZStack {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(Color(red: 0.93, green: 0.90, blue: 0.84))
                AgentIdenticonView(seed: seed, size: 28)
            }
            .frame(height: 48)
            .frame(maxWidth: .infinity)
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(isSelected ? Color("AccentPrimary") : Theme.borderWarmDefault,
                            lineWidth: isSelected ? 1.5 : 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// A fresh batch of shuffle candidates — short hex seeds; the mark is
    /// fully determined by the seed, so what you see is what saves.
    private func shuffle() {
        variants = (0..<4).map { _ in
            UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(6).lowercased()
        }
    }

    private func save() {
        isSaving = true
        saveError = nil
        let avatar = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            do {
                // Empty string clears the column (COALESCE treats "" as a
                // value) — the deterministic default mark returns.
                try await hubStore.updateAgent(id: agent.id, avatar: avatar)
                agentModeStore.fetchAgentsIfNeeded(force: true)
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}

// MARK: - New context file sheet

/// Guided context-file creation ("New context file…" row): pick a category
/// (the platform's workspace layout), name the document, see the exact
/// workspace path before anything is written. Create PUTs starter content
/// (parent directories are created server-side), then hands the file back
/// so the detail pushes the editor. Existing paths are rejected rather
/// than silently overwritten — overwrite lives in the file editor.
private struct NewWorkspaceFileSheet: View {
    let agentId: String
    let existingPaths: Set<String>
    let onCreated: (WorkspaceFileInfo) -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var category: AgentWorkspaceCategory = AgentWorkspaceLayout.categories[0]
    @State private var name = ""
    @State private var isCreating = false
    @State private var createError: String? = nil

    private let agentClient = AgentClient()

    /// Sanitized document filename: uppercased, spaces to dashes, .md
    /// suffix — the platform's doc-name convention.
    private var fileName: String {
        let base = name.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: " ", with: "-")
        guard !base.isEmpty else { return "" }
        let upper = base.uppercased()
        return upper.hasSuffix(".MD") ? upper : upper + ".MD"
    }

    private var fullPath: String {
        fileName.isEmpty ? "" : category.path(for: fileName)
    }

    private var isDuplicate: Bool {
        !fullPath.isEmpty && existingPaths.contains(fullPath)
    }

    private var canCreate: Bool {
        !fileName.isEmpty && !isDuplicate && !isCreating
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Picker("Category", selection: $category) {
                        ForEach(AgentWorkspaceLayout.categories) { entry in
                            Text(entry.label).tag(entry)
                        }
                    }
                    Text(category.guidance)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                } header: {
                    Text("Category")
                }

                Section {
                    TextField("e.g. Launch Notes", text: $name)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                } header: {
                    Text("Document")
                } footer: {
                    if isDuplicate {
                        Text("A file at this path already exists — open it from the list instead.")
                    } else if !fullPath.isEmpty {
                        Text(fullPath)
                            .font(.system(.caption, design: .monospaced))
                    } else {
                        Text("Saved as an uppercase .md name inside the category.")
                    }
                }

                if let createError {
                    Section {
                        Text(createError)
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("New Context File")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: create) {
                        if isCreating {
                            ProgressView()
                        } else {
                            Text("Create")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(!canCreate)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func create() {
        isCreating = true
        createError = nil
        let path = fullPath
        let content = AgentWorkspaceLayout.starterContent(for: path)
        Task {
            do {
                try await agentClient.writeWorkspaceFile(agentId: agentId, path: path, content: content)
                dismiss()
                onCreated(WorkspaceFileInfo(path: path, sizeBytes: content.utf8.count, modifiedAt: nil))
            } catch {
                createError = "Couldn't create the file: \(error.localizedDescription)"
            }
            isCreating = false
        }
    }
}
