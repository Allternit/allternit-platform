import SwiftUI

/// The Bots home on a phone (allternit-ai src/views/bots/BotLaunchpadView.tsx
/// as the phone shell lays it out), kept calm on purpose (Eoj, 2026-10-29):
/// "Your team" live roster (List · Grid · Deck), "What should the team work
/// on?", and the launch composer resting at the bottom ("To: A:// · new
/// project", or one bot picked with its @). Threads open from the header;
/// projects and group chats live in the Bots rail; team templates are in
/// the New bot sheet and the rail's ⋯ → Marketplace.
struct BotLaunchpadView: View {
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @EnvironmentObject private var navigator: BotNavigator

    /// The roster layout, kept per scope like the web's useOverviewLayout
    /// (`allternit.session-overview.layout.bots-home-roster`).
    @AppStorage("allternit.session-overview.layout.bots-home-roster") private var layoutRaw = RosterLayout.list.rawValue
    /// The bot in To: (nil = the main bot plans it as a new project).
    @State private var selectedBotId: String?
    @State private var draft = ""
    @State private var busy = false
    @State private var hint: String?
    @State private var isNewBotPresented = false
    @State private var isTemplatesPresented = false
    @State private var templateError: String?

    private var layout: RosterLayout { RosterLayout(rawValue: layoutRaw) ?? .list }
    private var bots: [AgentRecord] { store.bots }
    private var mainBot: AgentRecord? { bots.first(where: \.isMainBot) }
    private var mainBotName: String { mainBot?.botDisplayName ?? "A://" }
    private var selectedBot: AgentRecord? { selectedBotId.flatMap { id in bots.first { $0.id == id } } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                roster
                header
            }
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 24)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(Color("BgPrimary"))
        .safeAreaInset(edge: .bottom, spacing: 0) {
            BotLaunchComposer(
                draft: $draft,
                selectedBot: selectedBot,
                selectedStatus: selectedBot.map { statusOf($0) },
                mainBot: mainBot,
                mainBotName: mainBotName,
                ownBots: ownBots,
                busy: busy,
                hint: hint,
                onPick: { selectedBotId = $0; hint = nil },
                onAddBot: { isNewBotPresented = true },
                onSend: send
            )
        }
        .refreshable { await refresh() }
        .task {
            await refresh()
            // The web refreshes threads + projects every 20 s while the hub is up.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 20_000_000_000)
                await store.refreshThreads()
                await store.refreshProjects()
            }
        }
        .sheet(isPresented: $isNewBotPresented) {
            NewBotSheet(onBrowseTemplates: { isTemplatesPresented = true }) { _ in Task { await refresh() } }
        }
        .sheet(isPresented: $isTemplatesPresented) {
            AgentTemplateSheet(onConfirm: createFromTemplate)
        }
        .alert("Couldn't create the bot", isPresented: Binding(
            get: { templateError != nil }, set: { if !$0 { templateError = nil } }
        )) {
            Button("OK", role: .cancel) { templateError = nil }
        } message: { Text(templateError ?? "") }
    }

    // MARK: - Roster

    private var roster: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("YOUR TEAM")
                    .font(.system(size: 12, weight: .semibold))
                    .tracking(0.72)
                    .foregroundColor(Color("TextTertiary"))
                Spacer()
                RosterLayoutSwitch(value: Binding(get: { layout }, set: { next in
                    withAnimation(.easeOut(duration: 0.16)) { layoutRaw = next.rawValue }
                }))
            }
            Group {
                switch layout {
                case .list: rosterList
                case .grid: rosterGrid
                case .deck: rosterDeck
                }
            }
            .transition(.opacity)
            if let error = hub.loadError, bots.isEmpty {
                Text("Couldn't load your bots: \(error)")
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
    }

    private var entries: [RosterEntry] {
        // Main bot first, then pinned, then the rest (FactoryTeamStrip stripOrder).
        let ordered = bots.sorted { a, b in
            if a.isMainBot != b.isMainBot { return a.isMainBot }
            return false
        }
        var out = ordered.map { bot in
            RosterEntry.bot(bot, rank: bot.isMainBot ? .main : store.isPinned(bot.id) ? .pinned : .rest, status: statusOf(bot))
        }
        out += store.groups.map { group in
            let members = group.members.compactMap { store.bot($0.botId) }.map(statusOf)
            let status: RosterStatus = members.contains(.waiting) ? .waiting
                : members.contains(where: { $0 == .working || $0 == .talking }) ? .working : .idle
            return RosterEntry.group(group, status: status)
        }
        return out
    }

    private var rosterList: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: 6) {
                ForEach(entries) { entry in rosterTile(entry) }
                VStack(spacing: 4) {
                    Button { isNewBotPresented = true } label: {
                        Image(systemName: "plus")
                            .font(.system(size: 16))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 48, height: 48)
                            .overlay(Circle().stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                            .padding(4)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("New bot")
                    Text("New bot")
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextTertiary"))
                        .accessibilityHidden(true)
                }
                .frame(width: 72)
            }
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 6)
        }
        .padding(.horizontal, -14)
    }

    private func rosterTile(_ entry: RosterEntry) -> some View {
        let main = entry.rank == .main
        let size: CGFloat = main ? 60 : 48
        let selected = entry.botId != nil && entry.botId == selectedBotId
        return VStack(spacing: 2) {
            Button { open(entry) } label: {
                RosterFace(entry: entry, size: size).padding(4)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open \(entry.name), \(entry.status.word)")
            HStack(spacing: 2) {
                Text(entry.name)
                    .font(.system(size: 12.5, weight: main ? .semibold : .regular))
                    .foregroundColor(main ? Color("TextPrimary") : Color("TextSecondary"))
                    .underline(selected)
                    .lineLimit(1)
                if case .bot(let bot, _, _) = entry {
                    AtButton(selected: selected, name: entry.name) { toggleTo(bot) }
                }
            }
            RosterStatusWord(status: entry.status, size: 11)
        }
        .frame(width: main ? 84 : 72)
    }

    private var rosterGrid: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                ForEach(entries) { entry in rosterCard(entry, deck: false) }
                Button { isNewBotPresented = true } label: {
                    VStack(spacing: 6) {
                        Image(systemName: "plus").font(.system(size: 16))
                        Text("New bot").font(.system(size: 12.5))
                    }
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity, minHeight: 124)
                    .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                }
                .buttonStyle(.plain)
            }
            .padding(.bottom, 4)
        }
        .frame(maxHeight: 360)
        .fixedSize(horizontal: false, vertical: entries.count < 4)
    }

    private var rosterDeck: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            LazyHStack(spacing: 12) {
                ForEach(entries) { entry in
                    rosterCard(entry, deck: true)
                        .frame(width: min(UIScreen.main.bounds.width * 0.78, 300))
                }
            }
            .scrollTargetLayout()
            .padding(.horizontal, 14)
            .padding(.top, 4)
            .padding(.bottom, 8)
        }
        .scrollTargetBehavior(.viewAligned)
        .padding(.horizontal, -14)
        .frame(height: 312)
    }

    private func rosterCard(_ entry: RosterEntry, deck: Bool) -> some View {
        let facts = cardFacts(entry)
        let selected = entry.botId != nil && entry.botId == selectedBotId
        let tint: Color? = deck ? entry.bot?.botAccentColor.map { Color(hex: $0) } : nil
        return Button { open(entry) } label: {
            VStack(alignment: deck ? .center : .leading, spacing: 0) {
                if deck {
                    VStack(spacing: 8) {
                        RosterFace(entry: entry, size: entry.botId == nil ? 64 : 72)
                        Text(entry.name)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        RosterStatusWord(status: entry.status, size: 12)
                    }
                    .padding(.top, 4)
                    .frame(maxWidth: .infinity)
                    if let task = facts.task {
                        (Text("Now: ").foregroundColor(Color("TextTertiary")) + Text(task).foregroundColor(Color("TextPrimary")))
                            .font(.system(size: 12))
                            .lineLimit(2)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(RoundedRectangle(cornerRadius: 8).fill(Color("FillNeutral")))
                            .padding(.top, 12)
                    }
                    Text(facts.line)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 8)
                    Spacer(minLength: 8)
                    Text(facts.when)
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(maxWidth: .infinity, alignment: .trailing)
                } else {
                    HStack(spacing: 10) {
                        RosterFace(entry: entry, size: 40)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(entry.name)
                                .font(.system(size: 13.5, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            RosterStatusWord(status: entry.status, size: 11)
                        }
                        Spacer(minLength: 22)
                    }
                    Text(facts.line)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 8)
                    Spacer(minLength: 6)
                    Text(facts.when)
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
            }
            .padding(deck ? 16 : 12)
            .frame(maxWidth: .infinity, minHeight: deck ? 300 : 124, maxHeight: deck ? 300 : nil, alignment: .top)
            .background(
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .fill(Color("BgPanel"))
                    .overlay(
                        LinearGradient(colors: [(tint ?? .clear).opacity(0.12), .clear], startPoint: .top, endPoint: .center)
                            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                    )
            )
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(selected ? Color("AccentPrimary") : Color("BorderSubtle")))
            .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Open \(entry.name), \(entry.status.word)")
        .overlay(alignment: .topTrailing) {
            if case .bot(let bot, _, _) = entry {
                AtButton(selected: selected, name: entry.name) { toggleTo(bot) }
                    .padding(10)
            }
        }
    }

    private struct CardFacts { let line: String; let when: String; let task: String? }

    /// The card's last line and time (the rail row's line, else the tagline),
    /// and on a deck card the bot's current task (roster-status botCurrentTask).
    private func cardFacts(_ entry: RosterEntry) -> CardFacts {
        switch entry {
        case .bot(let bot, _, _):
            let threads = store.threads(for: bot.id)
            let latest = threads.max { $0.lastActivityAt < $1.lastActivityAt }
            let live = threads
                .filter { $0.group == "working" || $0.group == "waiting" }
                .max { $0.lastActivityAt < $1.lastActivityAt }
            return CardFacts(
                line: latest.map { $0.statusLine ?? $0.title } ?? bot.botTagline,
                when: latest.map { BotTime.short($0.lastActivityAt) } ?? "",
                task: live.map { $0.title.isEmpty ? ($0.statusLine ?? "") : $0.title }
            )
        case .group(let group, _):
            let last = group.log.last
            let who = last.map { $0.from == "user" ? "You" : ($0.displayName ?? "Bot") }
            return CardFacts(
                line: last.map { "\(who ?? ""): \($0.text)" } ?? "\(group.members.count) bots",
                when: BotTime.short(last?.timestamp ?? group.updatedAt),
                task: "\(group.members.count) bots · \(group.members.prefix(3).map(\.displayName).joined(separator: ", "))"
            )
        }
    }

    // MARK: - Title

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(selectedBot.map { "What should \($0.botDisplayName) work on?" } ?? "What should the team work on?")
                .font(.system(size: 34, weight: .regular, design: .serif))
                .lineSpacing(2)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Text(selectedBot.map { "\($0.botDisplayName) starts a thread for it and keeps going when you close the app." }
                 ?? "Describe it once. \(mainBotName) splits it into threads, picks the bots, and keeps going when you close the app.")
                .font(.system(size: 16))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.top, 8)
    }

    // MARK: - Actions

    private var ownBots: [AgentRecord] {
        bots.sorted { a, b in a.isMainBot && !b.isMainBot }
    }

    private func statusOf(_ bot: AgentRecord) -> RosterStatus {
        if store.waitingCount(for: bot.id) > 0 || status.needsAttention(for: bot.id) { return .waiting }
        if store.workingCount(for: bot.id) > 0 || status.isWorking(for: bot.id) { return .working }
        return .idle
    }

    /// The @: put the bot in To:, or take it out. The main bot is the
    /// new-project route, so @ on it goes back to "· new project".
    private func toggleTo(_ bot: AgentRecord) {
        hint = nil
        selectedBotId = (selectedBotId == bot.id || bot.isMainBot) ? nil : bot.id
    }

    private func open(_ entry: RosterEntry) {
        switch entry {
        case .bot(let bot, _, _): navigator.open(.chat(botId: bot.id, sessionId: nil))
        case .group(let group, _): navigator.open(.group(group.id))
        }
    }

    private func refresh() async {
        await store.refreshAll()
    }

    private func createFromTemplate(_ template: AgentTemplate) {
        Task {
            do {
                _ = try await hub.createFromTemplate(templateId: template.id)
                hub.fetchAgentsIfNeeded(force: true)
            } catch {
                templateError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    /// Send: one bot in To: starts a thread with it; otherwise the main bot
    /// takes it as a new project and plans the threads (startProjectWithAl).
    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        hint = nil
        busy = true
        let bot = selectedBot
        let name = mainBotName
        Task {
            defer { busy = false }
            if let bot {
                do {
                    let sessionId = try await store.newThreadSession(for: bot, title: Self.projectTitle(from: text))
                    try await BotModeClient.shared.sendUserMessage(sessionId: sessionId, text: text)
                    draft = ""
                    navigator.open(.chat(botId: bot.id, sessionId: sessionId))
                    await store.refreshThreads()
                } catch {
                    hint = "\(bot.botDisplayName) couldn't start that. Try again."
                }
                return
            }
            do {
                let projectId = try await BotModeClient.shared.createBotProject(title: Self.projectTitle(from: text))
                try await BotModeClient.shared.sendToCoordinator(projectId: projectId, text: text)
                draft = ""
                await store.refreshProjects()
                if let project = store.projects.first(where: { $0.id == projectId }) {
                    navigator.open(.project(project))
                }
            } catch {
                hint = "\(name) couldn't start that project. Try again."
            }
        }
    }

    /// web coordinator.ts `projectTitleFrom`: first line, ≤ 60 characters.
    static func projectTitle(from text: String) -> String {
        let line = (text.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: "\n").first.map(String.init) ?? "")
            .split(whereSeparator: \.isWhitespace).joined(separator: " ")
        if line.count > 60 { return String(line.prefix(57)).trimmingCharacters(in: .whitespaces) + "…" }
        return line.isEmpty ? "New project" : line
    }
}

