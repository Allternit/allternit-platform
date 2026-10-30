import SwiftUI

// The project's pane on the phone (ProjectSidePanel + the Factory views, shown
// full screen by openPhonePane on the web): a menu title switches Threads ·
// Delegate · Library · Routines · Team graph · Board · Flow. Plus the sheets
// the project page opens: settings, edit details, save as template, thread
// graph, memory, needs you, add a bot, status page.

struct ProjectPaneContext: Hashable {
    let id: String
    let title: String
    let botIds: [String]
}

enum ProjectPaneKind: String, CaseIterable, Hashable {
    case threads, delegate, library, routines, team, board, flow

    var label: String {
        switch self {
        case .threads: return "Threads"
        case .delegate: return "Delegate"
        case .library: return "Library"
        case .routines: return "Routines"
        case .team: return "Team graph"
        case .board: return "Board"
        case .flow: return "Flow"
        }
    }
}

struct ProjectPaneScreen: View {
    let context: ProjectPaneContext
    @State private var kind: ProjectPaneKind

    init(context: ProjectPaneContext, initialKind: ProjectPaneKind) {
        self.context = context
        _kind = State(initialValue: initialKind)
    }

    var body: some View {
        Group {
            switch kind {
            case .threads: ProjectThreadsPane(projectId: context.id)
            case .delegate: ProjectDelegatePane(projectId: context.id)
            case .library: ProjectLibraryPane(projectId: context.id)
            case .routines: ProjectRoutinesPane(context: context)
            case .team: ProjectTeamPane(botIds: context.botIds)
            case .board: ProjectBoardPane(projectId: context.id)
            case .flow: ProjectFlowPane(projectId: context.id)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Menu {
                    ForEach(ProjectPaneKind.allCases, id: \.self) { option in
                        Button {
                            kind = option
                        } label: {
                            if option == kind {
                                Label(option.label, systemImage: "checkmark")
                            } else {
                                Text(option.label)
                            }
                        }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text(kind.label)
                            .font(.system(size: 15, weight: .semibold))
                        Image(systemName: "chevron.down")
                            .font(.system(size: 10, weight: .semibold))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .accessibilityLabel("Pane: \(kind.label)")
            }
        }
    }
}

// MARK: - Threads

struct ProjectThreadsPane: View {
    let projectId: String

    @EnvironmentObject private var navigator: BotNavigator
    @State private var threads: [BotThread] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var graphOpen = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Spacer()
                    Button { graphOpen = true } label: {
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
                }
                .padding(.horizontal, 16)
                if isLoading && threads.isEmpty {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if let error, threads.isEmpty {
                    ProjectInlineError(text: error) { Task { await load() } }.padding(.horizontal, 16)
                } else {
                    BotThreadsPanel(
                        botId: nil,
                        greeting: threads.contains { $0.group == "waiting" } ? "Needs you" : "All on track",
                        threadsOverride: threads,
                        onOpen: { navigator.openThread($0) },
                        onReopen: { thread in
                            Task {
                                try? await BotProjectClient.shared.reopenThread(thread)
                                await load()
                            }
                        }
                    )
                }
            }
        }
        .refreshable { await load() }
        .task {
            await load()
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                if Task.isCancelled { break }
                await load()
            }
        }
        .sheet(isPresented: $graphOpen) {
            ProjectThreadGraphSheet(threads: threads) { thread in
                graphOpen = false
                navigator.openThread(thread)
            }
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            threads = try await BotModeClient.shared.listProjectThreads(projectId: projectId)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

/// A thread with no chat yet (ProjectSidePanel `threadChat`): what it's about
/// and where it stands; once it has a session it IS the chat.
struct BotProjectThreadPage: View {
    @State private var thread: BotThread
    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var reopenError: String?

    init(thread: BotThread) {
        _thread = State(initialValue: thread)
    }

    var body: some View {
        Group {
            if let sessionId = thread.currentSessionId, let bot = store.bot(thread.botId) {
                BotChatSessionView(bot: bot, explicitSessionId: sessionId)
            } else {
                waiting
            }
        }
        .task {
            while !Task.isCancelled, thread.currentSessionId == nil {
                if let fresh = try? await BotProjectClient.shared.thread(id: thread.id) { thread = fresh }
                try? await Task.sleep(nanoseconds: 5_000_000_000)
            }
        }
    }

    private var waiting: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let bot = store.bot(thread.botId) {
                    Button { navigator.open(.page(bot.id)) } label: {
                        HStack(spacing: 8) {
                            BotAvatarView(bot: bot, size: 26)
                            BotNameText(bot.botDisplayName, size: 14)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                Text(thread.title)
                    .font(.system(size: 22, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                if let line = thread.statusLine, !line.isEmpty {
                    Text(line)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let todo = thread.todo, !todo.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(todo.enumerated()), id: \.offset) { _, item in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Image(systemName: item.state == "done" ? "checkmark.circle.fill"
                                      : item.state == "active" ? "circle.dotted" : "circle")
                                    .font(.system(size: 13))
                                    .foregroundColor(item.state == "done" ? Theme.statusSuccess : Color("TextTertiary"))
                                Text(item.text)
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextPrimary"))
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                }
                Text("This thread hasn't started yet. It opens here once it has.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity)
                    .multilineTextAlignment(.center)
                    .padding(.top, 16)
                if thread.group == "resolved" {
                    Button("Reopen") {
                        Task {
                            do {
                                try await BotProjectClient.shared.reopenThread(thread)
                                thread = try await BotProjectClient.shared.thread(id: thread.id)
                            } catch {
                                reopenError = "Couldn't reopen the thread. Try again."
                            }
                        }
                    }
                    .font(.system(size: 13, weight: .medium))
                    .frame(maxWidth: .infinity, minHeight: 44)
                    if let reopenError {
                        Text(reopenError).font(.system(size: 12)).foregroundColor(Theme.statusError)
                    }
                }
            }
            .padding(16)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Thread")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// The plan as a graph (ThreadGraph.tsx): each task thread in the column of
/// its dependency depth, edges from what it waits on.
struct ProjectThreadGraphSheet: View {
    let threads: [BotThread]
    let onOpen: (BotThread) -> Void

    @ObservedObject private var store = BotModeStore.shared
    @Environment(\.dismiss) private var dismiss

    private static let column: CGFloat = 196
    private static let row: CGFloat = 64
    private static let nodeWidth: CGFloat = 172
    private static let nodeHeight: CGFloat = 48
    private static let pad: CGFloat = 12

    private struct Node: Identifiable {
        let thread: BotThread
        let x: CGFloat
        let y: CGFloat
        var id: String { thread.id }
        var center: CGPoint { CGPoint(x: x + ProjectThreadGraphSheet.nodeWidth / 2, y: y + ProjectThreadGraphSheet.nodeHeight / 2) }
    }

    private var nodes: [Node] {
        let task = threads.filter { $0.kind == "task" && !$0.incognito }
        let byId = Dictionary(task.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var depth: [String: Int] = [:]
        func visit(_ thread: BotThread, _ seen: inout Set<String>) -> Int {
            if let known = depth[thread.id] { return known }
            if seen.contains(thread.id) { return 0 }
            seen.insert(thread.id)
            var deepest = 0
            for id in thread.dependsOn ?? [] {
                if let dep = byId[id] { deepest = max(deepest, visit(dep, &seen) + 1) }
            }
            depth[thread.id] = deepest
            return deepest
        }
        for thread in task {
            var seen = Set<String>()
            _ = visit(thread, &seen)
        }
        var rows: [Int: Int] = [:]
        return task.map { thread in
            let d = depth[thread.id] ?? 0
            let r = rows[d] ?? 0
            rows[d] = r + 1
            return Node(thread: thread, x: Self.pad + CGFloat(d) * Self.column, y: Self.pad + CGFloat(r) * Self.row)
        }
    }

    var body: some View {
        NavigationStack {
            Group {
                let laidOut = nodes
                if laidOut.isEmpty {
                    Text("No plan yet. Ask Al for something and the threads show here.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .multilineTextAlignment(.center)
                        .padding(24)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ScrollView([.horizontal, .vertical]) {
                        graph(laidOut)
                    }
                }
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Thread graph")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func graph(_ laidOut: [Node]) -> some View {
        let width = (laidOut.map { $0.x + Self.nodeWidth }.max() ?? 0) + Self.pad
        let height = (laidOut.map { $0.y + Self.nodeHeight }.max() ?? 0) + Self.pad
        let at = Dictionary(laidOut.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return ZStack(alignment: .topLeading) {
            Path { path in
                for node in laidOut {
                    for id in node.thread.dependsOn ?? [] {
                        guard let from = at[id] else { continue }
                        let start = CGPoint(x: from.x + Self.nodeWidth, y: from.center.y)
                        let end = CGPoint(x: node.x, y: node.center.y)
                        let mid = (start.x + end.x) / 2
                        path.move(to: start)
                        path.addCurve(to: end, control1: CGPoint(x: mid, y: start.y), control2: CGPoint(x: mid, y: end.y))
                    }
                }
            }
            .stroke(Color("BorderStrong"), lineWidth: 1)
            ForEach(laidOut) { node in
                Button { onOpen(node.thread) } label: { nodeLabel(node.thread) }
                    .buttonStyle(.plain)
                    .position(node.center)
            }
        }
        .frame(width: width, height: height)
    }

    private func nodeLabel(_ thread: BotThread) -> some View {
        let group = BotThreadGroup(rawValue: thread.group) ?? .idle
        let parts = [store.bot(thread.botId)?.botDisplayName, thread.progressLabel].compactMap { $0 }
        return HStack(spacing: 8) {
            Circle().fill(group.color).frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 1) {
                Text(thread.title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(parts.joined(separator: " · "))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .frame(width: Self.nodeWidth, height: Self.nodeHeight)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
    }
}

// MARK: - Delegate

/// The Delegate board on the phone (ProjectDelegate.tsx `phone`): one column
/// at a time, "Assign to…" instead of drag, the bots with cost and load below.
struct ProjectDelegatePane: View {
    let projectId: String

    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var board: DelegateBoard?
    @State private var threads: [BotThread] = []
    @State private var loadError: String?
    @State private var note: String?
    @State private var column: Column = .todo
    @State private var newTask = ""
    @State private var estimate: Int?
    @State private var busy = false

    private enum Column: String, CaseIterable, Identifiable {
        case todo, working, review, done
        var id: String { rawValue }
        var label: String {
            switch self {
            case .todo: return "To do"
            case .working: return "Working"
            case .review: return "Needs you"
            case .done: return "Done"
            }
        }

        /// delegate.ts delegateColumn.
        static func of(_ status: String) -> Column? {
            switch status {
            case "queued": return .todo
            case "planning", "working": return .working
            case "blocked", "needs_you", "paused", "review": return .review
            case "done", "failed": return .done
            default: return nil
            }
        }
    }

    private struct Card: Identifiable {
        var task: DelegateTask? = nil
        var threadId: String? = nil
        let title: String
        var status: String? = nil
        let estimate: Int?
        var botId: String? = nil
        var id: String { task?.id ?? threadId ?? title }
    }

    private var now: Date { board.flatMap { BotTime.date($0.now) } ?? Date() }

    private var cards: [Column: [Card]] {
        guard let board else { return [:] }
        var out: [Column: [Card]] = [:]
        out[.todo] = board.tasks.map { Card(task: $0, title: $0.title, estimate: $0.estimateMin) }
        var inLanes = Set<String>()
        for bot in board.bots {
            for item in bot.queue {
                inLanes.insert(item.threadId)
                guard let col = Column.of(item.status) else { continue }
                out[col, default: []].append(Card(threadId: item.threadId, title: item.title, status: item.status,
                                                  estimate: item.estimateMin, botId: bot.id))
            }
        }
        // Needs you and Done also come from the project's threads (the board
        // returns only open lane threads).
        for thread in threads where thread.kind == "task" && !inLanes.contains(thread.id) {
            guard let col = Column.of(thread.status), col == .review || col == .done else { continue }
            out[col, default: []].append(Card(threadId: thread.id, title: thread.title, status: thread.status,
                                              estimate: nil, botId: thread.botId))
        }
        return out
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let board {
                    content(board)
                } else if let loadError {
                    ProjectInlineError(text: loadError) { Task { await load() } }
                } else {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task {
            await load()
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                if Task.isCancelled { break }
                await load()
            }
        }
    }

    @ViewBuilder
    private func content(_ board: DelegateBoard) -> some View {
        HStack {
            Text(board.auto ? "New tasks go to whoever is free first" : "Tap Assign to hand a task to a bot")
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextSecondary"))
            Spacer(minLength: 8)
            Toggle("Auto", isOn: Binding(get: { board.auto }, set: { on in setAuto(on) }))
                .labelsHidden()
                .disabled(busy)
            Text("Auto").font(.system(size: 12.5)).foregroundColor(Color("TextSecondary"))
        }
        let all = cards
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(Column.allCases) { option in
                    Button { column = option } label: {
                        HStack(spacing: 5) {
                            Text(option.label)
                            Text("\(all[option]?.count ?? 0)").foregroundColor(Color("TextTertiary"))
                        }
                        .font(.system(size: 13, weight: column == option ? .medium : .regular))
                        .foregroundColor(column == option ? Color("TextPrimary") : Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .frame(minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 8).fill(column == option ? Color("FillNeutral") : Color.clear))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        if column == .todo { addRow }
        let list = all[column] ?? []
        if list.isEmpty {
            Text("Nothing here")
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextTertiary"))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 16)
        }
        ForEach(list) { card in cardRow(card, board: board) }
        if let note {
            Text(note).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
        }
        botsSection(board)
    }

    private var addRow: some View {
        HStack(spacing: 6) {
            TextField("Add a task", text: $newTask)
                .font(.system(size: 14))
                .submitLabel(.done)
                .onSubmit { addTask() }
            Menu {
                Button("No estimate") { estimate = nil }
                ForEach([10, 15, 20, 30, 45, 60, 90, 120], id: \.self) { minutes in
                    Button(DelegateWords.minutes(minutes)) { estimate = minutes }
                }
            } label: {
                HStack(spacing: 3) {
                    Text(estimate.map(DelegateWords.minutes) ?? "Estimate")
                    Image(systemName: "chevron.down").font(.system(size: 9))
                }
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextSecondary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            ProjectIconButton(systemName: "plus", label: "Add task") { addTask() }
                .disabled(busy || newTask.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.leading, 12)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func chip(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: 5).fill(Color("FillNeutral")))
    }

    @ViewBuilder
    private func cardRow(_ card: Card, board: DelegateBoard) -> some View {
        let summary = VStack(alignment: .leading, spacing: 4) {
            Text(card.title)
                .font(.system(size: 13.5))
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.leading)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 6) {
                if card.status == "queued" { chip("Next in line") }
                if let estimate = card.estimate { chip(DelegateWords.minutes(estimate)) }
                Spacer(minLength: 0)
                if let botId = card.botId, let bot = store.bot(botId) { BotAvatarView(bot: bot, size: 20) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        Group {
            if let task = card.task {
                VStack(alignment: .leading, spacing: 0) {
                    summary
                    HStack(spacing: 4) {
                        Menu {
                            ForEach(board.bots) { bot in
                                Button("\(bot.name) · \(DelegateWords.dropPreview(bot, estimate: task.estimateMin, now: now))") {
                                    assign(task, to: bot)
                                }
                            }
                        } label: {
                            Text("Assign to…")
                                .font(.system(size: 12.5))
                                .foregroundColor(Color("TextPrimary"))
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .disabled(busy || board.bots.isEmpty)
                        Spacer()
                        ProjectIconButton(systemName: "trash", label: "Remove \(task.title)") { remove(task) }
                            .disabled(busy)
                    }
                }
            } else {
                Button { openThread(card) } label: {
                    summary
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    @ViewBuilder
    private func botsSection(_ board: DelegateBoard) -> some View {
        HStack {
            Text("Bots").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            Spacer()
            if let queued = board.queuedCostCents {
                Text("≈ \(DelegateWords.cents(queued)) in lanes · estimate")
                    .font(.system(size: 11.5).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(.top, 8)
        if board.bots.isEmpty {
            Text("Add a bot to this project to hand tasks to.")
                .font(.system(size: 12.5))
                .foregroundColor(Color("TextSecondary"))
        }
        ForEach(board.bots) { bot in
            Button { navigator.open(.page(bot.id)) } label: { botRow(bot) }
                .buttonStyle(.plain)
        }
    }

    private func botRow(_ bot: DelegateBot) -> some View {
        let load = DelegateWords.load(bot, now: now)
        let active = bot.queue.filter { DelegateWords.isActive($0.status) }.count
        return HStack(spacing: 10) {
            if let record = store.bot(bot.id) { BotAvatarView(bot: record, size: 26) }
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    BotNameText(store.bot(bot.id)?.botDisplayName ?? bot.name, size: 13)
                    if active > 0 {
                        Text("\(active)").font(.system(size: 11.5)).foregroundColor(Color("TextTertiary"))
                    }
                }
                Text(DelegateWords.costLine(bot)).font(.system(size: 11.5)).foregroundColor(Color("TextTertiary"))
            }
            Spacer(minLength: 4)
            Text(load)
                .font(.system(size: 11.5))
                .foregroundColor(load == "Free" ? Theme.statusSuccess : Color("TextSecondary"))
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(Capsule().fill(Color("FillNeutral")))
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }

    // MARK: Actions

    private func load() async {
        do {
            let loaded = try await BotProjectClient.shared.delegateBoard(projectId: projectId)
            board = loaded
            threads = (try? await BotModeClient.shared.listProjectThreads(projectId: projectId)) ?? threads
            loadError = nil
        } catch is CancellationError {
        } catch {
            loadError = "Couldn't load Delegate. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func openThread(_ card: Card) {
        guard let threadId = card.threadId else { return }
        Task {
            if let thread = threads.first(where: { $0.id == threadId }) {
                navigator.openThread(thread)
            } else if let thread = try? await BotProjectClient.shared.thread(id: threadId) {
                navigator.openThread(thread)
            } else {
                note = "Couldn't open \(card.title). Try again."
            }
        }
    }

    private func run(_ failure: String, _ work: @escaping @MainActor () async throws -> Void) {
        busy = true
        note = nil
        Task {
            defer { busy = false }
            do {
                try await work()
                await load()
            } catch {
                note = failure
            }
        }
    }

    private func addTask() {
        let title = newTask.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, !busy else { return }
        let minutes = estimate
        newTask = ""
        estimate = nil
        run("Couldn't add that task. Try again.") {
            let updated = try await BotProjectClient.shared.addDelegateTask(projectId: projectId, title: title, estimateMin: minutes)
            if let autoError = updated.autoError { note = "Auto couldn't hand that out: \(autoError)" }
        }
    }

    private func remove(_ task: DelegateTask) {
        run("Couldn't remove \(task.title). Try again.") {
            try await BotProjectClient.shared.removeDelegateTask(projectId: projectId, taskId: task.id)
        }
    }

    private func assign(_ task: DelegateTask, to bot: DelegateBot) {
        run("Couldn't hand \(task.title) to \(bot.name). Try again.") {
            try await BotProjectClient.shared.assignDelegateTask(projectId: projectId, taskId: task.id, botId: bot.id)
        }
    }

    private func setAuto(_ on: Bool) {
        run("Couldn't change Auto. Try again.") {
            try await BotProjectClient.shared.setDelegateAuto(projectId: projectId, auto: on)
        }
    }
}

// MARK: - Library

struct ProjectLibraryPane: View {
    let projectId: String

    @State private var items: [ArtifactV2Summary] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var opened: ArtifactV2Summary?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if isLoading && items.isEmpty {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if let error, items.isEmpty {
                    ProjectInlineError(text: error) { Task { await load() } }
                } else if items.isEmpty {
                    Text("Documents, sites and files the threads make show here.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(maxWidth: .infinity)
                        .multilineTextAlignment(.center)
                        .padding(.vertical, 40)
                } else {
                    ForEach(items) { item in
                        Button { opened = item } label: { ProjectArtifactRow(item: item) }
                            .buttonStyle(.plain)
                    }
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await load() }
        .sheet(item: $opened) { item in
            V2ArtifactView(artifactId: item.id, initialTitle: item.title) {
                items.removeAll { $0.id == item.id }
            }
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            let threads = try await BotModeClient.shared.listProjectThreads(projectId: projectId)
            let sessions = Set(threads.compactMap(\.currentSessionId))
            var query = ArtifactV2ListQuery()
            query.limit = 100
            let page = try await ArtifactsV2Client.shared.list(query)
            items = page.items.filter { $0.origin?.sessionId.map(sessions.contains) ?? false }
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

// MARK: - Routines

struct ProjectRoutinesPane: View {
    let context: ProjectPaneContext

    @State private var routines: [ProjectRoutineRow] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var isAdding = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if isLoading && routines.isEmpty {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if let error, routines.isEmpty {
                    ProjectInlineError(text: error) { Task { await load() } }
                } else if routines.isEmpty {
                    VStack(spacing: 12) {
                        Image(systemName: "clock.arrow.circlepath")
                            .font(.system(size: 22))
                            .foregroundColor(Color("TextTertiary"))
                        Text("Use routines to schedule recurring work in this project, like a morning summary or a weekly report.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .multilineTextAlignment(.center)
                        addButton
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 60)
                } else {
                    ForEach(routines) { routine in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(routine.name)
                                .font(.system(size: 13.5))
                                .foregroundColor(Color("TextPrimary"))
                            Text(routine.scheduleLine)
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextTertiary"))
                        }
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    }
                    addButton.padding(.top, 8)
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await load() }
        .sheet(isPresented: $isAdding) {
            ProjectNewRoutineSheet(context: context) { await load() }
        }
    }

    private var addButton: some View {
        Button { isAdding = true } label: {
            HStack(spacing: 6) {
                Image(systemName: "plus").font(.system(size: 12, weight: .semibold))
                Text("New routine").font(.system(size: 13, weight: .medium))
            }
            .foregroundColor(Color("OnAccent"))
            .padding(.horizontal, 14)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 10).fill(Color("AccentPrimary")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func load() async {
        defer { isLoading = false }
        do {
            routines = try await BotProjectClient.shared.routines(projectId: context.id)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

/// A routine for this project (openScheduledTaskModal with `projectId`):
/// the row carries `metadata.projectId`, which is what the project lists by.
struct ProjectNewRoutineSheet: View {
    let context: ProjectPaneContext
    let onSaved: @MainActor () async -> Void

    @ObservedObject private var store = BotModeStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var instruction = ""
    @State private var botId: String?
    @State private var frequency: CronWords.Frequency = .daily
    @State private var time = Calendar.current.date(bySettingHour: 9, minute: 0, second: 0, of: Date()) ?? Date()
    @State private var weekday = 1
    @State private var isSaving = false
    @State private var error: String?

    private var bots: [AgentRecord] {
        let team = context.botIds.compactMap { store.bot($0) }
        return team.isEmpty ? store.bots : team
    }

    private var cron: String {
        let c = Calendar.current.dateComponents([.hour, .minute], from: time)
        return CronWords.cron(frequency, hour: c.hour ?? 9, minute: c.minute ?? 0, weekday: weekday)
    }

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && !instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && botId != nil && !isSaving
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $name)
                    TextField("What should the bot do?", text: $instruction, axis: .vertical)
                        .lineLimit(3...8)
                }
                Section {
                    Picker("Bot", selection: $botId) {
                        Text("Choose").tag(String?.none)
                        ForEach(bots) { Text($0.botDisplayName).tag(String?.some($0.id)) }
                    }
                }
                Section {
                    Picker("Repeats", selection: $frequency) {
                        ForEach(CronWords.Frequency.allCases) { Text($0.rawValue).tag($0) }
                    }
                    if frequency == .weekly {
                        Picker("Day", selection: $weekday) {
                            ForEach(0..<7, id: \.self) { Text(Calendar.current.weekdaySymbols[$0]).tag($0) }
                        }
                    }
                    DatePicker(frequency == .hourly ? "Minute" : "Time", selection: $time, displayedComponents: .hourAndMinute)
                } footer: {
                    Text(CronWords.describe(type: "cron", expression: cron))
                }
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle("New routine")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isSaving ? "Saving…" : "Save") { Task { await save() } }
                        .disabled(!canSave)
                }
            }
        }
    }

    private func save() async {
        guard let botId else { return }
        isSaving = true
        error = nil
        defer { isSaving = false }
        do {
            _ = try await AutomationClient.shared.createRoutine(CreateAutomationRoutineRequest(
                name: name.trimmingCharacters(in: .whitespaces),
                description: nil,
                agentId: botId,
                scheduleType: "cron",
                scheduleExpression: cron,
                timezone: TimeZone.current.identifier,
                executionDomain: "local",
                config: ["jobType": "agent", "prompt": instruction.trimmingCharacters(in: .whitespacesAndNewlines)],
                metadata: ["projectId": .string(context.id), "projectTitle": .string(context.title)]
            ))
            await onSaved()
            dismiss()
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

// MARK: - Factory panes

private struct ProjectNodeRow: View {
    let card: FactoryNodeCard
    @EnvironmentObject private var navigator: BotNavigator

    var body: some View {
        Button {
            navigator.open(.projectNode(dagId: card.dagId, nodeId: card.nodeId, title: card.title))
        } label: {
            HStack(spacing: 8) {
                Circle()
                    .fill(card.status == "done" ? Theme.statusSuccess
                          : card.status == "needs_you" ? Theme.statusWarning
                          : card.status == "failed" ? Theme.statusError
                          : card.status == "working" || card.status == "checking" ? Theme.statusInfo
                          : Color("TextTertiary"))
                    .frame(width: 7, height: 7)
                VStack(alignment: .leading, spacing: 1) {
                    Text(card.title)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.leading)
                    Text([card.statusLabel, card.assignee].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                if let proof = card.proof, proof.total > 0 {
                    Text("\(proof.proven)/\(proof.total)")
                        .font(.system(size: 11.5).monospacedDigit())
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct ProjectPaneHeading: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
            .padding(.top, 10)
    }
}

/// Board: Needs you, Now, Next, then the waves (ProjectBoardView `oneColumn`).
struct ProjectBoardPane: View {
    let projectId: String

    @State private var board: FactoryBoard?
    @State private var isLoading = true
    @State private var failed = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if let board {
                    section("Needs you", board.summary.needsYou)
                    section("Now", board.summary.now)
                    section("Next", board.summary.next)
                    ForEach(board.waves) { wave in
                        section("Wave \(wave.depth + 1)", wave.nodes)
                    }
                } else if isLoading {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if failed {
                    ProjectInlineError(text: "The Factory engine isn't running on this computer.") { Task { await load() } }
                } else {
                    ProjectCardNote("No campaign yet. Its Board and Flow show here once work runs through the Factory.")
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await load() }
    }

    @ViewBuilder
    private func section(_ title: String, _ nodes: [FactoryNodeCard]) -> some View {
        if !nodes.isEmpty {
            ProjectPaneHeading(title)
            ForEach(nodes) { ProjectNodeRow(card: $0) }
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            if let campaign = try await BotProjectClient.shared.campaign(projectId: projectId) {
                board = try await BotProjectClient.shared.board(campaignId: campaign.id)
            } else {
                board = nil
            }
            failed = false
        } catch is CancellationError {
        } catch {
            failed = true
        }
    }
}

/// Flow: the DAG's nodes by depth, with what each waits on (ProjectFlowView `list`).
struct ProjectFlowPane: View {
    let projectId: String

    @State private var flow: FactoryFlow?
    @State private var isLoading = true
    @State private var failed = false

    private var levels: [(depth: Int, nodes: [FactoryNodeCard])] {
        guard let flow else { return [] }
        let grouped = Dictionary(grouping: flow.nodes) { $0.depth ?? 0 }
        return grouped.keys.sorted().map { ($0, grouped[$0] ?? []) }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if let flow {
                    Text(flow.title)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    ForEach(levels, id: \.depth) { level in
                        ProjectPaneHeading("Step \(level.depth + 1)")
                        ForEach(level.nodes) { node in
                            ProjectNodeRow(card: node)
                            let waits = flow.edges.filter { $0.to == node.nodeId }.compactMap { edge in
                                flow.nodes.first { $0.nodeId == edge.from }?.title
                            }
                            if !waits.isEmpty {
                                ProjectCardNote("After \(waits.joined(separator: ", "))").padding(.leading, 15)
                            }
                        }
                    }
                } else if isLoading {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if failed {
                    ProjectInlineError(text: "The Factory engine isn't running on this computer.") { Task { await load() } }
                } else {
                    ProjectCardNote("No campaign yet. Its Board and Flow show here once work runs through the Factory.")
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            guard let campaign = try await BotProjectClient.shared.campaign(projectId: projectId) else {
                flow = nil
                failed = false
                return
            }
            let board = try await BotProjectClient.shared.board(campaignId: campaign.id)
            if let dagId = board.waves.flatMap(\.nodes).first?.dagId {
                flow = try await BotProjectClient.shared.flow(dagId: dagId)
            } else {
                flow = nil
            }
            failed = false
        } catch is CancellationError {
        } catch {
            failed = true
        }
    }
}

/// Team lanes on the phone (ProjectTeamGraph `stacked`): each bot with what
/// it's on, and who hands off to whom.
struct ProjectTeamPane: View {
    let botIds: [String]

    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var agents: [FactoryAgent] = []
    @State private var edges: [FactoryTeam.Edge] = []
    @State private var isLoading = true
    @State private var failed = false

    private func name(_ id: String) -> String {
        store.bot(id)?.botDisplayName ?? agents.first { $0.id == id }?.name ?? id
    }

    private func edgeWord(_ kind: String) -> String {
        switch kind {
        case "delegates_to": return "hands off to"
        case "checks": return "checks"
        case "directs": return "directs"
        default: return kind
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if isLoading && agents.isEmpty {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if agents.isEmpty {
                    ProjectCardNote(failed ? "Team graph: the Factory engine isn't running on this computer."
                                    : "Team graph: these bots aren't in the Factory yet.")
                        .padding(.bottom, 8)
                    ForEach(botIds.compactMap { store.bot($0) }) { bot in
                        Button { navigator.open(.page(bot.id)) } label: {
                            HStack(spacing: 8) {
                                BotAvatarView(bot: bot, size: 22)
                                BotNameText(bot.botDisplayName, size: 13)
                                Spacer(minLength: 0)
                            }
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                } else {
                    ForEach(agents) { agent in
                        Button {
                            if let node = agent.currentNode {
                                navigator.open(.projectNode(dagId: node.dagId, nodeId: node.nodeId, title: node.title))
                            } else {
                                navigator.open(.page(agent.id))
                            }
                        } label: {
                            ProjectAgentLabel(agent: agent)
                        }
                        .buttonStyle(.plain)
                    }
                    if !edges.isEmpty {
                        ProjectPaneHeading("Hand-offs")
                        ForEach(edges, id: \.self) { edge in
                            Text("\(name(edge.from)) \(edgeWord(edge.kind)) \(name(edge.to))")
                                .font(.system(size: 12.5))
                                .foregroundColor(Color("TextSecondary"))
                                .padding(.vertical, 4)
                        }
                    }
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            let team = try await BotProjectClient.shared.team(botIds: botIds)
            agents = team.agents
            edges = team.edges
            failed = false
        } catch is CancellationError {
        } catch {
            failed = true
        }
    }
}

/// A Factory node (factoryNodePane): status, intent, requirements, proof, progress.
struct ProjectNodePageView: View {
    let dagId: String
    let nodeId: String
    let title: String

    @State private var page: FactoryNodePage?
    @State private var isLoading = true
    @State private var error: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Text(page?.card.title ?? title)
                    .font(.system(size: 22, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                if let page {
                    Text([page.card.statusLabel, page.card.assignee].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                    if let intent = page.spec?.intent, !intent.isEmpty {
                        Text(intent)
                            .font(.system(size: 13.5))
                            .foregroundColor(Color("TextPrimary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let requirements = page.spec?.miniRequirements, !requirements.isEmpty {
                        ProjectPaneHeading("Requirements")
                        ForEach(requirements, id: \.self) { line in
                            Text("• \(line)").font(.system(size: 13)).foregroundColor(Color("TextPrimary"))
                        }
                    }
                    if let proof = page.spec?.proofContract, !proof.isEmpty {
                        ProjectPaneHeading("Proof")
                        ForEach(proof, id: \.self) { line in
                            Label(line.line, systemImage: line.checkedBy == nil ? "circle" : "checkmark.circle.fill")
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextPrimary"))
                        }
                    }
                    if let progress = page.progressMd, !progress.isEmpty {
                        ProjectPaneHeading("Progress")
                        Text(progress).font(.system(size: 13)).foregroundColor(Color("TextPrimary"))
                    }
                } else if isLoading {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if let error {
                    ProjectInlineError(text: error) { Task { await load() } }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Node")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            page = try await BotProjectClient.shared.node(dagId: dagId, nodeId: nodeId)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = "Couldn't load this node. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}

// MARK: - Sheets

/// Project settings (P-settings): Appearance and the coordinator's instructions.
struct ProjectSettingsSheet: View {
    let projectId: String
    let scope: BackdropScope
    let colors: [String?]
    let note: String
    let placeholder: String

    @Environment(\.dismiss) private var dismiss
    @State private var instructions = ""
    @State private var loaded = false
    @State private var saving = false
    @State private var error: String?
    @State private var appearanceOpen = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Appearance") {
                    Button("Background") { appearanceOpen = true }
                        .foregroundColor(Color("TextPrimary"))
                }
                Section {
                    if loaded {
                        TextField(placeholder, text: $instructions, axis: .vertical)
                            .lineLimit(5...12)
                    } else {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                } header: {
                    Text("Coordinator instructions")
                } footer: {
                    Text(note)
                }
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle("Project settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving…" : "Done") { save() }
                        .disabled(saving || !loaded)
                }
            }
            .sheet(isPresented: $appearanceOpen) {
                StageBackdropScopeSheet(scope: scope, title: "Customize appearance", colors: colors)
            }
            .task {
                instructions = (try? await BotProjectClient.shared.instructions(projectId: projectId)) ?? ""
                loaded = true
            }
        }
    }

    private func save() {
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                try await BotProjectClient.shared.saveInstructions(projectId: projectId, instructions: instructions)
                dismiss()
            } catch {
                self.error = "Couldn't save the instructions. Try again."
            }
        }
    }
}

struct ProjectEditDetailsSheet: View {
    let onSave: @MainActor (String, String) async throws -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var title: String
    @State private var details: String
    @State private var saving = false
    @State private var error: String?

    init(title: String, details: String, onSave: @escaping @MainActor (String, String) async throws -> Void) {
        self.onSave = onSave
        _title = State(initialValue: title)
        _details = State(initialValue: details)
    }

    private var trimmedTitle: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        NavigationStack {
            Form {
                Section("Name") {
                    TextField("Name this project", text: $title)
                }
                Section("Description") {
                    TextField("What is this project about?", text: $details, axis: .vertical)
                        .lineLimit(3...8)
                }
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle("Edit details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving…" : "Save") {
                        saving = true
                        error = nil
                        Task {
                            defer { saving = false }
                            do {
                                try await onSave(trimmedTitle, details.trimmingCharacters(in: .whitespacesAndNewlines))
                                dismiss()
                            } catch {
                                self.error = "Couldn't save the project details. Try again."
                            }
                        }
                    }
                    .disabled(saving || trimmedTitle.isEmpty)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Save as template (SaveAsTemplateDialog + from-live.ts `projectToTemplate`):
/// the team's bots and the plan from the project's task threads.
struct ProjectSaveTemplateSheet: View {
    let projectId: String
    let title: String
    let details: String?
    let botIds: [String]
    let threads: [BotThread]

    @ObservedObject private var store = BotModeStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var visibility = "private"
    @State private var saving = false
    @State private var error: String?

    init(projectId: String, title: String, details: String?, botIds: [String], threads: [BotThread]) {
        self.projectId = projectId
        self.title = title
        self.details = details
        self.botIds = botIds
        self.threads = threads
        _name = State(initialValue: title)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Name") { TextField("Template name", text: $name) }
                Section {
                    Picker("Who can use it", selection: $visibility) {
                        Text("Only me").tag("private")
                        Text("My organization").tag("org")
                    }
                    .pickerStyle(.segmented)
                } footer: {
                    Text("Saves the team's bots and this project's plan to Templates. Conversations and files stay out of it.")
                }
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle("Save as template")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving…" : "Save") { save() }
                        .disabled(saving || name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func save() {
        saving = true
        error = nil
        let payload = template()
        let chosen = visibility
        Task {
            defer { saving = false }
            do {
                try await BotProjectClient.shared.saveTeamTemplate(visibility: chosen, template: payload)
                dismiss()
            } catch {
                self.error = "Couldn't save the template: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func slug(_ text: String) -> String {
        let cleaned = BotModeStore.slugify(text)
        return cleaned.isEmpty ? "template" : String(cleaned.prefix(40))
    }

    private func unique(_ prefix: String, _ text: String) -> String {
        "\(prefix)-\(slug(text))-\(UUID().uuidString.prefix(6).lowercased())"
    }

    private func strings(_ values: [String]) -> JSONValue { .array(values.map { .string($0) }) }

    private func template() -> [String: JSONValue] {
        let team = botIds.compactMap { store.bot($0) }
        var templateIdByBot: [String: String] = [:]
        let bots: [JSONValue] = team.map { bot in
            let botName = bot.botDisplayName
            let id = unique("bot", botName)
            templateIdByBot[bot.id] = id
            let starter = "Introduce yourself in two lines and say what you'd do first as \(botName)."
            return .object([
                "id": .string(id), "version": .string("1.0.0"), "source": .string("mine"), "category": .string("ops"),
                "name": .string(botName), "tagline": .string(bot.botTagline), "description": .string(bot.description ?? ""),
                "color": .string("#6b7280"), "instructions": .string(bot.systemPrompt ?? ""),
                "model": .object(["tier": .string("auto")]),
                "tools": .array([]), "skills": .array([]), "connections": .array([]), "routines": .array([]),
                "computer": .string("none"), "approvals": .array([]), "memorySeed": .array([]), "starterPrompts": .array([]),
                "firstTask": .object(["prompt": .string(starter), "expect": .string("A short reply in the role")]),
                "keywords": strings([botName.lowercased()]),
            ])
        }
        let planThreads = threads.filter { $0.kind == "task" && !$0.incognito && templateIdByBot[$0.botId] != nil }
        var stepIds: [String: String] = [:]
        var seen = Set<String>()
        for thread in planThreads {
            var id = slug(thread.title)
            while seen.contains(id) { id += "-2" }
            seen.insert(id)
            stepIds[thread.id] = id
        }
        let plan: [JSONValue] = planThreads.map { thread in
            var step: [String: JSONValue] = [
                "id": .string(stepIds[thread.id] ?? thread.id),
                "title": .string(thread.title),
                "bot": .string(templateIdByBot[thread.botId] ?? ""),
                "objective": .string(thread.title),
                "dependsOn": strings((thread.dependsOn ?? []).compactMap { stepIds[$0] }),
            ]
            if let todo = thread.todo, !todo.isEmpty { step["checklist"] = strings(todo.map(\.text)) }
            return .object(step)
        }
        let brief = details?.isEmpty == false ? details! : title
        let finalName = name.trimmingCharacters(in: .whitespaces)
        let teamTemplate: [String: JSONValue] = [
            "id": .string(unique("team", title)), "version": .string("1.0.0"), "source": .string("mine"),
            "category": .string("ops"), "title": .string(finalName),
            "tagline": .string(details?.isEmpty == false ? details! : "\(team.count) bots, saved from a project"),
            "description": .string(details ?? ""),
            "bots": strings(team.compactMap { templateIdByBot[$0.id] }),
            "brief": .object(["label": .string("What should the team do?"), "placeholder": .string(brief)]),
            "plan": .array(plan), "sharedContext": .array([]), "routines": .array([]), "exampleGoal": .string(brief),
        ]
        return ["team": .object(teamTemplate), "bots": .array(bots)]
    }
}

/// Project memory (ProjectMemoryModal): the facts learned in this project's sessions.
struct ProjectMemorySheet: View {
    let sessionIds: Set<String>

    @Environment(\.dismiss) private var dismiss
    @State private var facts: [String] = []
    @State private var isLoading = true

    var body: some View {
        NavigationStack {
            Group {
                if isLoading {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if facts.isEmpty {
                    VStack(spacing: 8) {
                        Text("No memory yet")
                            .font(.system(size: 17, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                        Text("Facts from this project's chats show here after a few turns.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextTertiary"))
                            .multilineTextAlignment(.center)
                    }
                    .padding(24)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    List(facts, id: \.self) { fact in
                        Text(fact).font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                    }
                }
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Memory")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .task {
                facts = (try? await BotProjectClient.shared.memoryFacts(sessionIds: sessionIds)) ?? []
                isLoading = false
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Every thread waiting on you (ProjectNeedsYou's popover).
struct ProjectNeedsYouSheet: View {
    let threads: [BotThread]
    let onOpen: (BotThread) -> Void

    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        NavigationStack {
            List(threads) { thread in
                Button { onOpen(thread) } label: {
                    HStack(spacing: 10) {
                        if let bot = store.bot(thread.botId) { BotAvatarView(bot: bot, size: 22) }
                        VStack(alignment: .leading, spacing: 1) {
                            Text(thread.title)
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            Text([store.bot(thread.botId)?.botDisplayName, thread.statusLine ?? "Waiting on you"]
                                .compactMap { $0 }.joined(separator: " · "))
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextTertiary"))
                                .lineLimit(1)
                        }
                    }
                    .frame(minHeight: 44)
                }
            }
            .navigationTitle("Needs you")
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
    }
}

/// "Add a bot" (ProjectBotLauncher): any other bot on a new thread in this project.
struct ProjectAddBotSheet: View {
    let bots: [AgentRecord]
    let onPick: (AgentRecord) -> Void

    var body: some View {
        NavigationStack {
            List(bots) { bot in
                Button { onPick(bot) } label: {
                    HStack(spacing: 10) {
                        BotAvatarView(bot: bot, size: 24)
                        BotNameText(bot.botDisplayName, size: 14)
                    }
                    .frame(minHeight: 44)
                }
            }
            .navigationTitle("Starts a new thread in this project")
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
    }
}

/// The status page (ProjectStatusPanel, full view): state, next action, the
/// sections, the log. Edits go in as "you".
struct ProjectStatusSheet: View {
    let projectId: String

    @Environment(\.dismiss) private var dismiss
    @State private var status: ProjectStatusRecord?
    @State private var error: String?
    @State private var saving = false
    @State private var editing: String?
    @State private var draft = ""

    private var closed: Bool { status?.lifecycle == "closed" }

    var body: some View {
        NavigationStack {
            Group {
                if let status {
                    form(status)
                } else if let error {
                    ProjectInlineError(text: "Couldn't load the status page. \(error)") { Task { await load() } }
                        .padding(16)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .navigationTitle("Status page")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .task { await load() }
        }
    }

    private func form(_ status: ProjectStatusRecord) -> some View {
        Form {
            Section {
                if closed {
                    ProjectStateChip(state: status.state, closed: true)
                } else {
                    Picker("State", selection: Binding(get: { status.state }, set: { setState($0) })) {
                        ForEach(ProjectStatusRecord.stateLabels, id: \.key) { Text($0.label).tag($0.key) }
                    }
                    .disabled(saving)
                }
                if !status.nextAction.isEmpty {
                    Text(status.nextAction).font(.system(size: 14))
                }
            }
            ForEach(ProjectStatusRecord.sectionOrder, id: \.key) { section in
                Section(section.label) {
                    if editing == section.key {
                        TextField(section.label, text: $draft, axis: .vertical)
                            .lineLimit(3...10)
                        HStack {
                            Button("Cancel") { editing = nil }
                                .buttonStyle(.borderless)
                            Spacer()
                            Button(saving ? "Saving…" : "Save") { saveSection(section.key) }
                                .buttonStyle(.borderless)
                                .disabled(saving)
                        }
                    } else {
                        let text = status.sections[section.key]?.text ?? ""
                        Button {
                            guard !closed else { return }
                            draft = text
                            editing = section.key
                        } label: {
                            Text(text.isEmpty ? "Add" : text)
                                .font(.system(size: 14))
                                .foregroundColor(text.isEmpty ? Color("TextTertiary") : Color("TextPrimary"))
                                .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            Section("Log") {
                if status.log.isEmpty {
                    Text("Nothing logged yet.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                }
                ForEach(Array(status.log.prefix(10).enumerated()), id: \.offset) { _, entry in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.note).font(.system(size: 13))
                        Text("\(entry.by) · \(BotTime.relative(entry.at))")
                            .font(.system(size: 11))
                            .foregroundColor(Color("TextTertiary"))
                    }
                }
            }
            if let error {
                Text(error).font(.caption).foregroundColor(Theme.statusError)
            }
        }
    }

    private func load() async {
        do {
            status = try await BotProjectClient.shared.status(projectId: projectId)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func setState(_ state: String) {
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                try await BotProjectClient.shared.patchStatus(projectId: projectId, state: state)
                await load()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private func saveSection(_ key: String) {
        let text = draft
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                try await BotProjectClient.shared.patchStatus(projectId: projectId, section: key, text: text)
                editing = nil
                await load()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}
