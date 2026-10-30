import SwiftUI

// MARK: - Pane list

/// The panes a code session opens (pane-manager.store.ts `CODE_PANE_STRIP`
/// plus the session panel), in the desktop strip's order. On the phone each
/// opens as a sheet from the session header's Panes menu; Terminal opens the
/// computer's shell in place.
enum CodePaneKind: String, CaseIterable, Identifiable {
    case session, terminal, changes, browser, computer, files, artifacts, transcript, tasks, codemap

    var id: String { rawValue }

    var label: String {
        switch self {
        case .session: return "Session panel"
        case .terminal: return "Terminal"
        case .changes: return "Changes"
        case .browser: return "Browser"
        case .computer: return "Computer"
        case .files: return "Files"
        case .artifacts: return "Artifacts"
        case .transcript: return "Transcript"
        // The desktop strip's Tasks pane: the session's background shells
        // and subagents.
        case .tasks: return "Background tasks"
        case .codemap: return "Code map"
        }
    }

    var icon: String {
        switch self {
        case .session: return "sidebar.right"
        case .terminal: return "apple.terminal"
        case .changes: return "plus.forwardslash.minus"
        case .browser: return "globe"
        case .computer: return "desktopcomputer"
        case .files: return "folder"
        case .artifacts: return "square.stack"
        case .transcript: return "text.alignleft"
        case .tasks: return "gearshape.2"
        case .codemap: return "point.3.connected.trianglepath.dotted"
        }
    }
}

// MARK: - Pane sheet

/// One pane as a sheet: the existing phone views where they exist (files,
/// diff, dev-server preview, computers, artifacts), and the session panel,
/// transcript, background tasks and code map read from the same routes the
/// desktop panes use.
struct CodePaneSheet: View {
    let kind: CodePaneKind
    let sessionId: String?
    let folder: CodeFolderInfo?
    @ObservedObject var instanceStore: InstanceStore
    @ObservedObject var terminals: CodeTerminalStore
    /// The Artifacts library's stores, passed in so the sheet never depends
    /// on what its presenter's environment hands down.
    @ObservedObject var modeStore: AppModeStore
    @ObservedObject var agentModeStore: AgentModeStore
    /// The Artifacts library's ☰: close the pane, open the drawer.
    let onOpenMenu: () -> Void

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        content
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
    }

    @ViewBuilder
    private var content: some View {
        switch kind {
        case .files:
            FileBrowserView(instanceStore: instanceStore)
        case .changes:
            if let sessionId {
                SessionDiffListView(sessionId: sessionId, instanceStore: instanceStore)
            } else {
                titled { CodePaneEmpty(text: "No changes yet. Edits the session makes show up here.") }
            }
        case .browser:
            if let ptyId = terminals.livePtyId {
                DevServerPortsView(ptyID: ptyId, instanceStore: instanceStore)
            } else {
                titled { CodePaneEmpty(text: "Start a dev server in the terminal to preview it here.") }
            }
        case .computer:
            NavigationStack {
                ComputersListView()
                    .navigationDestination(for: BotRoute.self) { route in
                        if case .computer(let computer) = route { ComputerDriveView(computer: computer) }
                    }
            }
        case .artifacts:
            ArtifactsLibraryView(isSidebarOpen: Binding(get: { false }, set: { open in
                if open {
                    dismiss()
                    onOpenMenu()
                }
            }))
            .environmentObject(modeStore)
            .environmentObject(agentModeStore)
        case .transcript:
            titled { CodeTranscriptPane(sessionId: sessionId) }
        case .tasks:
            titled { CodeBackgroundTasksPane(sessionId: sessionId) }
        case .codemap:
            titled { CodeMapPane(sessionId: sessionId, folder: folder, instanceStore: instanceStore) }
        case .session:
            titled { CodeSessionPanel(sessionId: sessionId, folder: folder) }
        case .terminal:
            // Opened in place by CodeModeView; never presented as a sheet.
            EmptyView()
        }
    }

    private func titled<Content: View>(@ViewBuilder _ body: () -> Content) -> some View {
        NavigationStack {
            body()
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(Color("BgPanel"))
                .navigationTitle(kind.label)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
                }
        }
    }
}