// MARK: - Roster pieces

enum RosterLayout: String, CaseIterable {
    case list, grid, deck

    var icon: String {
        switch self {
        case .list: return "list.bullet"
        case .grid: return "square.grid.2x2"
        case .deck: return "rectangle.split.2x1"
        }
    }

    var label: String {
        switch self {
        case .list: return "List"
        case .grid: return "Grid"
        case .deck: return "Deck"
        }
    }
}

/// The overview standard's List · Grid · Deck switch (OverviewLayoutSwitch).
struct RosterLayoutSwitch: View {
    @Binding var value: RosterLayout

    var body: some View {
        HStack(spacing: 0) {
            ForEach(RosterLayout.allCases, id: \.self) { layout in
                let on = value == layout
                Button { value = layout } label: {
                    Image(systemName: layout.icon)
                        .font(.system(size: 14, weight: on ? .bold : .regular))
                        .foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                        .frame(width: 36, height: 32)
                        .background(
                            RoundedRectangle(cornerRadius: 6, style: .continuous)
                                .fill(on ? Color("BgPanel") : .clear)
                                .shadow(color: .black.opacity(on ? 0.08 : 0), radius: 1, y: 1)
                        )
                }
                .buttonStyle(.plain)
                .accessibilityLabel(layout.label)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BorderStrong")))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Layout")
    }
}

/// Working · Needs you · Talking · Idle (roster-status.ts).
enum RosterStatus: Equatable {
    case waiting, talking, working, idle

