import SwiftUI

/// Threads on the phone (allternit-ai src/components/dispatch/
/// FabricThreadsPage.tsx → ThreadsPanel): every bot's threads grouped by
/// what they need — Waiting on you, Working, Queued, Idle, Resolved — or one
/// bot's threads from its page. Polls every 15s like the web.
struct BotThreadsView: View {
    let botId: String?

    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator

    var body: some View {
        ScrollView {
            BotThreadsPanel(botId: botId, greeting: "Threads") { thread in
                if let sessionId = thread.currentSessionId {
                    navigator.open(.chat(botId: thread.botId, sessionId: sessionId))
                }
            }
            .padding(.vertical, 8)
        }
        .background(Color("BgPrimary"))
        .navigationTitle(botId.flatMap { store.bot($0)?.botDisplayName } ?? "Threads")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await store.refreshThreads() }
        .task {
            await store.refreshThreads()
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 15_000_000_000)
                await store.refreshThreads()
            }
        }
    }
}

/// The Threads panel (components/threads/ThreadsPanel.tsx): a serif greeting
/// with what waits on you, then collapsible groups with counts; Resolved
/// starts folded. Rows open through `onOpen`.
struct BotThreadsPanel: View {
    /// One bot's threads (its avatar is then hidden), or every bot's.
    let botId: String?
    var greeting: String? = "Threads"
    /// Threads to show instead of the store's (a project's).
    var threadsOverride: [BotThread]? = nil
    let onOpen: (BotThread) -> Void

    @ObservedObject private var store = BotModeStore.shared
    @State private var collapsed: Set<String> = ["resolved"]

    private var threads: [BotThread] {
        let all = threadsOverride ?? (botId.map { id in store.threads.filter { $0.botId == id } } ?? store.threads)
        return all.filter { !$0.incognito }.sorted(by: BotModeStore.threadOrder)
    }

    private func waitingLine(_ count: Int) -> String {
        if count == 0 { return "Nothing is waiting on you." }
        return count == 1 ? "1 thread is waiting on you." : "\(count) threads are waiting on you."
    }

    var body: some View {
        let rows = threads
        let grouped = Dictionary(grouping: rows, by: \.group)
        VStack(alignment: .leading, spacing: 0) {
            if let greeting {
                VStack(alignment: .leading, spacing: 2) {
                    Text(greeting)
                        .font(.system(size: 21, weight: .semibold, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                    Text(waitingLine(rows.filter { $0.group == "waiting" }.count))
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 8)
            }
            if rows.isEmpty {
                if store.isLoadingThreads && threadsOverride == nil {
                    ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
                } else if let error = store.threadsError, threadsOverride == nil {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load threads",
                                      message: error, actionTitle: "Retry", action: { Task { await store.refreshThreads() } })
                } else {
                    Text("No threads yet. Start one from the composer.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 16)
                        .padding(.vertical, 24)
                }
            }
            ForEach(BotThreadGroup.allCases, id: \.self) { group in
                if let items = grouped[group.rawValue], !items.isEmpty {
                    section(group, items)
                }
            }
        }
    }

    private func section(_ group: BotThreadGroup, _ items: [BotThread]) -> some View {
        let folded = collapsed.contains(group.rawValue)
        return VStack(alignment: .leading, spacing: 2) {
            Button {
                withAnimation(.easeOut(duration: 0.16)) {
                    if folded { collapsed.remove(group.rawValue) } else { collapsed.insert(group.rawValue) }
                }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.down")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(folded ? -90 : 0))
                    Text(group.title)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("\(items.count)")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("TextTertiary"))
                    Spacer()
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(group.title), \(items.count)")
            .accessibilityValue(folded ? "Collapsed" : "Expanded")
            if !folded {
                ForEach(items) { thread in
                    Button { onOpen(thread) } label: {
                        BotThreadRow(thread: thread, showsBot: botId == nil)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.top, 8)
        .padding(.bottom, 4)
    }
}

enum BotThreadGroup: String, CaseIterable {
    case waiting, working, queued, idle, resolved

    var title: String {
        switch self {
        case .waiting: return "Waiting on you"
        case .working: return "Working"
        case .queued: return "Queued"
        case .idle: return "Idle"
        case .resolved: return "Resolved"
        }
    }

    var color: Color {
        switch self {
        case .waiting: return Theme.statusWarning
        case .working: return Theme.statusSuccess
        case .queued: return Theme.statusInfo
        case .idle, .resolved: return Color("TextTertiary")
        }
    }
}

/// One thread row (ThreadsPanel): bot avatar, title, the live status line,
/// the k/n progress and the time.
struct BotThreadRow: View {
    let thread: BotThread
    var showsBot = true

    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        let bot = store.bot(thread.botId)
        let group = BotThreadGroup(rawValue: thread.group) ?? .idle
        HStack(spacing: 8) {
            if showsBot, let bot {
                BotAvatarView(bot: bot, size: 22)
            } else {
                Circle().fill(group.color).frame(width: 8, height: 8).frame(width: 22)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(thread.title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if let line = thread.statusLine, !line.isEmpty {
                    Text(line)
                        .font(.system(size: 11.5))
                        .foregroundColor(group == .waiting ? Theme.statusWarning : Color("TextTertiary"))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 6)
            HStack(spacing: 8) {
                if let progress = thread.progressLabel { Text(progress) }
                Text(BotTime.relative(thread.lastActivityAt))
            }
            .font(.system(size: 11).monospacedDigit())
            .foregroundColor(Color("TextTertiary"))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// A bot project's home (views/bots/ProjectHomeView.tsx, phone subset): its
/// team and its threads by status.
struct BotProjectHomeView: View {
    let project: BotProjectSummary

    @ObservedObject private var store = BotModeStore.shared
    @State private var threads: [BotThread] = []
    @State private var isLoading = true
    @State private var error: String? = nil

    var body: some View {
        List {
            if let description = project.description, !description.isEmpty {
                Section { Text(description).foregroundColor(Color("TextSecondary")) }
            }
            let team = project.botIds.compactMap { store.bot($0) }
            if !team.isEmpty {
                Section("Team") {
                    ForEach(team) { bot in
                        NavigationLink(value: BotRoute.chat(botId: bot.id, sessionId: nil)) {
                            HStack(spacing: 10) {
                                BotAvatarView(bot: bot, size: 30)
                                Text(bot.botDisplayName)
                            }
                        }
                    }
                }
            }
            Section("Threads") {
                if isLoading && threads.isEmpty {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let error {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load threads",
                                      message: error, actionTitle: "Retry", action: { Task { await load() } })
                } else if threads.isEmpty {
                    Text("No threads in this project yet.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(threads.sorted(by: BotModeStore.threadOrder)) { thread in
                        if let sessionId = thread.currentSessionId {
                            NavigationLink(value: BotRoute.chat(botId: thread.botId, sessionId: sessionId)) {
                                BotThreadRow(thread: thread)
                            }
                        } else {
                            BotThreadRow(thread: thread)
                        }
                    }
                }
            }
        }
        .navigationTitle(project.title)
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            threads = try await BotModeClient.shared.listProjectThreads(projectId: project.id)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
