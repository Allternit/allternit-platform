import SwiftUI
import UniformTypeIdentifiers

// The cards of the project page (BaseProjectView's ProjectSection, the
// asideSections of ProjectHomeView) and the coordinator conversation
// (components/threads/coordinator-parts.tsx, DecisionCard.tsx). Every card
// handles its own loading, error and empty state.

// MARK: - Chrome

/// One card: title, the right-hand control, the body.
struct ProjectCard<Right: View, Content: View>: View {
    let title: String
    @ViewBuilder let right: () -> Right
    @ViewBuilder let content: () -> Content

    init(title: String, @ViewBuilder right: @escaping () -> Right, @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.right = right
        self.content = content
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 8)
                right()
            }
            .frame(minHeight: 44)
            content()
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

extension ProjectCard where Right == EmptyView {
    init(title: String, @ViewBuilder content: @escaping () -> Content) {
        self.init(title: title, right: { EmptyView() }, content: content)
    }
}

struct ProjectCardNote: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(Color("TextTertiary"))
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// A text action with a 44 pt target.
struct ProjectTextButton: View {
    let title: String
    let action: () -> Void
    init(_ title: String, action: @escaping () -> Void) {
        self.title = title
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

struct ProjectIconButton: View {
    let systemName: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 16))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

struct ProjectInlineError: View {
    let text: String
    let retry: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(text)
                .font(.system(size: 12))
                .foregroundColor(Theme.statusError)
                .fixedSize(horizontal: false, vertical: true)
            Button("Retry", action: retry)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
                .buttonStyle(.plain)
        }
    }
}

// MARK: - Team

struct ProjectTeamCard: View {
    let context: ProjectPaneContext
    let team: [AgentRecord]
    let threads: [BotThread]
    let onOpenThread: (BotThread) -> Void
    let onOpenPane: (ProjectPaneKind) -> Void
    let onOpenNode: (FactoryNodeRef) -> Void

    @EnvironmentObject private var navigator: BotNavigator
    @State private var graphMode = true
    @State private var agents: [FactoryAgent] = []
    @State private var factoryChecked = false
    @State private var factoryFailed = false

    private var graphReady: Bool { !agents.isEmpty }