    var word: String {
        switch self {
        case .waiting: return "Needs you"
        case .talking: return "Talking"
        case .working: return "Working"
        case .idle: return "Idle"
        }
    }

    /// The Bots home's neutral ink: Working wears the text colour, not an accent.
    var tone: Color {
        switch self {
        case .waiting: return Theme.statusWarning
        case .talking: return Theme.statusSuccess
        case .working: return Color("TextPrimary")
        case .idle: return Color("TextTertiary")
        }
    }
}

enum RosterRank { case main, pinned, rest, group }

enum RosterEntry: Identifiable {
    case bot(AgentRecord, rank: RosterRank, status: RosterStatus)
    case group(BotGroupChat, status: RosterStatus)

    var id: String {
        switch self {
        case .bot(let bot, _, _): return "bot:\(bot.id)"
        case .group(let group, _): return "group:\(group.id)"
        }
    }

    var name: String {
        switch self {
        case .bot(let bot, _, _): return bot.botDisplayName
        case .group(let group, _): return group.name
        }
    }

    var status: RosterStatus {
        switch self {
        case .bot(_, _, let status), .group(_, let status): return status
        }
    }

    var rank: RosterRank {
        switch self {
        case .bot(_, let rank, _): return rank
        case .group: return .group
        }
    }

