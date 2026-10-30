import SwiftUI

/// A bot's page (allternit-ai src/views/bots/home/PhoneBotPage.tsx): the
/// banner with the big avatar, name, role line and Edit, then one card of
/// rows with live counts — Threads · Tasks · Routines · Artifacts · Memory ·
/// Computer · Channels. Each row opens the view that already exists for it:
/// in a bot thread Threads and Tasks switch the thread's top deck, the rest
/// push the bot screen on that tab.
struct BotPageView: View {
    let bot: AgentRecord
    /// In a thread: switch its top deck instead of pushing.
    var onOpenThreads: (() -> Void)? = nil
    var onOpenTasks: (() -> Void)? = nil

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var desktop = BotDesktopStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var routinesOn: Int? = nil
    @State private var channelNames: [String]? = nil

    /// The bot's own colour (given, or read from its art).
    private var accent: Color? {
        colors.hex(for: bot).map { Color(hex: $0) }
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                banner
                rows
            }
            .padding(.bottom, 32)
        }
        // The page wears the bot's wash (the phone bot page's tint); the
        // drawer follows while it is open.
        .background { BotWashedBackground(color: .some(colors.hex(for: bot))).ignoresSafeArea() }
        .claimsBotTint(bot.id)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            desktop.fetchStatusIfNeeded(botId: bot.id)
            await loadCounts()
        }
    }

    private var banner: some View {
        VStack(spacing: 0) {
            BotAvatarView(bot: bot, size: 92, live: true)
                .padding(4)
                .background(Circle().fill((accent ?? .clear).opacity(0.16)))
            BotNameText(bot.botDisplayName, size: 22)
                .font(.system(size: 22, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .padding(.top, 8)
            if !roleLine.isEmpty {
                Text(roleLine)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
                    .frame(maxWidth: 320)
                    .padding(.top, 2)
            }
            Button { navigator.open(.settings(bot.id)) } label: {
                Label("Edit", systemImage: "pencil")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 16)
                    .frame(minHeight: 36)
                    .background(Capsule().fill(Color("BgPanel")))
                    .overlay(Capsule().stroke(Color("BorderSubtle")))
            }
            .buttonStyle(.plain)
            .padding(.top, 10)
        }
        .padding(.horizontal, 14)
        .padding(.top, 16)
        .padding(.bottom, 20)
        .frame(maxWidth: .infinity)
        .background(
            // A bot with a colour already wears it as the page wash.
            LinearGradient(colors: accent == nil ? [Color("TextPrimary").opacity(0.18), Color("TextPrimary").opacity(0.03)] : [.clear],
                           startPoint: .top, endPoint: .bottom)
        )
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private var roleLine: String {
        let model = bot.model.split(separator: "/").last.map(String.init)
        return [bot.botTagline, model].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    private var rows: some View {
        let working = store.workingCount(for: bot.id)
        let waiting = store.waitingCount(for: bot.id)
        return VStack(spacing: 0) {
            pageRow("Threads", icon: "bubble.left.and.bubble.right", value: "\(store.openThreadCount(for: bot.id)) open") {
                if let onOpenThreads { onOpenThreads() } else { navigator.open(.botHome(bot.id, .threads)) }
            }
            pageRow("Tasks", icon: "checklist",
                    value: waiting > 0 ? "\(working) working · \(waiting) need you" : "\(working) working") {
                if let onOpenTasks { onOpenTasks() } else { navigator.open(.tasks(bot.id)) }
            }
            pageRow("Routines", icon: "clock.arrow.circlepath", value: routinesOn.map { "\($0) on" }) {
                navigator.open(.botHome(bot.id, .routines))
            }
            pageRow("Artifacts", icon: "square.grid.2x2", value: nil) {
                navigator.open(.botHome(bot.id, .artifacts))
            }
            pageRow("Memory", icon: "brain", value: nil) {
                navigator.open(.botHome(bot.id, .memory))
            }
            pageRow("Computer", icon: "desktopcomputer", value: computerLabel) {
                navigator.open(.botComputer(bot.id))
            }
            pageRow("Channels", icon: "dot.radiowaves.left.and.right",
                    value: channelNames.map { $0.isEmpty ? "None" : $0.joined(separator: ", ") }, last: true) {
                // Settings holds the channels on the web; here they keep their own screen.
                navigator.open(.channels(bot.id))
            }
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
        .padding(.horizontal, 14)
    }

    /// web phoneComputerLabel: Running · Starting · Idle.
    private var computerLabel: String {
        switch desktop.entry(for: bot.id)?.status?.status {
        case .running?: return "Running"
        default: return "Idle"
        }
    }

    private func pageRow(_ label: String, icon: String, value: String?, last: Bool = false,
                         action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 17))
                    .frame(width: 22)
                    .foregroundColor(Color("TextSecondary"))
                Text(label)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 8)
                if let value {
                    Text(value)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 50)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) {
            if !last { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
        }
        .accessibilityLabel(value.map { "\(label), \($0)" } ?? label)
    }

    private func loadCounts() async {
        async let routines = try? AutomationClient.shared.listRoutines(recentRuns: 0)
        async let channels = try? BotModeClient.shared.listChannels(botId: bot.id)
        let (r, c) = await (routines, channels)
        routinesOn = r?.filter { $0.agentId == bot.id && $0.isEnabled }.count
        channelNames = c?.filter(\.enabled).map(\.label)
    }
}

