import SwiftUI

/// Threads on the phone (allternit-ai src/components/dispatch/
/// FabricThreadsPage.tsx → ThreadsPanel): every bot's threads grouped by
/// what they need — Waiting on you, Working, Queued, Idle, Resolved — or one
/// bot's threads from its page. Polls every 15s like the web.
struct BotThreadsView: View {
    let botId: String?

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    /// List · Grid · Deck, like the web's Threads tab (BotThreadsOverview).
    @AppStorage("allternit.session-overview.layout.bot-threads") private var layoutRaw = OverviewLayout.list.rawValue

    private var layout: OverviewLayout { OverviewLayout(stored: layoutRaw) ?? .list }

    /// The open threads as cards; resolved ones stay out (BotThreadsOverview).
    private var cards: [OverviewCardItem] {
        let all = botId.map { id in store.threads.filter { $0.botId == id } } ?? store.threads
        return all.filter { !$0.incognito && $0.group != "resolved" }
            .sorted(by: BotModeStore.threadOrder)
            .map { thread in
                let tone: OverviewCardItem.Tone = {
                    switch thread.group {
                    case "waiting": return .waiting
                    case "working": return .working
                    default: return .idle
                    }
                }()
                return OverviewCardItem(
                    id: thread.id,
                    title: thread.title,
                    group: botId == nil ? store.bot(thread.botId)?.botDisplayName : nil,
                    status: tone == .idle ? (thread.group == "queued" ? "Queued" : nil) : BotThreadGroup(rawValue: thread.group)?.title,
                    tone: tone,
                    preview: thread.statusLine,
                    meta: BotTime.relative(thread.lastActivityAt),
                    footer: thread.progressLabel
                )
            }
    }

    private func open(_ thread: BotThread) {
        navigator.openThread(thread)
    }

    var body: some View {
        ScrollView {
            let waiting = botId.map { store.waitingCount(for: $0) } ?? store.waitingCount
            HStack(alignment: .bottom) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Threads")
                        .font(.system(size: 21, weight: .semibold, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                    Text(waiting == 0 ? "Nothing is waiting on you."
                         : waiting == 1 ? "1 thread is waiting on you." : "\(waiting) threads are waiting on you.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer()
                OverviewLayoutSwitch(raw: $layoutRaw, fallback: .list)
            }
            .padding(.horizontal, 16)
            .padding(.top, 16)
            .padding(.bottom, 4)
            if layout == .list {
                BotThreadsPanel(botId: botId, greeting: nil, onOpen: open)
                    .padding(.bottom, 8)
            } else if cards.isEmpty {
                Text(store.isLoadingThreads ? "Loading threads…" : "No open threads. Start one from the composer.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 24)
            } else {
                let accent = colors.hex(for: botId.flatMap { store.bot($0) }).map { Color(hex: $0) } ?? Theme.accentChat
                OverviewCards(items: cards, layout: layout, accent: accent) { item in
                    if let thread = store.threads.first(where: { $0.id == item.id }), let bot = store.bot(thread.botId), botId == nil {
                        BotStatusAvatar(bot: bot, size: layout.faceSize, tiled: true)
                    }
                } onOpen: { item in
                    if let thread = store.threads.first(where: { $0.id == item.id }) { open(thread) }
                }
                .padding(.vertical, 8)
            }
        }
        // One bot's threads wear its colour; all threads stay neutral.
        .background { BotWashedBackground(color: .some(colors.hex(for: botId.flatMap { store.bot($0) }))).ignoresSafeArea() }
        .claimsBotTint(botId)
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
    /// Resolved rows get a Reopen (ThreadsPanel `onReopen`).
    var onReopen: ((BotThread) -> Void)? = nil

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
                    HStack(spacing: 4) {
                        Button { onOpen(thread) } label: {
                            BotThreadRow(thread: thread, showsBot: botId == nil)
                        }
                        .buttonStyle(.plain)
                        if group == .resolved, let onReopen {
                            Button("Reopen") { onReopen(thread) }
                                .font(.system(size: 12, weight: .medium))
                                .foregroundColor(Color("TextPrimary"))
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                                .buttonStyle(.plain)
                        }
                    }
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
                HStack(spacing: 6) {
                    if thread.kind == "task" {
                        Text("TASK")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundColor(Theme.statusInfo)
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(RoundedRectangle(cornerRadius: 3).fill(Theme.statusInfo.opacity(0.14)))
                    }
                    Text(thread.title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                }
                if let line = thread.statusLine, !line.isEmpty {
                    Text(line)
                        .font(.system(size: 11.5))
                        .foregroundColor(group == .waiting ? Theme.statusWarning : Color("TextTertiary"))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 6)
            HStack(spacing: 8) {
                if let progress = thread.progressLabel {
                    HStack(spacing: 4) {
                        // The thread's progress ring (ThreadProgressRing).
                        ZStack {
                            Circle().stroke(Color("FillNeutral"), lineWidth: 2)
                            Circle()
                                .trim(from: 0, to: CGFloat(thread.progress[0]) / CGFloat(max(thread.progress[1], 1)))
                                .stroke(group.color, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                                .rotationEffect(.degrees(-90))
                        }
                        .frame(width: 13, height: 13)
                        Text(progress)
                    }
                }
                Text(BotTime.relative(thread.lastActivityAt))
            }
            .font(.system(size: 11).monospacedDigit())
            .foregroundColor(Color("TextTertiary"))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
        .frame(minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}