    var bot: AgentRecord? {
        if case .bot(let bot, _, _) = self { return bot }
        return nil
    }

    var botId: String? { bot?.id }
}

struct RosterStatusWord: View {
    let status: RosterStatus
    var size: CGFloat = 11

    var body: some View {
        Text(status.word)
            .font(.system(size: size, weight: status == .idle ? .regular : .medium))
            .foregroundColor(status.tone)
            .lineLimit(1)
    }
}

/// A face in the roster: the bot's avatar (or a group's stacked members)
/// with Working's quiet ring, Needs you's amber dot and the pinned star.
struct RosterFace: View {
    let entry: RosterEntry
    let size: CGFloat

    var body: some View {
        ZStack {
            switch entry {
            case .bot(let bot, _, _):
                BotAvatarView(bot: bot, size: size, live: false)
            case .group(let group, _):
                GroupAvatarStack(members: group.members, size: size * (group.members.count == 1 ? 0.86 : 0.68))
                    .frame(height: size)
            }
        }
        .overlay {
            if entry.status == .working {
                Group {
                    if entry.bot != nil {
                        Circle().stroke(Color("TextPrimary").opacity(0.75), lineWidth: 2)
                    } else {
                        RoundedRectangle(cornerRadius: 12).stroke(Color("TextPrimary").opacity(0.75), lineWidth: 2)
                    }
                }
                .padding(-3)
            }
        }
        .overlay(alignment: .topTrailing) {
            if entry.status == .waiting {
                Circle().fill(Theme.statusWarning)
                    .frame(width: 12, height: 12)
                    .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                    .offset(x: 2, y: -2)
            }
        }
        .overlay(alignment: .topLeading) {
            if entry.rank == .pinned {
                Image(systemName: "star.fill")
                    .font(.system(size: 8))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 16, height: 16)
                    .background(Circle().fill(Color("BgPanel")))
                    .overlay(Circle().stroke(Color("BorderSubtle")))
                    .offset(x: -2, y: -2)
            }
        }
    }
}