/// A pane's empty or error line (the desktop panes' centred tertiary copy).
struct CodePaneEmpty: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 13))
            .lineSpacing(3)
            .foregroundColor(Color("TextTertiary"))
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
            .padding(.top, 48)
    }
}

// MARK: - Transcript

/// The session's raw transcript (CodeTranscriptPane): every message in
/// order, role over text, monospaced.
struct CodeTranscriptPane: View {
    let sessionId: String?

    @State private var messages: [AgentSessionMessage] = []
    @State private var loaded = false
    @State private var error: String? = nil

    var body: some View {
        Group {
            if sessionId == nil {
                CodePaneEmpty(text: "No session selected.")
            } else if !loaded {
                ProgressView().padding(.top, 48)
            } else if let error, messages.isEmpty {
                CodePaneEmpty(text: "Couldn't load the transcript: \(error)")
            } else if messages.isEmpty {
                CodePaneEmpty(text: "No messages in this session yet.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        ForEach(messages) { message in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(message.role.uppercased())
                                    .font(.system(size: 10, weight: .semibold))
                                    .tracking(0.6)
                                    .foregroundColor(Color("TextTertiary"))
                                Text(message.content)
                                    .font(.system(size: 12, design: .monospaced))
                                    .foregroundColor(Color("TextPrimary"))
                                    .textSelection(.enabled)
                            }
                        }
                    }
                    .padding(16)
                }
            }
        }
        .task(id: sessionId) {
            guard let sessionId else { return }
            do {
                messages = try await AgentChatClient().listMessages(sessionId: sessionId)
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
            loaded = true
        }
    }
}

// MARK: - Background tasks

/// The session's background shells and subagents (CodeTasksPane): status,
/// kind, description, age or run time, the output or error, and Stop on a
/// running one. Refreshes every 3s while one is running.
struct CodeBackgroundTasksPane: View {
    let sessionId: String?

    @State private var tasks: [SessionTask] = []
    @State private var loaded = false
    @State private var error: String? = nil
    @State private var expanded: Set<String> = []

    var body: some View {
        Group {
            if sessionId == nil {
                CodePaneEmpty(text: "No background tasks yet. Shells and subagents the session starts show up here.")
            } else if !loaded {
                Text("Loading tasks…")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .padding(.top, 24)
            } else if tasks.isEmpty {
                CodePaneEmpty(text: error.map { "Couldn't load tasks: \($0)" }
                              ?? "No background tasks yet. Shells and subagents the session starts show up here.")
            } else {
                List(tasks) { task in row(task) }
                    .listStyle(.plain)
                    .refreshable { await load() }
            }
        }
        .task(id: sessionId) {
            while !Task.isCancelled {
                await load()
                let running = tasks.contains(where: \.isActive)
                try? await Task.sleep(for: .seconds(running ? 3 : 15))
            }
        }
    }

    private func row(_ task: SessionTask) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                statusIcon(task.status)
                VStack(alignment: .leading, spacing: 2) {
                    Text(task.description.isEmpty ? task.kindLabel : task.description)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                    Text("\(task.kindLabel) · \(task.status) · \(duration(task))")
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 4)
                if task.isActive, let sessionId {
                    Button("Stop") {
                        Task {
                            try? await AgentChatClient().cancelBackgroundTask(sessionId: sessionId, taskId: task.id)
                            await load()
                        }
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .buttonStyle(.bordered)
                    .accessibilityLabel("Stop \(task.description)")
                }
            }
            if let text = task.error ?? task.output, !text.isEmpty {
                Text(text)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(task.error != nil ? Theme.statusError : Color("TextSecondary"))
                    .lineLimit(expanded.contains(task.id) ? nil : 4)
                    .textSelection(.enabled)
                    .onTapGesture {
                        if expanded.contains(task.id) { expanded.remove(task.id) } else { expanded.insert(task.id) }
                    }
            }
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func statusIcon(_ status: String) -> some View {
        switch status {
        case "queued", "running":
            ProgressView().controlSize(.small).frame(width: 16)
        case "completed":
            Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
        case "failed":
            Image(systemName: "xmark.circle.fill").foregroundColor(Theme.statusError)
        default:
            Image(systemName: "stop.circle").foregroundColor(Color("TextTertiary"))
        }
    }

