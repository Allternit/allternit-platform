import SwiftUI

/// Threads on the phone (allternit-ai src/components/dispatch/
/// FabricThreadsPage.tsx → ThreadsPanel): every bot's threads grouped by
/// what they need — Waiting on you, Working, Queued, Idle, Resolved — or one
/// bot's threads from its page. Polls every 15s like the web.
struct BotThreadsView: View {
    let botId: String?

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared

    private var threads: [BotThread] {
        let all = botId.map { id in store.threads.filter { $0.botId == id } } ?? store.threads
        return all.sorted(by: BotModeStore.threadOrder)
    }

    var body: some View {
        let grouped = Dictionary(grouping: threads, by: \.group)
        List {
            if store.isLoadingThreads && store.threads.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let error = store.threadsError, store.threads.isEmpty {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load threads",
                                  message: error, actionTitle: "Retry", action: { Task { await store.refreshThreads() } })
            } else if threads.isEmpty {
                FriendlyStateView(style: .empty, icon: "bubble.left.and.text.bubble.right", title: "No threads yet",
                                  message: "Threads show up here by status as you start work with your bots.")
            } else {
                ForEach(BotThreadGroup.allCases, id: \.self) { group in
                    if let rows = grouped[group.rawValue], !rows.isEmpty {
                        Section {
                            ForEach(rows) { thread in
                                BotThreadRow(thread: thread, showsBot: botId == nil)
                            }
                        } header: {
                            HStack {
                                Text(group.title)
                                if group == .waiting { WaitingBadge(count: rows.count) }
                            }
                        }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(botId.flatMap { store.bot($0)?.botDisplayName }.map { "\($0) threads" } ?? "Threads")
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

/// One thread: bot avatar, title, status line, progress; waiting threads
/// carry Approve / Decline (threads/:id/approve|decline).
struct BotThreadRow: View {
    let thread: BotThread
    var showsBot = true

    @ObservedObject private var store = BotModeStore.shared
    @State private var isActing = false
    @State private var actionError: String? = nil

    var body: some View {
        let bot = store.bot(thread.botId)
        let group = BotThreadGroup(rawValue: thread.group) ?? .idle
        VStack(alignment: .leading, spacing: 8) {
            Group {
                if let sessionId = thread.currentSessionId {
                    NavigationLink(value: BotRoute.chat(botId: thread.botId, sessionId: sessionId)) { label(bot, group) }
                } else {
                    label(bot, group)
                }
            }
            if group == .waiting {
                HStack(spacing: 10) {
                    Button("Approve") { act(approve: true) }
                        .buttonStyle(.borderedProminent)
                    Button("Decline") { act(approve: false) }
                        .buttonStyle(.bordered)
                    if isActing { ProgressView() }
                }
                .font(.subheadline)
                .disabled(isActing)
                if let actionError {
                    Text(actionError).font(.caption).foregroundColor(Theme.statusError)
                }
            }
        }
    }

    private func label(_ bot: AgentRecord?, _ group: BotThreadGroup) -> some View {
        HStack(spacing: 10) {
            if showsBot, let bot {
                BotAvatarView(bot: bot, size: 32)
            } else {
                Circle().fill(group.color).frame(width: 8, height: 8)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(thread.title)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text([showsBot ? bot?.botDisplayName : nil, thread.statusLine, thread.progressLabel]
                    .compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundColor(group == .waiting ? Theme.statusWarning : Color("TextSecondary"))
                    .lineLimit(2)
            }
            Spacer(minLength: 4)
            Text(BotTime.relative(thread.lastActivityAt))
                .font(.caption2)
                .foregroundColor(Color("TextTertiary"))
        }
    }

    private func act(approve: Bool) {
        isActing = true
        actionError = nil
        Task {
            defer { isActing = false }
            do {
                if approve { try await store.approve(thread) } else { try await store.decline(thread) }
            } catch {
                actionError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
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
                        BotThreadRow(thread: thread)
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