/// The small @ that puts a bot in To:.
struct AtButton: View {
    let selected: Bool
    let name: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "at")
                .font(.system(size: 11, weight: .bold))
                .foregroundColor(selected ? Color("BgPrimary") : Color("TextTertiary"))
                .frame(width: 20, height: 20)
                .background(Circle().fill(selected ? Color("TextPrimary") : .clear))
                .frame(width: 32, height: 32)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(width: 20, height: 20)
        .accessibilityLabel(selected ? "Stop sending to \(name)" : "Send to \(name)")
    }
}

// MARK: - Launch composer

/// The launch composer resting at the bottom of the Bots home: the "To:" row
/// (who gets it, with the caret menu), then the composer card.
struct BotLaunchComposer: View {
    @Binding var draft: String
    let selectedBot: AgentRecord?
    let selectedStatus: RosterStatus?
    let mainBot: AgentRecord?
    let mainBotName: String
    let ownBots: [AgentRecord]
    let busy: Bool
    let hint: String?
    let onPick: (String?) -> Void
    let onAddBot: () -> Void
    let onSend: () -> Void

    @StateObject private var dictation = DictationController()
    @StateObject private var attachments = AttachmentStore()
    @State private var isDictating = false
    @State private var dictationBase = ""

    private var placeholder: String {
        selectedBot.map { "Ask \($0.botDisplayName) to…" }
            ?? "e.g. Get the pricing page ready for Monday and check we still beat Lambda on H100s…"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            toRow
            card
            if let hint {
                Text(hint)
                    .font(.system(size: 12.5))
                    .foregroundColor(Theme.statusWarning)
                    .accessibilityAddTraits(.updatesFrequently)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 6)
        .padding(.bottom, 8)
        .background(
            // The blurred band the "To:" chip rides in; content scrolls behind it.
            // web: inset -34px above the To: row, 96% page colour, 14px blur,
            // fading in over the first 22px.
            Rectangle().fill(.ultraThinMaterial)
                .overlay(Color("BgPrimary").opacity(0.96))
                .mask(VStack(spacing: 0) {
                    LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(height: 22)
                    Rectangle()
                })
                .padding(.top, -34)
                .ignoresSafeArea(edges: .bottom)
        )
        .onChange(of: dictation.transcript) { _, transcript in
            guard isDictating else { return }
            draft = dictationBase + transcript
        }
        .onChange(of: dictation.isRecording) { _, recording in
            guard !recording, isDictating else { return }
            draft = dictationBase + dictation.transcript
            isDictating = false
        }
    }