    /// CodeTasksPane `formatDuration`: run time, live while running.
    private func duration(_ task: SessionTask) -> String {
        let end = task.time.finished ?? Date().timeIntervalSince1970 * 1000
        let seconds = max(0, Int(((end - task.time.created) / 1000).rounded()))
        if seconds < 60 { return "\(seconds)s" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes)m \(seconds % 60)s" }
        return "\(minutes / 60)h \(minutes % 60)m"
    }

    private func load() async {
        guard let sessionId else { return }
        do {
            tasks = try await AgentChatClient().sessionTasks(sessionId: sessionId)
            error = nil
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
        loaded = true
    }
}

// MARK: - Code map

/// The repo map (CodeMapPanel), as a ranked list on the phone: gizzi-code's
/// `GET v1/codemap/graph` top files by score with their importers and
/// imports, and how many files the last prompt listed.
struct CodeMapPane: View {
    let sessionId: String?
    let folder: CodeFolderInfo?
    @ObservedObject var instanceStore: InstanceStore

    private struct Graph: Decodable {
        struct File: Decodable { let path: String; let score: Double?; let importers: Int?; let imports: Int? }
        let project: Bool?
        let files: [File]?
        let total: Int?
        let promptShown: Int?
    }

    @State private var graph: Graph? = nil
    @State private var message: String? = nil