    var body: some View {
        ProjectCard(title: "Team") {
            HStack(spacing: 8) {
                Text(team.count == 1 ? "1 bot" : "\(team.count) bots")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                if !team.isEmpty { modeSwitch }
            }
        } content: {
            if team.isEmpty {
                ProjectCardNote("No bots yet. Al picks bots for each thread, or @ a bot in the composer.")
            } else if graphMode && graphReady {
                ForEach(agents) { agent in agentRow(agent) }
                Button { onOpenPane(.team) } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "point.3.connected.trianglepath.dotted").font(.system(size: 12))
                        Text("Open team lanes").font(.system(size: 12.5))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            } else {
                ForEach(team) { bot in
                    Button { navigator.open(.page(bot.id)) } label: {
                        HStack(spacing: 8) {
                            BotAvatarView(bot: bot, size: 20)
                            BotNameText(bot.botDisplayName, size: 13)
                            Spacer(minLength: 0)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                if graphMode && factoryChecked && !graphReady {
                    ProjectCardNote(factoryFailed
                                    ? "Team graph: the Factory engine isn't running on this computer."
                                    : "Team graph: these bots aren't in the Factory yet.")
                }
            }
        }
        .task(id: context.botIds) {
            do {
                agents = try await BotProjectClient.shared.team(botIds: context.botIds).agents
                factoryFailed = false
            } catch is CancellationError {
                return
            } catch {
                factoryFailed = true
            }
            factoryChecked = true
        }
    }

    private var modeSwitch: some View {
        HStack(spacing: 2) {
            ForEach([true, false], id: \.self) { graph in
                Button { graphMode = graph } label: {
                    Text(graph ? "Graph" : "List")
                        .font(.system(size: 11.5))
                        .foregroundColor(graphMode == graph ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(RoundedRectangle(cornerRadius: 5).fill(graphMode == graph ? Color("BgPrimary") : Color.clear))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(graphMode == graph ? .isSelected : [])
            }
        }
        .padding(.horizontal, 2)
        .background(RoundedRectangle(cornerRadius: 7).fill(Color("FillNeutral")).padding(.vertical, 8))
    }

    /// A bot in the graph opens what it's doing (openAgentNode): its current
    /// node, else its latest thread in the project, else the project's threads.
    private func agentRow(_ agent: FactoryAgent) -> some View {
        Button {
            if let node = agent.currentNode {
                onOpenNode(node)
            } else if let latest = threads.filter({ $0.botId == agent.id }).max(by: { $0.lastActivityAt < $1.lastActivityAt }) {
                onOpenThread(latest)
            } else {
                onOpenPane(.threads)
            }
        } label: {
            ProjectAgentLabel(agent: agent)
        }
        .buttonStyle(.plain)
    }
}

/// A Factory bot's row: face, name, what it's on.
struct ProjectAgentLabel: View {
    let agent: FactoryAgent
    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        HStack(spacing: 8) {
            if let bot = store.bot(agent.id) {
                BotAvatarView(bot: bot, size: 22)
            }
            VStack(alignment: .leading, spacing: 1) {
                BotNameText(store.bot(agent.id)?.botDisplayName ?? agent.name, size: 13)
                let line = agent.currentNode?.title ?? agent.role ?? agent.state
                if let line, !line.isEmpty {
                    Text(line)
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let state = agent.state, !state.isEmpty {
                Text(state.replacingOccurrences(of: "_", with: " ").capitalized)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }
}

// MARK: - Campaign

struct ProjectCampaignCard: View {
    let projectId: String
    let onOpen: (ProjectPaneKind) -> Void

    @State private var campaign: FactoryCampaign?
    @State private var isLoading = true
    @State private var failed = false

    var body: some View {
        ProjectCard(title: "Campaign") {
            if let campaign {
                Text(campaign.total > 0 ? "\(campaign.proven)/\(campaign.total) proven" : "—")
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
            }
        } content: {
            if let campaign {
                Text(campaign.needsYou == 0 ? "Nothing needs you"
                     : campaign.needsYou == 1 ? "1 node needs you" : "\(campaign.needsYou) nodes need you")
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
                HStack(spacing: 16) {
                    ProjectTextButton("Board") { onOpen(.board) }
                    ProjectTextButton("Flow") { onOpen(.flow) }
                }
            } else {
                ProjectCardNote(isLoading ? "Checking the Factory…"
                                : failed ? "The Factory engine isn't running on this computer."
                                : "No campaign yet. Its Board and Flow show here once work runs through the Factory.")
            }
        }
        .task(id: projectId) {
            do {
                campaign = try await BotProjectClient.shared.campaign(projectId: projectId)
                failed = false
            } catch is CancellationError {
                return
            } catch {
                failed = true
            }
            isLoading = false
        }
    }
}

// MARK: - Artifacts

struct ProjectArtifactsCard: View {
    let items: [ArtifactV2Summary]
    let onOpen: (ArtifactV2Summary) -> Void
    let onLibrary: () -> Void

    private let shown = 8

    var body: some View {
        ProjectCard(title: "Artifacts") {
            if !items.isEmpty {
                ProjectTextButton("Library · \(items.count)", action: onLibrary)
            }
        } content: {
            if items.isEmpty {
                ProjectCardNote("Documents, sites and files the threads make show here.")
            } else {
                ForEach(items.prefix(shown)) { item in
                    Button { onOpen(item) } label: { ProjectArtifactRow(item: item) }
                        .buttonStyle(.plain)
                }
                if items.count > shown {
                    ProjectCardNote("\(items.count - shown) more in the threads")
                }
            }
        }
    }
}

struct ProjectArtifactRow: View {
    let item: ArtifactV2Summary

    private var icon: String {
        switch item.kind {
        case "design", "site", "html": return "globe"
        case "sheet": return "tablecells"
        case "slides": return "rectangle.on.rectangle"
        case "image": return "photo"
        case "pdf": return "doc.richtext"
        default: return "doc.text"
        }
    }

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(BotTime.relative(item.updatedAt))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
            Spacer(minLength: 0)
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }
}

// MARK: - Routines

struct ProjectRoutinesCard: View {
    let projectId: String
    let onOpen: () -> Void

    @State private var routines: [ProjectRoutineRow] = []
    @State private var error: String?

    var body: some View {
        ProjectCard(title: "Routines") {
            ProjectTextButton(routines.isEmpty ? "Add" : "Open · \(routines.count)", action: onOpen)
        } content: {
            if let error {
                ProjectInlineError(text: error) { Task { await load() } }
            } else if routines.isEmpty {
                ProjectCardNote("Schedule recurring work in this project.")
            } else {
                ForEach(routines.prefix(4)) { routine in
                    Button(action: onOpen) {
                        HStack(spacing: 8) {
                            Text(routine.name)
                                .font(.system(size: 12.5))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            Spacer(minLength: 4)
                            Text(routine.status == "active" ? "Active" : routine.status)
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextTertiary"))
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .task(id: projectId) { await load() }
    }

    private func load() async {
        do {
            routines = try await BotProjectClient.shared.routines(projectId: projectId)
            error = nil
        } catch is CancellationError {
        } catch {
            // No routines service on an older computer reads as none yet.
            routines = []
            self.error = nil
        }
    }
}

// MARK: - Status page

struct ProjectStatusCard: View {
    let projectId: String
    let onOpen: () -> Void

    @State private var status: ProjectStatusRecord?
    @State private var isLoading = true
    @State private var error: String?

    var body: some View {
        ProjectCard(title: "Status page") {
            if status != nil { ProjectTextButton("Open", action: onOpen) }
        } content: {
            if isLoading && status == nil {
                ProjectCardNote("Loading status…")
            } else if let error, status == nil {
                ProjectInlineError(text: "Couldn't load the status page. \(error)") { Task { await load() } }
            } else if let status {
                ProjectStateChip(state: status.state, closed: status.lifecycle == "closed")
                if status.nextAction.isEmpty {
                    ProjectCardNote("No status yet. Bots fill it on their first task.")
                } else {
                    Text(status.nextAction)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .task(id: projectId) { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            status = try await BotProjectClient.shared.status(projectId: projectId)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

struct ProjectStateChip: View {
    let state: String
    var closed = false

    private var color: Color {
        switch state {
        case "on_track", "done": return Theme.statusSuccess
        case "at_risk", "needs_decision": return Theme.statusWarning
        case "blocked": return Theme.statusError
        default: return Color("TextTertiary")
        }
    }

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(closed ? "Closed" : ProjectStatusRecord.label(forState: state))
                .font(.system(size: 11.5))
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Capsule().fill(Color("FillNeutral")))
    }
}

// MARK: - Context files

struct ProjectContextFilesCard: View {
    let projectId: String

    @State private var files: [CoworkProjectFile] = []
    @State private var error: String?
    @State private var isPicking = false
    @State private var isAdding = false

    private let projects = ProjectsClient()
    private let chat = AgentChatClient()

    var body: some View {
        ProjectCard(title: "Context") {
            if isAdding {
                ProgressView()
            } else {
                ProjectIconButton(systemName: "plus", label: "Add files") { isPicking = true }
            }
        } content: {
            if let error {
                ProjectInlineError(text: error) { Task { await load() } }
            }
            if files.isEmpty {
                ProjectCardNote("Add PDFs, documents, or other text to reference in this project.")
            } else {
                ForEach(files) { file in
                    HStack(spacing: 10) {
                        Image(systemName: "doc.text")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                        Text(file.name)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        ProjectIconButton(systemName: "xmark", label: "Remove \(file.name)") { remove(file) }
                    }
                }
            }
        }
        .sheet(isPresented: $isPicking) {
            DocumentPicker { url in add(url) }
        }
        .task(id: projectId) { await load() }
    }

    private func load() async {
        do {
            files = try await projects.listFiles(projectId: projectId)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = "Couldn't load the files. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// bot-project-files.ts: the bytes to uploads (20 MB cap), then a row on the project.
    private func add(_ url: URL) {
        guard let data = try? Data(contentsOf: url) else {
            error = "Couldn't read that file."
            return
        }
        guard data.count <= 20 * 1024 * 1024 else {
            error = "Files up to 20 MB can be added to a project."
            return
        }
        let mediaType = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        let name = url.lastPathComponent
        isAdding = true
        Task {
            defer { isAdding = false }
            do {
                let upload = try await chat.upload(name: name, mediaType: mediaType, dataBase64: data.base64EncodedString())
                try await projects.addFile(projectId: projectId, name: name, url: upload.url, uploadId: upload.uploadId, mediaType: mediaType)
                await load()
            } catch {
                self.error = "Couldn't add \(name). Try again."
            }
        }
    }

    private func remove(_ file: CoworkProjectFile) {
        Task {
            do {
                try await projects.deleteFile(projectId: projectId, fileId: file.id)
                files.removeAll { $0.id == file.id }
            } catch {
                self.error = "Couldn't remove the file. Try again."
            }
        }
    }
}

// MARK: - Coordinator conversation

/// Al's welcome, your messages with their receipts, Al's replies with fanout
/// chips, routed pills and wrap-ups, day dividers, and a decision card for
/// every thread waiting on you.
struct ProjectConversationView: View {
    let messages: [ProjectMessage]
    let threads: [BotThread]
    var artifacts: [ArtifactV2Summary] = []
    /// nil: no welcome (a Chat project's page has its own intro).
    var welcomeTeam: [String]? = nil
    let onOpenThread: (BotThread) -> Void
    var onOpenArtifact: ((ArtifactV2Summary) -> Void)? = nil

    private var byId: [String: BotThread] {
        Dictionary(threads.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
    }

    private static func date(_ iso: String) -> Date? { BotTime.date(iso) }

    private static func time(_ iso: String) -> String {
        date(iso)?.formatted(date: .omitted, time: .shortened) ?? ""
    }

    /// components/threads/day-label.ts: Today, Yesterday, else the date.
    private static func day(_ iso: String) -> String? {
        guard let date = date(iso) else { return nil }
        if Calendar.current.isDateInToday(date) { return "Today" }
        if Calendar.current.isDateInYesterday(date) { return "Yesterday" }
        return date.formatted(date: .abbreviated, time: .omitted)
    }

    private func receipt(after index: Int) -> String? {
        guard index + 1 < messages.count else { return nil }
        let next = messages[index + 1]
        guard next.role == "coordinator", let payload = next.payload else { return nil }
        if payload.kind == "fanout" {
            return payload.threads.count == 1 ? "Sent to 1 thread" : "Sent to \(payload.threads.count) threads"
        }
        if payload.kind == "routed" { return "Sent to one thread" }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let welcomeTeam { welcome(welcomeTeam) }
            ForEach(Array(messages.enumerated()), id: \.element.id) { index, message in
                if let label = Self.day(message.createdAt),
                   index == 0 || Self.day(messages[index - 1].createdAt) != label {
                    dayDivider(label)
                }
                if message.role == "user" {
                    userBubble(message, receipt: receipt(after: index))
                } else {
                    coordinatorReply(message)
                }
            }
            ForEach(threads.filter { $0.group == "waiting" }) { thread in
                ProjectDecisionCard(thread: thread) { onOpenThread(thread) }
            }
        }
    }

    private func welcome(_ team: [String]) -> some View {
        let who: String
        if team.isEmpty {
            who = "threads, pick a bot for each"
        } else if team.count == 1 {
            who = "threads for \(team[0])"
        } else {
            who = "threads for \(team.dropLast().joined(separator: ", ")) and \(team[team.count - 1])"
        }
        return VStack(alignment: .leading, spacing: 6) {
            alHeader(nil)
            Text("I coordinate the work here. Tell me what this project needs, and I'll split it into \(who), start them, and let you know when something needs you.")
                .font(.system(size: 13.5))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func alHeader(_ time: String?) -> some View {
        HStack(spacing: 6) {
            AProtocolMark(height: 8, label: "Al")
            Text("Al").font(.system(size: 12, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(time.map { "· coordinator · \($0)" } ?? "· coordinator")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        }
    }

    private func dayDivider(_ label: String) -> some View {
        HStack(spacing: 10) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            Text(label).font(.system(size: 11.5)).foregroundColor(Color("TextTertiary"))
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
        .accessibilityElement(children: .combine)
    }

    private func userBubble(_ message: ProjectMessage, receipt: String?) -> some View {
        VStack(alignment: .trailing, spacing: 4) {
            Text(message.text)
                .font(.system(size: 13.5))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 12, bottomTrailingRadius: 4, topTrailingRadius: 12)
                    .fill(Color("FillNeutral")))
                .padding(.leading, 36)
            if let receipt {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.uturn.left").font(.system(size: 10))
                    Text("\(receipt) · \(Self.time(message.createdAt))")
                }
                .font(.system(size: 11.5))
                .foregroundColor(Color("TextTertiary"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }

    @ViewBuilder
    private func coordinatorReply(_ message: ProjectMessage) -> some View {
        let payload = message.payload
        let kind = payload?.kind
        VStack(alignment: .leading, spacing: 6) {
            alHeader(Self.time(message.createdAt))
            if kind == "synthesis" {
                wrapUp(message.text, count: payload?.threadCount ?? threads.count)
            } else {
                Text(message.text)
                    .font(.system(size: 13.5))
                    .foregroundColor(kind == "blocked" ? Theme.statusWarning : Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if kind == "fanout", let receipts = payload?.threads {
                ForEach(receipts, id: \.id) { fanoutChip($0) }
            }
            if kind == "routed" || kind == "completed" || kind == "blocked",
               let threadId = payload?.threadId,
               kind == "routed" || byId[threadId] != nil {
                threadPill(title: payload?.threadTitle ?? byId[threadId]?.title ?? "Thread", threadId: threadId)
            }
        }
    }

    private func dotColor(_ group: String?) -> Color {
        (group.flatMap(BotThreadGroup.init(rawValue:)) ?? .queued).color
    }

    private func fanoutChip(_ receipt: ProjectReceiptThread) -> some View {
        let thread = byId[receipt.id]
        let files = artifacts.filter { item in
            guard let session = thread?.currentSessionId else { return false }
            return item.origin?.sessionId == session
        }
        return HStack(spacing: 6) {
            Button { if let thread { onOpenThread(thread) } } label: {
                HStack(spacing: 8) {
                    Circle().fill(dotColor(thread?.group)).frame(width: 7, height: 7)
                    Text(thread?.title ?? receipt.title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let line = thread?.statusLine, !line.isEmpty {
                        Text(line)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .lineLimit(1)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10)
                .frame(minHeight: 44)
                .background(RoundedRectangle(cornerRadius: 10).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(thread == nil)
            if let first = files.first, let onOpenArtifact {
                Button { onOpenArtifact(first) } label: {
                    Text(files.count > 1 ? "\(first.title) +\(files.count - 1)" : first.title)
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .padding(.horizontal, 6)
                        .frame(maxWidth: 110, minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func threadPill(title: String, threadId: String) -> some View {
        let thread = byId[threadId]
        return Button { if let thread { onOpenThread(thread) } } label: {
            HStack(spacing: 6) {
                Circle().fill(dotColor(thread?.group ?? "working")).frame(width: 6, height: 6)
                Text(title).lineLimit(1)
            }
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(Color("AccentPrimary"))
            .padding(.horizontal, 8)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 6).fill(Color("AccentPrimary").opacity(0.14)).padding(.vertical, 8))
        }
        .buttonStyle(.plain)
        .disabled(thread == nil)
    }

    private func wrapUp(_ text: String, count: Int) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("WRAP-UP · \(count) \(count == 1 ? "THREAD" : "THREADS")")
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(Color("TextTertiary"))
            Text(text)
                .font(.system(size: 13.5))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

/// A thread waiting on you (DecisionCard.tsx): when the bot asked a question
/// with options, they answer it right here; otherwise the card opens the thread.
struct ProjectDecisionCard: View {
    let thread: BotThread
    let onOpen: () -> Void

    @ObservedObject private var store = BotModeStore.shared
    @State private var pending: AgentQuestionRequest?
    @State private var answered: String?
    @State private var error: String?

    private var question: AgentQuestionInfo? { pending?.questions.first }
    private var choices: [AgentQuestionOption] {
        guard let question, !question.multiple, pending?.questions.count == 1 else { return [] }
        return question.options
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Circle().fill(Theme.statusWarning).frame(width: 7, height: 7)
                if let bot = store.bot(thread.botId) { BotAvatarView(bot: bot, size: 18) }
                Text(thread.title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
            }
            Text(question?.question ?? thread.statusLine ?? "This thread needs a call from you.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if let answered {
                Text("You picked \(answered).")
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(choices, id: \.label) { choice in
                    decisionButton(choice.label, filled: true) { answer(choice.label) }
                }
            }
            decisionButton("View thread", filled: false, action: onOpen)
            if let error {
                Text(error).font(.system(size: 12)).foregroundColor(Theme.statusError)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).fill(Theme.statusWarning.opacity(0.06)))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.statusWarning.opacity(0.4), lineWidth: 1))
        .task(id: "\(thread.currentSessionId ?? "")|\(thread.lastActivityAt)") {
            guard let sessionId = thread.currentSessionId else { return }
            pending = (try? await AgentAsksClient.shared.listQuestions(sessionId: sessionId))?.first
        }
    }

    private func decisionButton(_ title: String, filled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(filled ? Color("OnAccent") : Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(minHeight: 44)
                .background(RoundedRectangle(cornerRadius: 8).fill(filled ? Color("AccentPrimary") : Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: filled ? 0 : 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func answer(_ label: String) {
        guard let pending else { return }
        error = nil
        Task {
            do {
                try await AgentAsksClient.shared.replyQuestion(
                    id: pending.id,
                    answers: [AgentAsksClient.ReplyAnswer(questionIndex: 0, answer: .one(label))]
                )
                answered = label
            } catch {
                self.error = "That didn't go through. Try again."
            }
        }
    }
}