// MARK: - Tasks

/// The bot's chats outside its main chat (PhoneBotPage "Tasks").
struct BotTasksView: View {
    let bot: AgentRecord
    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var sessions: [AgentSession] = []
    @State private var isLoading = true
    @State private var error: String? = nil
    @State private var isStarting = false
    @ObservedObject private var colors = BotColorStore.shared
    /// List · Grid · Deck, the same switch as the Bots home.
    @AppStorage("allternit.session-overview.layout.bot-tasks") private var layoutRaw = OverviewLayout.list.rawValue

    private var layout: OverviewLayout { OverviewLayout(stored: layoutRaw) ?? .list }

    private var cards: [OverviewCardItem] {
        sessions.map { session in
            let working = ShellSessionsStore.shared.isStreaming(session.id)
            return OverviewCardItem(
                id: session.id,
                title: session.name ?? "Untitled task",
                status: working ? "Working" : nil,
                tone: working ? .working : .idle,
                preview: session.description,
                meta: BotTime.relative(session.updatedAt),
                footer: "\(session.messageCount) message\(session.messageCount == 1 ? "" : "s")"
            )
        }
    }

    var body: some View {
        ScrollView {
            HStack(spacing: 10) {
                Button {
                    startThread()
                } label: {
                    Label(isStarting ? "Starting…" : "New task", systemImage: "plus")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .frame(height: 34)
                        .background(Capsule().fill(Color("FillNeutral")))
                }
                .buttonStyle(.plain)
                .disabled(isStarting)
                Spacer()
                OverviewLayoutSwitch(raw: $layoutRaw, fallback: .list)
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 8)
            if isLoading && sessions.isEmpty {
                ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
            } else if let error {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load tasks",
                                  message: error, actionTitle: "Retry", action: { Task { await load() } })
            } else if sessions.isEmpty {
                Text("No tasks yet. Start one and \(bot.botDisplayName) works on it in its own chat.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 32)
                    .padding(.vertical, 24)
            } else {
                OverviewCards(items: cards, layout: layout,
                              accent: colors.hex(for: bot).map { Color(hex: $0) } ?? Theme.accentChat) { _ in
                    Image(systemName: "checklist")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                } onOpen: { item in
                    navigator.open(.chat(botId: bot.id, sessionId: item.id))
                }
                .padding(.bottom, 16)
            }
        }
        .background { BotWashedBackground(color: .some(colors.hex(for: bot))).ignoresSafeArea() }
        .navigationTitle("Tasks")
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let main = store.canonicalChatIds[bot.id]
            sessions = try await AgentChatClient().listSessions()
                .filter { $0.agentId == bot.id && $0.id != main && !$0.ephemeral }
                .sorted { $0.updatedAt > $1.updatedAt }
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func startThread() {
        isStarting = true
        Task {
            defer { isStarting = false }
            do {
                let id = try await store.newThreadSession(for: bot, title: "New task")
                navigator.open(.chat(botId: bot.id, sessionId: id))
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}

// MARK: - Routines

struct BotRoutinesView: View {
    let bot: AgentRecord
    @State private var routines: [Routine] = []
    @State private var isLoading = true
    @State private var error: String? = nil
    /// The bot's scheduled Automation routines (bot-routine.service: `agent_id` = the bot).
    @State private var scheduled: [AutomationRoutine] = []
    @State private var editing: AutomationRoutine?
    @State private var saving = false
    @State private var saveError: String?

    var body: some View {
        List {
            if !scheduled.isEmpty {
                Section {
                    ForEach(scheduled) { r in
                        Button { saveError = nil; editing = r } label: {
                            HStack(spacing: 10) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(Self.title(r.name, bot: bot)).foregroundColor(Color("TextPrimary")).lineLimit(2)
                                    Text([RoutineRule(cron: r.scheduleExpression)?.label ?? r.scheduleExpression,
                                          r.isEnabled ? nil : "Paused"].compactMap { $0 }.joined(separator: " · "))
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                                Spacer(minLength: 8)
                                Image(systemName: "clock").foregroundColor(Color("TextSecondary"))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint("Change when it runs")
                    }
                } header: {
                    Text("On a schedule")
                }
            }
            if isLoading && routines.isEmpty && scheduled.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let error {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load routines",
                                  message: error, actionTitle: "Retry", action: { Task { await load() } })
            } else if routines.isEmpty && scheduled.isEmpty {
                Text("\(bot.botDisplayName) has no routines yet. Add one from Automation.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(routines) { routine in
                    NavigationLink {
                        RoutineDetailView(routine: routine)
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(routine.name).lineLimit(1)
                            Text([routine.schedule ?? routine.trigger, routine.state].compactMap { $0 }.joined(separator: " · "))
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
            }
        }
        .navigationTitle("Routines")
        .refreshable { await load() }
        .task { await load() }
        .sheet(item: $editing) { r in
            RoutineScheduleEditor(
                title: Self.title(r.name, bot: bot),
                initial: RoutineRule(cron: r.scheduleExpression) ?? RoutineRule(repeatKind: .weekdays, at: 9 * 60, days: RoutineRule.weekdays),
                saving: saving, error: saveError,
                onSave: { edit in save(r, rule: edit.rule) },
                onCancel: { editing = nil })
        }
    }

    /// "[bot:Name] Morning note" → "Morning note".
    private static func title(_ name: String, bot: AgentRecord) -> String {
        let prefix = "[bot:\(bot.botDisplayName)] "
        if name.hasPrefix(prefix) { return String(name.dropFirst(prefix.count)) }
        if name.hasPrefix("[bot:"), let end = name.firstIndex(of: "]") {
            return name[name.index(after: end)...].trimmingCharacters(in: .whitespaces)
        }
        return name
    }

    private func save(_ r: AutomationRoutine, rule: RoutineRule) {
        saving = true
        saveError = nil
        Task {
            defer { saving = false }
            do {
                try await AutomationClient.shared.updateSchedule(id: r.id, cron: rule.cron, timezone: TimeZone.current.identifier)
                editing = nil
                await load()
            } catch {
                saveError = "Couldn’t save the schedule: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        if let rows = try? await AutomationClient.shared.listRoutines(recentRuns: 0) {
            scheduled = rows.filter { $0.agentId == bot.id && $0.scheduleType == "cron" }
        }
        do {
            routines = try await RoutinesClient.shared.listRoutines().filter { $0.agentId == bot.id }
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

// MARK: - Memory

/// What the bot remembers (views/bots/BotMemoryTab.tsx): server memories
/// scoped to the bot, add and forget.
struct BotMemoryView: View {
    let bot: AgentRecord
    @State private var memories: [BotMemoryItem] = []
    @State private var isLoading = true
    @State private var error: String? = nil
    @State private var draft = ""
    @State private var search = ""

    private var filtered: [BotMemoryItem] {
        let q = search.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return memories }
        return memories.filter { $0.content.localizedCaseInsensitiveContains(q) }
    }

    var body: some View {
        List {
            Section("Teach \(bot.botDisplayName)") {
                HStack {
                    TextField("Something to remember", text: $draft, axis: .vertical)
                    Button("Add") { add() }
                        .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            Section {
                if isLoading && memories.isEmpty {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let error {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load memory",
                                      message: error, actionTitle: "Retry", action: { Task { await load() } })
                } else if filtered.isEmpty {
                    Text(memories.isEmpty ? "Nothing remembered yet." : "No matches.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(filtered) { memory in
                        VStack(alignment: .leading, spacing: 3) {
                            Text(memory.content)
                            Text([memory.type, memory.source, BotTime.relative(memory.createdAt)]
                                .compactMap { $0 }.joined(separator: " · "))
                                .font(.caption2)
                                .foregroundColor(Color("TextTertiary"))
                        }
                        .swipeActions {
                            Button("Forget", role: .destructive) { forget(memory) }
                        }
                    }
                }
            }
        }
        .searchable(text: $search)
        .navigationTitle("Memory")
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            memories = try await BotModeClient.shared.listMemories(botId: bot.id)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func add() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        Task {
            do {
                try await BotModeClient.shared.addMemory(botId: bot.id, content: text)
                await load()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
                draft = text
            }
        }
    }

    private func forget(_ memory: BotMemoryItem) {
        memories.removeAll { $0.id == memory.id }
        Task {
            do { try await BotModeClient.shared.forgetMemory(id: memory.id) } catch { await load() }
        }
    }
}

// MARK: - Channels

/// Messaging connections the bot answers on (gateway bot channels).
struct BotChannelsView: View {
    let bot: AgentRecord
    @State private var channels: [BotChannel] = []
    @State private var isLoading = true
    @State private var error: String? = nil
    @State private var connecting = false

    var body: some View {
        List {
            if isLoading && channels.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let error {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load channels",
                                  message: error, actionTitle: "Retry", action: { Task { await load() } })
            } else if channels.isEmpty {
                Text("No messaging connections yet.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(channels) { channel in
                    Toggle(isOn: Binding(
                        get: { channel.enabled },
                        set: { set(channel, enabled: $0) }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(channel.label)
                            Text(([channel.handle.map { "@\($0)" }, channel.state, channel.isDefault ? "default" : nil] as [String?])
                                .compactMap { $0 }.joined(separator: " · "))
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
            }
            if let email = bot.identityChannels?.email?.address {
                Section("Identity") {
                    Label(email, systemImage: "envelope")
                }
            }
            Section {
                Button { connecting = true } label: {
                    Label("Connect a channel", systemImage: "plus")
                }
            }
        }
        .navigationTitle("Channels")
        .refreshable { await load() }
        .task { await load() }
        .sheet(isPresented: $connecting, onDismiss: { Task { await load() } }) {
            GiveBotPhoneSheet(botId: bot.id)
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            channels = try await BotModeClient.shared.listChannels(botId: bot.id)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func set(_ channel: BotChannel, enabled: Bool) {
        Task {
            do {
                try await BotModeClient.shared.setChannel(botId: bot.id, accountId: channel.accountId, enabled: enabled)
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
            await load()
        }
    }
}

// MARK: - Time

enum BotTime {
    nonisolated(unsafe) private static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    nonisolated(unsafe) private static let plain = ISO8601DateFormatter()

    static func date(_ iso: String) -> Date? {
        if let d = fractional.date(from: iso) ?? plain.date(from: iso) { return d }
        // SQLite CURRENT_TIMESTAMP: "2026-10-27 12:00:00"
        return plain.date(from: iso.replacingOccurrences(of: " ", with: "T") + "Z")
    }

    /// The overview's short age ("now", "5m", "3h", "2d"; useModeOverview shortAgo).
    static func short(_ iso: String) -> String {
        guard let date = date(iso) else { return "" }
        let seconds = max(0, Date().timeIntervalSince(date))
        if seconds < 60 { return "now" }
        if seconds < 3600 { return "\(Int(seconds / 60))m" }
        if seconds < 86_400 { return "\(Int(seconds / 3600))h" }
        if seconds < 604_800 { return "\(Int(seconds / 86_400))d" }
        return "\(Int(seconds / 604_800))w"
    }

    static func relative(_ iso: String) -> String {
        guard let date = date(iso) else { return "" }
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        return f.localizedString(for: date, relativeTo: Date())
    }
}
