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
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    /// Filters the roster; set by the Bots header's search pill.
    var searchQuery: String = ""

    /// The roster layout, kept per scope like the web's useOverviewLayout
    /// (`allternit.session-overview.layout.bots-home-roster`).
    @AppStorage("allternit.session-overview.layout.bots-home-roster") private var layoutRaw = RosterLayout.list.rawValue
    /// The bot in To: (nil = the main bot plans it as a new project).
    @State private var selectedBotId: String?
    @State private var draft = ""
    @State private var busy = false
    @State private var hint: String?
    @State private var isNewBotPresented = false
    @State private var isPickHowPresented = false
    @State private var isTemplatesPresented = false
    @State private var templateError: String?
    // Arrivals (I13): the team flies in on entering Bots, a picked bot rises into place.
    @StateObject private var team = TeamArrivalState()
    @ObservedObject private var arrivals = ModeArrivalCenter.shared
    @Namespace private var arrivalSpace
    @State private var arrivingBotId: String?
    @State private var arrivalToken = UUID()

    private var layout: RosterLayout { RosterLayout(rawValue: layoutRaw) ?? .list }
    private var bots: [AgentRecord] { store.bots }
    private var mainBot: AgentRecord? { bots.first(where: \.isMainBot) }
    private var mainBotName: String { mainBot?.botDisplayName ?? "A://" }
    private var selectedBot: AgentRecord? { selectedBotId.flatMap { id in bots.first { $0.id == id } } }

    var body: some View {
        // BOTS block title (T11): it docks into the PhoneHeader's centre while the
        // page scrolls. The header sits just above this view, so its centre is
        // half a bar (PhoneHeader.barHeight 52 / 2) above our top edge; the dock draws from the screen's top.
        GeometryReader { geo in
            DockingTitleScrollView(
                title: "Bots",
                chrome: .overlay(barCenter: geo.frame(in: .global).minY - 26)
            ) {
                VStack(alignment: .leading, spacing: 24) {
                    if showsPickHow { pickHowCard }
                    roster
                    header
                }
                .padding(.top, 8)
                .padding(.bottom, 24)
            }
        }
        .scrollDismissesKeyboard(.interactively)
        // One bot in To: the top of the page wears its colour (BotColorWash),
        // and so do the header and the Bots rail. The dock paints its own page
        // fill, so the wash is a see-through layer on top of it.
        .overlay(alignment: .top) {
            BotColorWash(color: colors.hex(for: selectedBot))
                .frame(height: BotColorWash.height)
                .allowsHitTesting(false)
        }
        .claimsBotTint(selectedBotId)
        .overlay {
            if let id = arrivingBotId, let bot = bots.first(where: { $0.id == id }) {
                BotArrivalOverlay(bot: bot, hex: colors.hex(for: bot), namespace: arrivalSpace)
                    .id(arrivalToken)
            }
        }
        .onAppear { playTeamIfPending() }
        .onChange(of: arrivals.teamPending) { _, _ in playTeamIfPending() }
        .onChange(of: selectedBotId) { _, id in startBotArrival(id) }
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
            NewBotSheet(onBrowseTemplates: { isTemplatesPresented = true }) { outcome in
                // A new bot opens straight into its first thread (useCreateBotSubmit).
                Task {
                    await refresh()
                    if let botId = outcome.botIds.first { store.pendingOpenBotId = botId }
                }
            }
        }
        .sheet(isPresented: $isPickHowPresented) {
            // Harness & Bots 2.0: no bots yet → Create bot opens on the Runtime · Brain picker.
            NewBotSheet(onBrowseTemplates: { isTemplatesPresented = true }, opensRuntimePicker: true) { outcome in
                Task {
                    await refresh()
                    if let botId = outcome.botIds.first { store.pendingOpenBotId = botId }
                }
            }
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

    // MARK: - Empty state

    /// No bots yet (and not still loading, not searching).
    private var showsPickHow: Bool {
        bots.isEmpty && !hub.isLoading && searchQuery.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// "Pick how your bot works": the first step of Create bot, the runtime
    /// (Allternit, Claude Code, Codex, …) and the brain it thinks with.
    private var pickHowCard: some View {
        Button {
            Haptics.selection()
            isPickHowPresented = true
        } label: {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "square.stack.3d.up")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Pick how your bot works")
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                    Text("Choose its runtime (Allternit, Claude Code, Codex and more) and the brain it thinks with.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold))
                    .foregroundColor(Color("TextTertiary"))
                    .accessibilityHidden(true)
            }
            .padding(14)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityHint("Opens Create bot on the runtime choices")
        .accessibilityIdentifier("bots-pick-how")
    }

    // MARK: - Roster

    private var roster: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("YOUR TEAM")
                .font(.system(size: 12, weight: .semibold))
                .tracking(0.72)
                .foregroundColor(Color("TextTertiary"))
            Group {
                switch layout {
                case .list: rosterList
                case .grid: rosterGrid
                case .deck: rosterDeck
                }
            }
            .transition(.opacity)
            .background(alignment: .top) {
                TeamArrivalWash(hexes: bots.compactMap { colors.hex(for: $0) }, on: team.washOn)
                    .frame(height: 140)
                    .padding(.horizontal, -14)
            }
            // Account bots + the no-computer line (no error when there is no computer).
            AccountBotsStrip(hubError: hub.loadError, hasBots: !bots.isEmpty)
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
        let needle = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if !needle.isEmpty { out = out.filter { $0.name.lowercased().contains(needle) } }
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
                            .frame(width: 52, height: 52)
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
                .frame(width: 76)
            }
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 6)
        }
        .padding(.horizontal, -14)
    }

    private func rosterTile(_ entry: RosterEntry) -> some View {
        let main = entry.rank == .main
        let size: CGFloat = 52
        let selected = entry.botId != nil && entry.botId == selectedBotId
        return VStack(spacing: 2) {
            Button { open(entry) } label: {
                RosterFace(entry: entry, size: size).padding(4)
                    .matchedGeometryEffect(id: entry.botId ?? entry.id, in: arrivalSpace,
                                           isSource: true)
                    .teamFlight(index: entryIndex(entry), state: team)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open \(entry.name), \(entry.status.word)")
            HStack(spacing: 2) {
                BotNameText(entry.name, size: 12.5, ink: main ? "TextPrimary" : "TextSecondary")
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
        .frame(width: 76)
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
                        .frame(width: OverviewDeck.width, height: OverviewDeck.height - 110)
                }
            }
            .scrollTargetLayout()
            .padding(.horizontal, 14)
            .padding(.top, 4)
            .padding(.bottom, 8)
        }
        .scrollTargetBehavior(.viewAligned)
        .padding(.horizontal, -14)
        .frame(height: OverviewDeck.height - 90)
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
                        BotNameText(entry.name, size: 15)
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
                            BotNameText(entry.name, size: 14)
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
            .frame(maxWidth: .infinity, minHeight: deck ? nil : 124, maxHeight: deck ? .infinity : nil, alignment: .top)
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
                line: latest.map { $0.statusLine ?? $0.displayTitle } ?? bot.botTagline,
                when: latest.map { BotTime.short($0.lastActivityAt) } ?? "",
                task: live.map { $0.displayTitle.isEmpty ? ($0.statusLine ?? "") : $0.displayTitle }
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
            BotNameText(selectedBot.map { "\($0.botDisplayName) starts a thread for it and keeps going when you close the app." }
                 ?? "Describe it once. \(mainBotName) splits it into threads, picks the bots, and keeps going when you close the app.",
                 size: 16, ink: "TextSecondary")
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
    private func entryIndex(_ entry: RosterEntry) -> Int {
        entries.firstIndex(where: { $0.id == entry.id }) ?? 0
    }

    /// Entering Bots with no bot open: fly the team in (after any intro).
    private func playTeamIfPending() {
        guard navigator.path.isEmpty, arrivals.consumeTeam() else { return }
        team.play()
    }

    /// A bot was picked: wash in its colour, rise it out of a blur, settle it.
    private func startBotArrival(_ id: String?) {
        guard let id else { arrivingBotId = nil; return }
        arrivalToken = UUID()
        arrivingBotId = id
        let token = arrivalToken
        Task { @MainActor in
            await ModeIntroGate.shared.waitUntilClear()
            try? await Task.sleep(nanoseconds: 2_300_000_000)
            if arrivalToken == token { withAnimation(PolishTokens.arrivalBig) { arrivingBotId = nil } }
        }
    }

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
        // Bots saved to the account (no computer needed) + the no-computer state.
        await ProjectsLibraryStore.shared.load()
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

typealias RosterLayout = OverviewLayout

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
                // Every bot sits in the same round tile, whatever its art
                // (orb, sprite, portrait, Gizzi), so the row reads even. All art is
                // trimmed and fitted alike, so one size keeps every face inside the circle.
                ZStack {
                    Circle().fill(Color("BgSecondary"))
                    BotAvatarView(bot: bot, size: size * 0.84, live: false)
                }
                .frame(width: size, height: size)
                .clipShape(Circle())
                .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
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

/// The launch composer resting at the bottom of the Bots home: the composer
/// card, whose mode orb row carries "To:" (who gets it, with the caret menu).
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
    @ObservedObject private var colors = BotColorStore.shared
    @State private var isDictating = false
    @State private var dictationBase = ""

    private var placeholder: String {
        selectedBot.map { "Ask \($0.botDisplayName) to…" }
            ?? "e.g. Get the pricing page ready for Monday and check we still beat Lambda on H100s…"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
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
            // The blurred band the orb row (with the "To:" chip) rides in; content scrolls behind it.
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
            Text("To:").fixedSize()
            if let bot = selectedBot {
                Button { onPick(nil) } label: {
                    HStack(spacing: 4) {
                        BotAvatarView(bot: bot, size: 18)
                        BotNameText(bot.botDisplayName, size: 12.5, ink: "TextSecondary").foregroundColor(Color("TextSecondary")).lineLimit(1)
                        if let selectedStatus {
                            Text("· \(selectedStatus.word)")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundColor(selectedStatus.tone)
                        }
                        Text("×").foregroundColor(Color("TextSecondary")).fixedSize()
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Sending to \(bot.botDisplayName). Switch to \(mainBotName) as a new project")
            } else {
                BotNameText("\(mainBotName) · new project", size: 12.5, ink: "TextSecondary")
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
                    Button(action: onAddBot) { Label("Add a Connected bot…", systemImage: "powerplug") }
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
        }
        .font(.system(size: 12.5))
        .foregroundColor(Color("TextTertiary"))
        .padding(.leading, 6)
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
            playGamesAnytime: true,
            deck: .full,
            onSend: { _ in attachments.clear(); onSend() },
            onStop: {},
            onVoiceMode: toggleDictation,
            placeholderOverride: placeholder,
            accentBorder: colors.hex(for: selectedBot).map { Color(hex: $0).opacity(0.6) },
            orbRowTrailing: AnyView(toRow),
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