    var body: some View {
        Group {
            if let graph, let files = graph.files, !files.isEmpty {
                List {
                    Section {
                        ForEach(files, id: \.path) { file in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(file.path)
                                    .font(.system(size: 12, design: .monospaced))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                    .truncationMode(.head)
                                Text("\(file.importers ?? 0) importers · \(file.imports ?? 0) imports")
                                    .font(.system(size: 11))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                    } footer: {
                        Text("\(graph.total ?? files.count) files indexed · \(graph.promptShown ?? 0) in the last prompt's map")
                    }
                }
                .listStyle(.insetGrouped)
            } else if let message {
                CodePaneEmpty(text: message)
            } else {
                ProgressView().padding(.top, 48)
            }
        }
        .task(id: folder?.directory) { await load() }
    }

    private func load() async {
        guard let directory = folder?.directory else {
            message = "Open a project folder to see its code map."
            return
        }
        guard let baseURL = await InstanceConnection.resolveBaseURL(from: instanceStore) else {
            message = "Couldn't reach Gizzi Code."
            return
        }
        var query = "limit=200&directory=\(directory.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? directory)"
        if let sessionId { query += "&sessionID=\(sessionId.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? sessionId)" }
        do {
            let client = try await InstanceConnection.gizziClient(for: baseURL)
            let result: Graph = try await client.get(path: "v1/codemap/graph?\(query)")
            if result.project == false {
                message = "This folder isn't a project Gizzi Code maps."
            } else if result.files?.isEmpty ?? true {
                message = "No files in the code map yet."
            } else {
                graph = result
            }
        } catch {
            message = "Couldn't load the code map: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}

// MARK: - Session panel

/// The session panel (CodeSessionPanel) on the phone: Environment, Context,
/// Pull requests (the desktop Changes card), Background tasks and Connectors,
/// each from the route the desktop card reads.
struct CodeSessionPanel: View {
    let sessionId: String?
    let folder: CodeFolderInfo?

    @StateObject private var environmentStore = EnvironmentStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @AppStorage(CodePermissionMode.storageKey) private var permissionRaw = CodePermissionMode.manual.rawValue

    @State private var context: SessionContextBreakdown? = nil
    @State private var prs: [LinkedPullRequest] = []
    @State private var tasks: [SessionTask] = []
    @State private var steps: [CodeProgress.Step] = []
    @State private var outputs: [CodeOutputs.Row] = []
    @State private var showAllOutputs = false
    @State private var trees: FactoryTreesList? = nil
    @State private var treesError: String? = nil
    @State private var checkpoints: [FactoryTreeCheckpoint] = []

    private var permission: CodePermissionMode { CodePermissionMode(rawValue: permissionRaw) ?? .manual }

    /// The tree this session's folder sits in: the deepest tree path that
    /// contains it.
    private var tree: FactoryTree? {
        guard let directory = folder?.directory else { return trees?.trees.first }
        return trees?.trees
            .filter { directory == $0.path || directory.hasPrefix($0.path.hasSuffix("/") ? $0.path : $0.path + "/") }
            .max { $0.path.count < $1.path.count }
    }

    var body: some View {
        List {
            if !steps.isEmpty {
                Section {
                    ForEach(steps) { step in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            stepIcon(step.status)
                            Text(step.status == .inProgress ? (step.activeForm ?? step.content) : step.content)
                                .font(.system(size: 13, weight: step.status == .inProgress ? .semibold : .regular))
                                .foregroundColor(step.status == .completed ? Color("TextSecondary") : Color("TextPrimary"))
                                .strikethrough(step.status == .completed, color: Color("TextTertiary"))
                        }
                    }
                } header: {
                    let done = CodeProgress.done(steps)
                    Text(done == steps.count ? "Progress · All \(steps.count) done" : "Progress · Step \(min(done + 1, steps.count)) of \(steps.count)")
                }
            }
            Section("Environment") {
                row("Computer", environmentStore.environment == .cloud ? (relay.activeDevice?.name ?? "Cloud") : "This computer")
                row("Folder", folder?.directory ?? "No folder")
                if let branch = folder?.branch { row("Branch", branch) }
                row("Permissions", permission.label)
            }
            if let context {
                CodeContextRows(breakdown: context)
            }
            Section("Pull requests") {
                if prs.isEmpty {
                    Text("No pull requests yet.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                } else {
                    ForEach(prs) { pr in
                        Link(destination: URL(string: pr.url) ?? URL(string: "https://github.com")!) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(pr.title).font(.system(size: 13, weight: .medium)).lineLimit(2)
                                Text("\(pr.repo) #\(pr.number) · \(pr.state.capitalized)")
                                    .font(.system(size: 11))
                                    .foregroundColor(prTone(pr.state))
                            }
                        }
                    }
                }
            }
            Section {
                if outputs.isEmpty {
                    Text("Nothing yet. Artifacts, pages and files the session makes show up here.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                } else {
                    ForEach(showAllOutputs ? outputs : Array(outputs.prefix(8))) { output in outputRow(output) }
                    if outputs.count > 8 {
                        Button(showAllOutputs ? "Show fewer" : "Show all \(outputs.count)") { showAllOutputs.toggle() }
                            .font(.system(size: 13))
                    }
                }
            } header: {
                Text(outputs.isEmpty ? "Outputs" : "Outputs · \(outputs.count)")
            }
            Section("Background tasks") {
                let running = tasks.filter(\.isActive).count
                row("Running", "\(running)")
                row("All", "\(tasks.count)")
            }
            treeSection
            if let tools = context?.mcpTools, !tools.isEmpty {
                Section("Connectors") {
                    ForEach(Dictionary(grouping: tools, by: \.server).sorted { $0.key < $1.key }, id: \.key) { entry in
                        row(entry.key, "\(entry.value.count) tools · \(SessionContextBreakdown.format(entry.value.reduce(0) { $0 + $1.tokens }))")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .task(id: sessionId) { await load() }
        .refreshable { await load() }
    }

    /// TreeCard: the agent tree this folder sits in — branch, state, behind
    /// main, size, owner — its processes (dev servers flagged), overlaps,
    /// the machine's disk warnings, then checkpoints newest first.
    @ViewBuilder
    private var treeSection: some View {
        Section("Tree") {
            if let tree {
                row("Branch", tree.branch ?? "detached")
                row("State", tree.state.capitalized)
                if let behind = tree.behindMain { row("Behind main", "\(behind)") }
                if let dirty = tree.dirtyFiles, dirty > 0 { row("Uncommitted files", "\(dirty)") }
                if let size = tree.sizeBytes {
                    row("Size", ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file))
                }
                if let owner = tree.owner { row("Owner", owner.label) }
                ForEach(tree.processes ?? [], id: \.pid) { process in
                    row(process.devServer ? "Dev server" : "Process", process.command)
                }
                ForEach(Array((trees?.overlaps ?? []).filter { $0.a == tree.id || $0.b == tree.id }.enumerated()), id: \.offset) { _, overlap in
                    Label("Another tree changed \(overlap.files.count) of the same files", systemImage: "exclamationmark.triangle")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.statusWarning)
                }
                ForEach(checkpoints) { checkpoint in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(checkpoint.summary.isEmpty ? "Checkpoint \(checkpoint.n)" : checkpoint.summary)
                            .font(.system(size: 13))
                            .lineLimit(2)
                        Text("#\(checkpoint.n) · \(checkpoint.files) files · \(checkpoint.tool ?? "edit")")
                            .font(.system(size: 11))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            } else {
                Text(treesError.map { "Couldn't read agent trees: \($0)" } ?? "This folder isn't in an agent tree.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }
            ForEach(trees?.machine?.warnings ?? [], id: \.self) { warning in
                Label(warning, systemImage: "externaldrive.badge.exclamationmark")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.statusWarning)
            }
        }
    }

    @ViewBuilder
    private func outputRow(_ output: CodeOutputs.Row) -> some View {
        let label = HStack(spacing: 10) {
            Image(systemName: outputIcon(output.kind))
            .font(.system(size: 13))
            .foregroundColor(Color("TextSecondary"))
            .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(output.title).font(.system(size: 13, weight: .medium)).lineLimit(1)
                Text(output.subtitle)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        switch output.kind {
        case .artifact(let id, _):
            NavigationLink { V2ArtifactView(artifactId: id, initialTitle: output.title) } label: { label }
        case .page(let url):
            Link(destination: url) { label }
        case .file:
            // Written files open from the Files pane.
            label
        }
    }

    private func outputIcon(_ kind: CodeOutputs.Kind) -> String {
        switch kind {
        case .artifact: return "shippingbox"
        case .page: return "globe"
        case .file: return "doc.text"
        }
    }

    private func stepIcon(_ status: CodeProgress.Status) -> some View {
        Group {
            switch status {
            case .completed: Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
            case .inProgress: Image(systemName: "circle.dotted.circle").foregroundColor(Color("AccentPrimary"))
            case .failed: Image(systemName: "xmark.circle.fill").foregroundColor(Theme.statusError)
            case .pending: Image(systemName: "circle").foregroundColor(Color("TextTertiary"))
            }
        }
        .font(.system(size: 13))
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).font(.system(size: 13))
            Spacer(minLength: 12)
            Text(value)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    /// LinkedPR status colours: merged purple, conflict orange, open green.
    private func prTone(_ state: String) -> Color {
        switch state {
        case "merged": return Color(hex: "#8957E5")
        case "conflict": return Theme.statusWarning
        case "open": return Theme.statusSuccess
        case "closed": return Theme.statusError
        default: return Color("TextTertiary")
        }
    }

    private func load() async {
        await loadTrees()
        guard let sessionId else { return }
        let client = AgentChatClient()
        async let contextResult = try? client.sessionContext(sessionId: sessionId)
        async let prsResult = try? client.sessionPullRequests(sessionId: sessionId)
        async let tasksResult = try? client.sessionTasks(sessionId: sessionId)
        async let messagesResult = try? client.listMessages(sessionId: sessionId)
        if let value = await contextResult { context = value }
        prs = await prsResult ?? prs
        tasks = await tasksResult ?? tasks
        if let messages = await messagesResult {
            steps = CodeProgress.steps(from: messages)
            outputs = CodeOutputs.rows(from: messages)
        }
    }

    private func loadTrees() async {
        let factory = FactoryPaneClient()
        do {
            trees = try await factory.trees(path: folder?.directory)
            treesError = nil
            if let tree {
                checkpoints = (try? await factory.treeCheckpoints(id: tree.id)) ?? []
            }
        } catch let error as FactoryError where error.engineMissing {
            // Nothing serves the Factory here: no trees to show.
            trees = nil
        } catch {
            treesError = (error as? FactoryError)?.fact ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