    private var toRow: some View {
        HStack(spacing: 8) {
            Text("To:")
            if let bot = selectedBot {
                Button { onPick(nil) } label: {
                    HStack(spacing: 4) {
                        BotAvatarView(bot: bot, size: 18)
                        Text(bot.botDisplayName).foregroundColor(Color("TextSecondary")).lineLimit(1)
                        if let selectedStatus {
                            Text("· \(selectedStatus.word)")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundColor(selectedStatus.tone)
                        }
                        Text("×").foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Sending to \(bot.botDisplayName). Switch to \(mainBotName) as a new project")
            } else {
                Text("\(mainBotName) · new project")
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
            }
            Menu {
                Button { onPick(nil) } label: {
                    checkLabel("\(mainBotName) · new project", on: selectedBot == nil)
                }
                if !ownBots.isEmpty {
                    Section("Send it to one bot") {
                        ForEach(ownBots) { bot in
                            Button { onPick(bot.isMainBot ? nil : bot.id) } label: {
                                checkLabel(bot.botDisplayName, on: selectedBot?.id == bot.id)
                            }
                        }
                    }
                }
                Section {
                    Button(action: onAddBot) { Label("Add a vendor bot…", systemImage: "powerplug") }
                }
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 4)
                    .padding(.vertical, 3)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderStrong")))
            }
            .accessibilityLabel("Choose who to send to")
            Spacer(minLength: 0)
        }
        .font(.system(size: 12.5))
        .foregroundColor(Color("TextTertiary"))
    }

    @ViewBuilder
    private func checkLabel(_ title: String, on: Bool) -> some View {
        if on { Label(title, systemImage: "checkmark") } else { Text(title) }
    }

    /// The same composer as Home (web BotLaunchpadView mounts ChatComposer):
    /// + · model · voice/send, with the Agent mode / Plugins / Artifacts deck.
    private var card: some View {
        ComposerView(
            inputText: $draft,
            attachmentStore: attachments,
            isStreaming: false,
            isSendDisabled: busy,
            hasActiveSession: false,
            deck: .full,
            onSend: { _ in attachments.clear(); onSend() },
            onStop: {},
            onVoiceMode: toggleDictation,
            placeholderOverride: placeholder,
            accentBorder: selectedBot?.botAccentColor.map { Color(hex: $0).opacity(0.6) },
            showsPermission: false
        )
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
}

extension AgentRecord {
    /// web `botProfile.chiefOfStaff === true` (the main bot). The typed
    /// profile drops unknown keys, so read the raw bot profile in config.
    var isMainBot: Bool {
        if case .object(let profile)? = config?["botProfile"], case .bool(true)? = profile["chiefOfStaff"] { return true }
        if case .object(let profile)? = config?["bot_profile"], case .bool(true)? = profile["chiefOfStaff"] { return true }
        return false
    }
}
