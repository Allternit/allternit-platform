import SwiftUI

/// Whether the person has seen "Meet your bots". One flag across every
/// surface, like the web (allternit-ai `src/lib/bots/bots-intro.ts`): the
/// account flag `botsIntroSeenAt` (cloud-api `GET/PUT /api/v1/me/flags`),
/// with a local copy so the card never flashes back while the account loads
/// or when offline.
@MainActor
final class BotsIntroStore: ObservableObject {
    static let shared = BotsIntroStore()

    nonisolated static let flagKey = "botsIntroSeenAt"
    private static let localKey = "allternit-bots-intro-seen"

    /// True while the intro should be on screen.
    @Published var isPresented = false

    private var checkedThisLaunch = false

    private init() {}

    var seenLocally: Bool { UserDefaults.standard.string(forKey: Self.localKey) != nil }

    /// First workspace appearance: show the intro once per account.
    func checkOnLaunch() {
        guard !checkedThisLaunch else { return }
        checkedThisLaunch = true
        guard AuthManager.shared.isSignedIn else { return }
        if seenLocally {
            // Seen here before the account knew: tell the account.
            Task { await pushLocalToAccountIfNeeded() }
            return
        }
        let key = Self.flagKey
        Task {
            // A slow read must not hold the decision: 3 s, like the web.
            let seen = await withTaskGroup(of: Bool?.self) { group -> Bool? in
                group.addTask {
                    do { return try await AccountFlagsClient.shared.string(key) != nil } catch { return nil }
                }
                group.addTask {
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                    return nil
                }
                let first = await group.next() ?? nil
                group.cancelAll()
                return first
            }
            if seen == true {
                UserDefaults.standard.set(ISO8601DateFormatter().string(from: Date()), forKey: Self.localKey)
            } else {
                isPresented = true
            }
        }
    }

    /// Settings → "Meet your bots": show it again.
    func replay() {
        isPresented = true
    }

    /// Done or Skip: seen here at once, on the account when reachable.
    func markSeen() {
        recordSeen()
        isPresented = false
    }

    /// "Start the job": the intro counts as seen from here (web
    /// `intro.markSeen()` in startJob), while the done step stays open.
    func recordSeen() {
        let at = ISO8601DateFormatter().string(from: Date())
        UserDefaults.standard.set(at, forKey: Self.localKey)
        Task { try? await AccountFlagsClient.shared.set(Self.flagKey, to: at) }
    }

    private func pushLocalToAccountIfNeeded() async {
        guard let at = UserDefaults.standard.string(forKey: Self.localKey) else { return }
        do {
            if try await AccountFlagsClient.shared.string(Self.flagKey) == nil {
                try await AccountFlagsClient.shared.set(Self.flagKey, to: at)
            }
        } catch {
            // Offline: the account catches up on a later launch.
        }
    }
}

/// Presents "Meet your bots" over the workspace (attached to MainWorkspaceView).
struct MeetYourBotsHost: ViewModifier {
    @ObservedObject private var intro = BotsIntroStore.shared
    @EnvironmentObject private var modeStore: AppModeStore

    func body(content: Content) -> some View {
        content
            .onAppear { intro.checkOnLaunch() }
            .fullScreenCover(isPresented: $intro.isPresented) {
                MeetYourBotsView(onFinish: { openBotId in
                    intro.markSeen()
                    // Like the web's onDone: open the chat thread #1 started
                    // in (or the main bot) in Bots mode.
                    if let openBotId {
                        BotModeStore.shared.pendingOpenBotId = openBotId
                        modeStore.selectBarItem(.agents)
                    }
                })
            }
    }
}

// MARK: - Model

/// The flow's drafts: nothing is created until "Start the job" (web
/// MeetYourBotsCard). Team, main bot, routines and first job are picks;
/// then `MeetFinish.run` makes the real calls and the done step reports
/// each result.
@MainActor
final class MeetYourBotsModel: ObservableObject {
    enum Step: Equatable { case start, team, main, routines, notifications, job, done }
    enum RosterState: Equatable { case loading, loaded, failed(String) }

    struct JobOption: Identifiable {
        /// `meetMainKey` or a template id.
        let key: String
        let name: String
        let prompt: String
        let expect: String
        let starters: [String]
        var id: String { key }
    }

    let catalog = MeetBotCatalog.shared

    @Published private(set) var roster: [MeetRosterBot] = []
    @Published private(set) var rosterState: RosterState = .loading
    @Published private(set) var entry: MeetEntry = .setup
    @Published private(set) var stepIndex = 0
    @Published private(set) var members: [String] = []
    @Published var category = "all"
    @Published var mainChoice: MeetMainChoice = .primary
    @Published var lookName = ""
    @Published private(set) var routines: [MeetRoutineDraft] = []
    @Published private(set) var jobKey: String?
    @Published var jobText = ""
    @Published private(set) var items: [MeetFinishItem] = []
    @Published private(set) var running = false
    @Published private(set) var started = false
    @Published private(set) var adding: Set<String> = []

    private var removedDrafts: Set<String> = []
    private var seeded = false
    private let state = MeetFinishState()
    private var teamAtStart: [String] = []

    var steps: [Step] {
        entry == .setup
            ? [.start, .team, .main, .routines, .notifications, .job, .done]
            : [.start, .team, .routines, .notifications, .job, .done]
    }
    var step: Step { steps[min(stepIndex, steps.count - 1)] }

    // MARK: Roster

    func loadRoster() async {
        rosterState = .loading
        do {
            let bots = try await MeetAgentsAPI.roster()
            roster = bots
            rosterState = .loaded
            // Bot Mode's first visit skips "main" (a main bot already exists).
            // Decided before the person leaves the start step only.
            if stepIndex == 0 { entry = MeetMainBot.find(bots) == nil ? .setup : .botmode }
            if !seeded {
                seeded = true
                // Template bots the person already has start on the team (never made twice).
                let have = existingTemplateIds
                let ids = catalog.templates.map(\.id).filter { have.contains($0) }
                if members.isEmpty && !ids.isEmpty { members = Array(ids.prefix(12)) }
                syncDrafts()
            }
        } catch {
            rosterState = .failed(FriendlyErrorMessage.from(error.localizedDescription))
        }
    }

    var existingTemplateIds: Set<String> { Set(roster.compactMap(\.templateId)) }
    var mainBot: MeetRosterBot? { MeetMainBot.find(roster) }

    /// The name the flow calls the main bot.
    var leader: String {
        if entry == .botmode { return mainBot?.displayName ?? "your main bot" }
        switch mainChoice {
        case .primary: return catalog.primary.name
        case .look:
            let name = lookName.trimmingCharacters(in: .whitespacesAndNewlines)
            return name.isEmpty ? catalog.primary.name : name
        case .bot(let id): return roster.first(where: { $0.id == id })?.displayName ?? "your main bot"
        }
    }

    // MARK: Navigation

    func next() {
        guard stepIndex < steps.count - 1 else { return }
        if step == .main, case .look(let id, _) = mainChoice {
            mainChoice = .look(id: id, name: lookName.trimmingCharacters(in: .whitespacesAndNewlines))
        }
        stepIndex += 1
    }

    func back() { if stepIndex > 0 && step != .done { stepIndex -= 1 } }

    // MARK: Team

    var shownTemplates: [MeetBotTemplate] {
        catalog.templates.filter { category == "all" || $0.category == category }
    }

    func isOnTeam(_ id: String) -> Bool { members.contains(id) }

    func toggle(_ id: String) {
        if let i = members.firstIndex(of: id) { members.remove(at: i) } else { members.append(id) }
        syncDrafts()
        if let jobKey, jobKey != meetMainKey, !members.contains(jobKey) {
            self.jobKey = nil
            jobText = ""
        }
    }

    /// team-slots syncDrafts: drop a removed bot's routines, add a new bot's
    /// template routines (unless deleted, or the bot is already on the account).
    private func syncDrafts() {
        let keep = routines.filter { $0.botKey == meetMainKey || members.contains($0.botKey) }
        let have = Set(keep.map(\.id))
        let skip = existingTemplateIds
        var add: [MeetRoutineDraft] = []
        for id in members where !skip.contains(id) {
            guard let t = catalog.template(id) else { continue }
            add += MeetRoutineDraft.drafts(for: t).filter { !have.contains($0.id) && !removedDrafts.contains($0.id) }
        }
        routines = keep + add
    }

    func toggleRoutine(_ id: String) {
        guard let i = routines.firstIndex(where: { $0.id == id }) else { return }
        routines[i].enabled.toggle()
    }

    func removeRoutine(_ id: String) {
        routines.removeAll { $0.id == id }
        removedDrafts.insert(id)
    }

    /// The routine editor's save (RoutinesStep fromEditorDraft): a template
    /// routine keeps its prompt, a custom one's title is its instruction.
    func saveRoutine(id: String?, edit: RoutineEdit) {
        let title = edit.title.trimmingCharacters(in: .whitespacesAndNewlines)
        if let id, let i = routines.firstIndex(where: { $0.id == id }) {
            routines[i].cron = edit.rule.cron
            routines[i].scheduleLabel = edit.rule.label
            if !title.isEmpty { routines[i].title = title }
            if routines[i].custom { routines[i].prompt = routines[i].title }
            if let key = edit.botKey { routines[i].botKey = key }
        } else if !title.isEmpty {
            routines.append(MeetRoutineDraft(id: "custom:\(UUID().uuidString)", botKey: edit.botKey ?? meetMainKey, title: title,
                                             prompt: title, cron: edit.rule.cron, scheduleLabel: edit.rule.label,
                                             enabled: true, custom: true))
        }
    }

    /// Who can run a routine: the main bot and the team.
    var routineBots: [WizardOption] {
        [WizardOption(value: meetMainKey, label: leader)] + members.compactMap { id in
            catalog.template(id).map { WizardOption(value: id, label: $0.name) }
        }
    }

    // MARK: Who leads — vendor / new / import

    /// A plain-words line about what a setup sheet just did.
    @Published var setupNote: String?

    /// A bot made or connected from the main step: reload the roster and pick it to lead.
    func adoptLeader(_ outcome: BotSetupOutcome) async {
        setupNote = outcome.summary
        await loadRoster()
        let id = outcome.mainBotId ?? outcome.botIds.first
        if let id, roster.contains(where: { $0.id == id }) {
            mainChoice = .bot(id: id)
        } else if id != nil {
            setupNote = outcome.summary + " It’s on your account; pick it below once your bots list catches up."
        }
    }

    func routineBotName(_ key: String) -> String {
        key == meetMainKey ? leader : catalog.template(key)?.name ?? "Bot"
    }

    var routinesOn: Int { routines.filter(\.enabled).count }

    // MARK: First job

    var jobs: [JobOption] {
        let p = catalog.primary
        var out = [JobOption(key: meetMainKey, name: leader, prompt: p.firstTask.prompt, expect: p.firstTask.expect,
                             starters: Array(p.starterPrompts.prefix(3)))]
        for id in members {
            guard let t = catalog.template(id) else { continue }
            out.append(JobOption(key: id, name: t.name, prompt: t.firstTask.prompt, expect: t.firstTask.expect,
                                 starters: Array(t.starterPrompts.prefix(3))))
        }
        return out
    }

    func selectJob(_ job: JobOption) {
        guard jobKey != job.key else { return }
        jobKey = job.key
        jobText = job.prompt
    }

    var canStart: Bool {
        jobKey != nil && !jobText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !started
    }

    // MARK: Finishing

    private func input() -> MeetFinishInput {
        let text = jobText.trimmingCharacters(in: .whitespacesAndNewlines)
        return MeetFinishInput(
            entry: entry, members: members, main: mainChoice, routines: routines,
            job: jobKey.flatMap { key -> (botKey: String, text: String)? in text.isEmpty ? nil : (botKey: key, text: text) }
        )
    }

    func startJob() {
        guard canStart else { return }
        let input = input()
        started = true
        teamAtStart = members
        items = MeetFinish.plannedItems(input)
        stepIndex = steps.firstIndex(of: .done) ?? steps.count - 1
        BotsIntroStore.shared.recordSeen()
        Task { await run(input, prior: nil) }
    }

    func retry() {
        Task { await run(input(), prior: items) }
    }

    var failed: Bool { !running && items.contains { $0.status == .failed } }

    private func run(_ input: MeetFinishInput, prior: [MeetFinishItem]?) async {
        guard !running else { return }
        running = true
        let out = await MeetFinish.run(input, state: state, prior: prior) { [weak self] its in
            self?.merge(its)
        }
        merge(out)
        running = false
    }

    /// A later run's items replace the same keys and add the new ones.
    private func merge(_ out: [MeetFinishItem]) {
        let keys = Set(out.map(\.key))
        func order(_ k: String) -> Int { k == "main" ? 0 : k.hasPrefix("bot:") ? 1 : k == "routines" ? 2 : 3 }
        items = (items.filter { !keys.contains($0.key) } + out).sorted { order($0.key) < order($1.key) }
    }

    /// "Works well with your team": the bots the team hands work to.
    var suggestions: [MeetBotTemplate] {
        catalog.handoffSuggestions(for: teamAtStart.isEmpty ? members : teamAtStart)
    }

    func isMade(_ id: String) -> Bool { state.botIds[id] != nil || existingTemplateIds.contains(id) }

    /// Adds a suggested bot after the team was made; its routines come in
    /// paused (nobody reviewed them), like the web's addPending.
    func addLater(_ id: String) {
        guard !running, !adding.contains(id), let t = catalog.template(id) else { return }
        adding.insert(id)
        let drafts = MeetRoutineDraft.drafts(for: t).map { d -> MeetRoutineDraft in var d = d; d.enabled = false; return d }
        var prior = [MeetFinishItem(key: "main", label: items.first(where: { $0.key == "main" })?.label ?? "Main bot", status: .ok),
                     MeetFinishItem(key: "bot:\(id)", label: t.name, status: .pending)]
        if !drafts.isEmpty { prior.append(MeetFinishItem(key: "routines", label: "Routines", status: .pending)) }
        let input = MeetFinishInput(entry: entry, members: [id], main: mainChoice, routines: drafts, job: nil)
        Task {
            await run(input, prior: prior)
            if state.botIds[id] != nil {
                teamAtStart.append(id)
                if !members.contains(id) { members.append(id) }
            }
            adding.remove(id)
        }
    }

    /// The bot whose chat opens when the person finishes.
    var openBotId: String? {
        state.startedThreadBotId(for: input()) ?? state.mainBotId
    }

    var mainFailed: Bool { items.first(where: { $0.key == "main" })?.status == .failed }
}

// MARK: - View

/// "Meet your bots" on the phone, laid out like the web card stacked
/// full-bleed (MeetYourBotsCard `is-phone`): the step's visual on top, step
/// dots with Back / Skip, the step's panel, and a pinned footer.
struct MeetYourBotsView: View {
    /// The bot whose chat should open (thread #1's bot, or the main bot), or nil.
    let onFinish: (_ openBotId: String?) -> Void
    /// The done step's button ("Open Bot Mode"; first-run setup says what comes next).
    var doneLabel: String? = nil

    @StateObject private var model = MeetYourBotsModel()
    @ObservedObject private var hub = AgentHubStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var notificationsOn = false
    @State private var askingNotifications = false
    @State private var leadSheet: LeadSheet?
    @State private var editing: RoutineEditTarget?
    @State private var discOpen = false
    @State private var connectionsOpen = false

    /// The "who leads" setup paths beyond the looks (useBotSetup modes vendor / new / import).
    private enum LeadSheet: String, Identifiable {
        case vendor, new, importBot
        var id: String { rawValue }
    }

    /// The routine the editor is open for; nil id = a new routine.
    private struct RoutineEditTarget: Identifiable {
        let id: String
        let draftId: String?
        let title: String
        let rule: RoutineRule
        let botKey: String
    }

    var body: some View {
        GeometryReader { geo in
            VStack(spacing: 0) {
                visual
                    .frame(height: visualHeight(geo.size.height + geo.safeAreaInsets.top + geo.safeAreaInsets.bottom))
                    .frame(maxWidth: .infinity)
                    .clipped()
                    .animation(reduceMotion ? nil : .timingCurve(0.16, 1, 0.3, 1, duration: 0.4), value: model.stepIndex)
                header
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            Color.clear.frame(height: 0).id("top")
                            stepBody
                                .id(model.stepIndex)
                                .transition(reduceMotion ? .opacity : .asymmetric(
                                    insertion: .opacity.combined(with: .offset(y: 10)),
                                    removal: .opacity))
                        }
                        .padding(.horizontal, 20)
                        .padding(.vertical, 12)
                    }
                    .scrollDismissesKeyboard(.interactively)
                    .onChange(of: model.stepIndex) { _, _ in proxy.scrollTo("top", anchor: .top) }
                }
                footer
            }
        }
        .background(MeetPalette.bg.ignoresSafeArea())
        .animation(reduceMotion ? .easeOut(duration: 0.15) : .timingCurve(0.16, 1, 0.3, 1, duration: 0.3), value: model.stepIndex)
        .sheet(item: $leadSheet) { which in
            switch which {
            case .vendor:
                NavigationStack {
                    VendorConnectView(offerMain: true) { first, main in
                        let ids = [main ?? first].compactMap { $0 }
                        Task { await model.adoptLeader(BotSetupOutcome(botIds: ids, mainBotId: main, summary: "Vendor bot connected.")) }
                    }
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { leadSheet = nil } } }
                }
            case .new:
                NewBotSheet { outcome in Task { await model.adoptLeader(outcome) } }
            case .importBot:
                ImportBotSheet { outcome in Task { await model.adoptLeader(outcome) } }
            }
        }
        .sheet(item: $editing) { target in
            RoutineScheduleEditor(
                title: target.title, initial: target.rule, titleEditable: target.draftId == nil || isCustom(target.draftId),
                botChoices: model.routineBots, initialBot: target.botKey,
                onSave: { edit in
                    model.saveRoutine(id: target.draftId, edit: edit)
                    editing = nil
                },
                onDelete: target.draftId.map { id in { model.removeRoutine(id); editing = nil } },
                onCancel: { editing = nil })
        }
        .sheet(isPresented: $connectionsOpen) {
            NavigationStack {
                SettingsWebPage(page: .subscriptions, intro: "How your bots reach the apps, bots and coding tools you already use. Your Allternit plan is under Billing.")
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { connectionsOpen = false } } }
            }
        }
        .task {
            hub.fetchAgentsIfNeeded()
            await model.loadRoster()
            let status = await PushNotificationManager.shared.authorizationStatus()
            notificationsOn = status == .authorized || status == .provisional
        }
    }

    // MARK: Visual

    /// Web `visualH` for a stacked card: 45% of the card, capped per step.
    private func visualHeight(_ h: CGFloat) -> CGFloat {
        let cap: CGFloat
        switch model.step {
        case .start: cap = 340
        case .routines: cap = 380
        case .main: cap = 260
        default: cap = 250
        }
        return min(h * 0.45, cap)
    }

    @ViewBuilder
    private var visual: some View {
        ZStack {
            switch model.step {
            case .start:
                MeetNetworkView()
            case .team:
                let n = model.members.count
                grid(label: n > 0 ? "\(n) bot\(n > 1 ? "s" : "") · drag to arrange" : "Drag bots here, or tap a card",
                     locked: model.existingTemplateIds)
            case .main:
                MeetMainStage(field: leaderField, tile: AnyView(leaderTile(size: 150, live: true)), label: "\(model.leader) idle")
            case .routines:
                MeetDayCalendar(
                    items: model.routines.map { r in
                        MeetDayCalendar.Item(id: r.id, title: r.title, rule: r.rule, enabled: r.enabled,
                                             color: botColor(r.botKey), avatar: AnyView(tile(forKey: r.botKey, size: 16)))
                    },
                    onNew: { day, minute in addRoutine(on: day, at: minute) },
                    onEdit: { id in if let r = model.routines.first(where: { $0.id == id }) { edit(r) } })
            case .notifications:
                ZStack {
                    MeetPalette.soft
                    MeetBlockPattern()
                    Image(systemName: notificationsOn ? "bell.badge.fill" : "bell.badge")
                        .font(.system(size: 44, weight: .medium))
                        .foregroundStyle(MeetPalette.ink)
                        .frame(width: 96, height: 96)
                        .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 24))
                        .overlay(RoundedRectangle(cornerRadius: 24).stroke(MeetPalette.line))
                        .accessibilityHidden(true)
                }
            case .job:
                let job = model.jobs.first { $0.key == model.jobKey }
                let handoff = job.map { $0.key != meetMainKey } ?? false
                MeetFirstJobVisual(
                    main: AnyView(leaderTile(size: 56, live: true)),
                    other: handoff ? AnyView(tile(forKey: job!.key, size: 56)) : nil,
                    caption: job == nil ? "Pick a first job" : handoff ? "\(model.leader.capitalizedFirst) hands it to \(job!.name)" : "\(model.leader.capitalizedFirst) does this one",
                    expect: job?.expect)
            case .done:
                let n = model.members.count
                grid(label: "\(model.leader.capitalizedFirst) leads \(n) bot\(n == 1 ? "" : "s")",
                     locked: model.existingTemplateIds.union(model.members.filter { model.isMade($0) }))
            }
        }
        .id(model.step)
        .transition(reduceMotion ? .opacity : .opacity.combined(with: .scale(scale: 1.02)))
    }

    private func grid(label: String, locked: Set<String>) -> some View {
        MeetTeamGrid(
            members: model.members, catalog: model.catalog,
            center: hasLeader ? AnyView(leaderTile(size: 68, live: true)) : nil,
            centerName: model.leader, size: 52, label: label, lockedIds: locked,
            onRemove: { model.toggle($0) })
    }

    /// The main bot has been chosen (the grid shows it instead of the dashed slot).
    private var hasLeader: Bool {
        model.entry == .botmode || model.steps.firstIndex(of: .main).map { model.stepIndex > $0 } ?? true
    }

    // MARK: Chrome

    private var header: some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                ForEach(Array(model.steps.enumerated()), id: \.offset) { n, _ in
                    Capsule()
                        .fill(n <= model.stepIndex ? MeetPalette.coral : MeetPalette.line)
                        .frame(width: n == model.stepIndex ? 18 : 6, height: 6)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Step \(model.stepIndex + 1) of \(model.steps.count)")
            Spacer()
            HStack(spacing: 12) {
                if model.stepIndex > 0 && model.step != .done {
                    linkButton("Back") { model.back() }
                }
                if model.step != .start && model.step != .done {
                    linkButton("Skip") { onFinish(nil) }
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 16)
        .frame(minHeight: 40)
    }

    private func linkButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(title, action: action)
            .font(.system(size: 13))
            .foregroundStyle(MeetPalette.sub)
            .padding(.horizontal, 4)
            .frame(minHeight: 28)
            .contentShape(Rectangle().inset(by: -8))
    }

    @ViewBuilder
    private var footer: some View {
        VStack(spacing: 4) {
            switch model.step {
            case .start:
                primaryButton(model.entry == .setup ? "Build my team" : "Let’s go") { model.next() }
                ghostButton(model.entry == .setup ? "Skip for now" : "I’ll look around") { onFinish(nil) }
            case .team:
                let n = model.members.count
                primaryButton(n == 0 ? "Add at least one bot" : "Continue with \(n) bot\(n > 1 ? "s" : "")", disabled: n == 0) { model.next() }
            case .main:
                primaryButton("Make \(model.leader) my main bot") { model.next() }
            case .routines:
                primaryButton("Looks good") { model.next() }
            case .notifications:
                primaryButton(notificationsOn ? "Continue" : "Turn on notifications", disabled: askingNotifications) {
                    if notificationsOn { model.next(); return }
                    askingNotifications = true
                    Task {
                        notificationsOn = await PushNotificationManager.shared.requestPermissionAndRegister()
                        askingNotifications = false
                        model.next()
                    }
                }
                if !notificationsOn { ghostButton("Not now") { model.next() } }
            case .job:
                primaryButton("Start the job", disabled: !model.canStart) { model.startJob() }
            case .done:
                primaryButton(model.running ? "Setting up…" : doneLabel ?? "Open Bot Mode", disabled: model.running) {
                    onFinish(model.openBotId)
                }
                if model.failed { ghostButton("Retry what didn’t finish") { model.retry() } }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 8)
        .padding(.bottom, 16)
        .overlay(alignment: .top) { MeetPalette.fill.frame(height: 1) }
    }

    private func primaryButton(_ title: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(MeetPalette.bg)
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(MeetPalette.ink, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .opacity(disabled ? 0.35 : 1)
        }
        .buttonStyle(MeetPressStyle())
        .disabled(disabled)
    }

    private func ghostButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(MeetPalette.sub)
                .frame(maxWidth: .infinity, minHeight: 48)
        }
        .buttonStyle(MeetPressStyle())
    }

    // MARK: Pieces

    private func h2(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 24, weight: .semibold))
            .tracking(-0.36)
            .foregroundStyle(MeetPalette.ink)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityAddTraits(.isHeader)
    }

    private func sub(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 14))
            .lineSpacing(2.5)
            .foregroundStyle(MeetPalette.sub)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 4)
            .padding(.bottom, 12)
    }

    private func monoLabel(_ text: String) -> some View {
        Text(text.uppercased())
            .font(MeetPalette.mono(11.5, .semibold))
            .tracking(0.7)
            .foregroundStyle(MeetPalette.mut)
            .padding(.bottom, 8)
    }

    private func templateTile(_ t: MeetBotTemplate, size: CGFloat = 44) -> some View {
        MeetTemplateTile(color: t.color, spriteURL: t.spriteURL, size: size, label: t.id)
    }

    @ViewBuilder
    private func botTile(_ bot: MeetRosterBot, size: CGFloat = 40) -> some View {
        if let record = hub.agent(withId: bot.id) {
            BotAvatarView(bot: record, size: size)
        } else {
            AgentIdenticonView(seed: bot.id, size: size)
        }
    }

    @ViewBuilder
    private func leaderTile(size: CGFloat = 40, live: Bool = false) -> some View {
        if model.entry == .botmode, let main = model.mainBot {
            botTile(main, size: size)
        } else {
            switch model.mainChoice {
            case .primary:
                MeetLookTile(id: "a", size: size, live: live)
            case .look(let id, _):
                MeetLookTile(id: id, size: size, live: live)
            case .bot(let id):
                if let bot = model.roster.first(where: { $0.id == id }) { botTile(bot, size: size) } else { MeetLookTile(id: "a", size: size) }
            }
        }
    }

    /// The main bot's field colour on the stage.
    private var leaderField: Color {
        switch model.mainChoice {
        case .primary: return Color(hex: "#141413")
        case .look(let id, _): return Color(hex: model.catalog.looks.first { $0.id == id }?.field ?? "#141413")
        case .bot: return MeetPalette.soft
        }
    }

    private func botColor(_ key: String) -> Color {
        if key == meetMainKey { return model.mainChoice == .primary ? MeetPalette.ink : leaderField }
        return model.catalog.template(key).map { Color(hex: $0.color) } ?? MeetPalette.ink
    }

    @ViewBuilder
    private func tile(forKey key: String, size: CGFloat) -> some View {
        if key == meetMainKey {
            leaderTile(size: size)
        } else if let t = model.catalog.template(key) {
            templateTile(t, size: size)
        }
    }

    @ViewBuilder
    private var offlineCard: some View {
        if case .failed(let why) = model.rosterState {
            VStack(alignment: .leading, spacing: 8) {
                Text("Your bots live on your computer")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(MeetPalette.ink)
                Text("We couldn’t reach it right now (\(why)). You can still pick your team; it’s made when your computer is online.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(MeetPalette.sub)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Try again") { Task { await model.loadRoster() } }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(MeetPalette.ink)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 12))
            .padding(.bottom, 12)
        }
    }

    // MARK: Steps

    @ViewBuilder
    private var stepBody: some View {
        switch model.step {
        case .start: startStep
        case .team: teamStep
        case .main: mainStep
        case .routines: routinesStep
        case .notifications: notificationsStep
        case .job: jobStep
        case .done: doneStep
        }
    }

    private var startStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(model.entry == .setup ? "BOTS" : "BOT MODE · \(model.leader.uppercased())")
                .font(MeetPalette.mono(12, .semibold))
                .tracking(2.2)
                .foregroundStyle(MeetPalette.coral)
                .padding(.top, 8)
            Text(model.entry == .setup ? "Your bots hand work to each other." : "Welcome to \(model.leader)’s team.")
                .font(.system(size: 30, weight: .semibold))
                .tracking(-0.6)
                .lineSpacing(-2)
                .foregroundStyle(MeetPalette.ink)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 12)
                .accessibilityAddTraits(.isHeader)
            Text(model.entry == .setup
                 ? "Pick ready-made bots, choose who leads them, and see when they’ll work. You approve what goes out."
                 : "\(model.leader.capitalizedFirst) leads your bots. Add a few, see their week, and give \(model.leader) a first job.")
                .font(.system(size: 15))
                .lineSpacing(3.5)
                .foregroundStyle(MeetPalette.sub)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 12)
            if model.rosterState == .loading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading your bots…").font(.system(size: 13)).foregroundStyle(MeetPalette.sub)
                }
                .padding(.top, 16)
            }
            offlineCard.padding(.top, 16)
        }
    }

    private var teamStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            h2(model.entry == .setup ? "Who’s on your team?" : "Add bots to \(model.leader)’s team")
            sub("Drag a bot onto your team or tap to add. Each one is a ready-made template you can change later.")
            offlineCard
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    chip("All \(model.catalog.templates.count)", value: "all")
                    ForEach(model.catalog.categories, id: \.self) { c in
                        chip(MeetBotCatalog.categoryLabel(c), value: c)
                    }
                }
                .padding(.bottom, 8)
            }
            .accessibilityLabel("Filter bots by kind")
            VStack(spacing: 8) {
                ForEach(model.shownTemplates) { t in
                    templateCard(t, on: model.isOnTeam(t.id), have: model.existingTemplateIds.contains(t.id)) { model.toggle(t.id) }
                }
            }
        }
    }

    private func templateCard(_ t: MeetBotTemplate, on: Bool, have: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                templateTile(t)
                VStack(alignment: .leading, spacing: 1) {
                    Text(t.name).font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(on ? MeetPalette.bg : MeetPalette.ink)
                    Text(have ? "Already on your account" : t.tagline)
                        .font(.system(size: 12))
                        .lineSpacing(1.5)
                        .foregroundStyle(on ? MeetPalette.bg.opacity(0.72) : MeetPalette.mut)
                        .multilineTextAlignment(.leading)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Image(systemName: on ? "checkmark" : "plus")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(on ? .white : MeetPalette.sub)
                    .frame(width: 26, height: 26)
                    .background(on ? MeetPalette.coral : MeetPalette.bg, in: Circle())
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(on ? MeetPalette.ink : MeetPalette.fill, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .animation(.easeOut(duration: 0.18), value: on)
        }
        .buttonStyle(MeetPressStyle())
        .accessibilityLabel("\(t.name): \(t.tagline)")
        .accessibilityValue(on ? "On your team" : "Not on your team")
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func chip(_ label: String, value: String) -> some View {
        let on = model.category == value
        return Button { model.category = value } label: {
            Text(label)
                .font(.system(size: 12.5))
                .foregroundStyle(on ? MeetPalette.bg : MeetPalette.sub)
                .padding(.horizontal, 12)
                .frame(minHeight: 34)
                .background(on ? MeetPalette.ink : MeetPalette.fill, in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private var mainStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            h2("Who leads your team?")
            sub("Your main bot talks to you and hands each job to the bot that owns it.")
            HStack(alignment: .top, spacing: 8) {
                leaderTile(size: 28)
                Text(mainBubble)
                    .font(.system(size: 13))
                    .lineSpacing(2)
                    .foregroundStyle(MeetPalette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(MeetPalette.fill, in: UnevenRoundedRectangle(topLeadingRadius: 16, bottomLeadingRadius: 6, bottomTrailingRadius: 16, topTrailingRadius: 16))
                    .accessibilityAddTraits(.updatesFrequently)
            }
            .padding(.bottom, 12)
            .id(mainBubble)
            .transition(.opacity.combined(with: .offset(y: 6)))

            monoLabel("Allternit")
            picks {
                pick(label: "A://", selected: model.mainChoice == .primary) {
                    model.mainChoice = .primary
                } tile: { MeetLookTile(id: "a", size: 52, live: model.mainChoice == .primary) }
                ForEach(model.catalog.looks) { look in
                    pick(label: look.label, selected: isLook(look.id)) {
                        if model.lookName.isEmpty || model.catalog.looks.contains(where: { $0.label == model.lookName }) {
                            model.lookName = look.label
                        }
                        model.mainChoice = .look(id: look.id, name: model.lookName)
                    } tile: { MeetLookTile(id: look.id, size: 52, live: isLook(look.id)) }
                }
            }
            .padding(.bottom, 12)

            monoLabel("Bots you already use")
            picks {
                ForEach(model.roster, id: \.id) { bot in
                    pick(label: bot.displayName, selected: model.mainChoice == .bot(id: bot.id)) {
                        model.mainChoice = .bot(id: bot.id)
                    } tile: { botTile(bot, size: 52) }
                }
                ForEach(MeetLookTile.vendors, id: \.id) { v in
                    pick(label: v.label, selected: false) { leadSheet = .vendor } tile: { MeetLookTile(id: v.id, size: 52) }
                        .disabled(model.rosterState != .loaded)
                }
                pick(label: "New bot", selected: false) { leadSheet = .new } tile: { actionTile("plus") }
                    .disabled(model.rosterState != .loaded)
                pick(label: "Import", selected: false) { leadSheet = .importBot } tile: { actionTile("square.and.arrow.down") }
                    .disabled(model.rosterState != .loaded)
            }
            .padding(.bottom, 12)

            if case .look = model.mainChoice {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Name").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(MeetPalette.ink)
                    TextField("Name", text: $model.lookName)
                        .textInputAutocapitalization(.words)
                        .font(.system(size: 14))
                        .padding(.horizontal, 12)
                        .frame(height: 44)
                        .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(MeetPalette.line))
                        .accessibilityLabel("Main bot name")
                    Text("Shown in the Bots rail and in every thread.").font(.system(size: 12)).foregroundStyle(MeetPalette.mut)
                }
                .padding(.bottom, 12)
            }
            DisclosureGroup(isExpanded: $discOpen) {
                VStack(alignment: .leading, spacing: 10) {
                    disclosureRow("checkmark.circle", "Answers you directly, or hands a job to the bot that owns it and tells you who has it.")
                    disclosureRow("eye", "Sees the status of your bots’ threads (titles and progress), not their conversations. Incognito threads stay private.")
                    disclosureRow("lock", "Never sends, posts, pays, signs or deploys anything without your approval.")
                    disclosureRow("shuffle", "You can change your main bot any time from a bot’s ⋯ menu.")
                }
                .padding(.top, 10)
            } label: {
                Text("What your main bot can and can’t do")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(MeetPalette.ink)
            }
            .tint(MeetPalette.sub)
            if let note = model.setupNote {
                Text(note).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 10)
            }
            offlineCard.padding(.top, 12)
        }
    }

    private func picks<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 64, maximum: 76), spacing: 8, alignment: .top)], alignment: .leading, spacing: 8) {
            content()
        }
    }

    private func actionTile(_ icon: String) -> some View {
        Image(systemName: icon)
            .font(.system(size: 19))
            .foregroundStyle(MeetPalette.mut)
            .frame(width: 52, height: 52)
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(MeetPalette.lineDash, style: StrokeStyle(lineWidth: 2, dash: [5, 4])))
    }

    private func disclosureRow(_ icon: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).font(.system(size: 14)).foregroundStyle(MeetPalette.ink).frame(width: 18)
            Text(text).font(.system(size: 13)).foregroundStyle(MeetPalette.sub).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func isCustom(_ id: String?) -> Bool {
        id.flatMap { id in model.routines.first { $0.id == id }?.custom } ?? false
    }

    private func edit(_ r: MeetRoutineDraft) {
        editing = RoutineEditTarget(id: r.id, draftId: r.id, title: r.title,
                                    rule: r.rule ?? RoutineRule(repeatKind: .weekdays, at: 9 * 60, days: RoutineRule.weekdays),
                                    botKey: r.botKey)
    }

    /// A new routine at `minute` on `day`: weekdays on a weekday, that weekday otherwise (web ruleForSlot).
    private func addRoutine(on day: Date = Date(), at minute: Int? = nil) {
        let at = minute ?? min(21 * 60, (Calendar.current.component(.hour, from: Date()) + 1) * 60)
        let dow = Calendar.current.component(.weekday, from: day) - 1
        let rule = (1...5).contains(dow)
            ? RoutineRule(repeatKind: .weekdays, at: at, days: RoutineRule.weekdays)
            : RoutineRule(repeatKind: .weekly, at: at, days: [dow])
        editing = RoutineEditTarget(id: "new", draftId: nil, title: "", rule: rule, botKey: meetMainKey)
    }

    private func isLook(_ id: String) -> Bool {
        if case .look(let picked, _) = model.mainChoice { return picked == id }
        return false
    }

    private var mainBubble: String {
        switch model.mainChoice {
        case .bot(let id):
            let name = model.roster.first(where: { $0.id == id })?.displayName ?? "This bot"
            return "\(name) keeps its own memory and leads your team."
        case .look:
            let name = model.lookName.trimmingCharacters(in: .whitespaces)
            return (model.catalog.primary.welcome ?? "").replacingOccurrences(of: "{name}", with: name.isEmpty ? "A://" : name)
        case .primary:
            return (model.catalog.primary.welcome ?? "").replacingOccurrences(of: "{name}", with: "A://")
        }
    }

    private func pick<Tile: View>(label: String, selected: Bool, action: @escaping () -> Void, @ViewBuilder tile: () -> Tile) -> some View {
        Button(action: action) {
            VStack(spacing: 4) {
                tile()
                    .overlay(
                        RoundedRectangle(cornerRadius: 16, style: .continuous)
                            .stroke(MeetPalette.ink, lineWidth: selected ? 2 : 0)
                            .padding(-4)
                    )
                Text(label)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(MeetPalette.ink)
                    .multilineTextAlignment(.center)
                    .lineLimit(2)
            }
            .padding(4)
            .opacity(selected ? 1 : 0.78)
        }
        .buttonStyle(MeetPressStyle(scale: 0.94))
        .accessibilityLabel(label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var routinesStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            h2("When your bots work")
            sub("Your bots come with these routines. Pick a time on the calendar to add one, or open a routine to change when it runs.")
            VStack(spacing: 8) {
                ForEach(model.routines) { r in
                    HStack(spacing: 8) {
                        Button { edit(r) } label: {
                            HStack(spacing: 12) {
                                tile(forKey: r.botKey, size: 32)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(r.title).font(.system(size: 14, weight: .medium)).foregroundStyle(MeetPalette.ink)
                                        .multilineTextAlignment(.leading)
                                    Text("\(model.routineBotName(r.botKey)) · \(r.scheduleLabel)")
                                        .font(.system(size: 12)).foregroundStyle(MeetPalette.mut).lineLimit(2)
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.vertical, 10)
                            .padding(.leading, 12)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Edit \(r.title), \(model.routineBotName(r.botKey)), \(r.scheduleLabel)\(r.enabled ? "" : ", paused")")
                        Toggle("", isOn: Binding(get: { r.enabled }, set: { _ in model.toggleRoutine(r.id) }))
                            .labelsHidden()
                            .tint(MeetPalette.coral)
                            .accessibilityLabel("\(r.enabled ? "Pause" : "Resume") \(r.title)")
                    }
                    .padding(.trailing, 12)
                    .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 12))
                }
                if model.routines.isEmpty {
                    Text("None of your bots run on a schedule yet.")
                        .font(.system(size: 13)).foregroundStyle(MeetPalette.sub)
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 12))
                }
                Button { addRoutine() } label: {
                    Text("+ Add a routine")
                        .font(.system(size: 13.5, weight: .medium))
                        .foregroundStyle(MeetPalette.sub)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .overlay(RoundedRectangle(cornerRadius: 12).stroke(MeetPalette.lineDash, style: StrokeStyle(lineWidth: 2, dash: [5, 4])))
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var notificationsStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            h2("Know when a bot needs you")
            sub(notificationsOn
                ? "Notifications are on. You’ll hear when a bot asks for your OK, finishes a job, or messages you. Pick which ones in Settings."
                : "Bots work while you’re away. We’ll tell you when one asks for your OK, finishes a job, or messages you, nothing else. You choose which ones in Settings.")
        }
    }

    private var jobStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            h2("Give \(model.leader) a first job")
            sub("Each bot comes with a first job that proves it works. Pick one and edit it if you like.")
            VStack(spacing: 8) {
                ForEach(model.jobs) { job in
                    let on = model.jobKey == job.key
                    VStack(alignment: .leading, spacing: 0) {
                        Button { if !on { model.selectJob(job) } } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 8) {
                                    tile(forKey: job.key, size: 28)
                                    Text(job.name).font(.system(size: 14, weight: .semibold)).foregroundStyle(MeetPalette.ink)
                                    Spacer(minLength: 0)
                                    if job.key == meetMainKey {
                                        Text("MAIN BOT")
                                            .font(MeetPalette.mono(10.5, .semibold))
                                            .foregroundStyle(MeetPalette.coral)
                                            .padding(.horizontal, 6)
                                            .background(MeetPalette.coral.opacity(0.16), in: RoundedRectangle(cornerRadius: 4))
                                    }
                                }
                                if !on {
                                    Text(job.prompt)
                                        .font(.system(size: 12.5))
                                        .lineSpacing(1.5)
                                        .foregroundStyle(MeetPalette.sub)
                                        .lineLimit(2)
                                        .multilineTextAlignment(.leading)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                        if on {
                            TextField("First job", text: $model.jobText, axis: .vertical)
                                .lineLimit(4...8)
                                .font(.system(size: 14))
                                .padding(8)
                                .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 8))
                                .overlay(RoundedRectangle(cornerRadius: 8).stroke(MeetPalette.line))
                                .padding(.top, 8)
                                .accessibilityLabel("First job for \(job.name)")
                            if !job.starters.isEmpty {
                                monoLabel("Or try").padding(.top, 8).padding(.bottom, -4)
                                MeetFlow(spacing: 6) {
                                    ForEach(job.starters, id: \.self) { s in
                                        Button { model.jobText = s } label: {
                                            Text(s)
                                                .font(.system(size: 12.5))
                                                .foregroundStyle(MeetPalette.ink)
                                                .multilineTextAlignment(.leading)
                                                .padding(.horizontal, 10)
                                                .padding(.vertical, 4)
                                                .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 8))
                                        }
                                        .buttonStyle(.plain)
                                    }
                                }
                            }
                        }
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(on ? MeetPalette.bg : MeetPalette.fill, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(MeetPalette.ink, lineWidth: on ? 1.5 : 0))
                }
            }
        }
    }

    // MARK: Done

    private struct Row: Identifiable {
        let key: String
        let value: String
        var detail: String?
        var bad = false
        var busy = false
        var id: String { key }
    }

    private var rows: [Row] {
        let items = model.items
        func by(_ k: String) -> MeetFinishItem? { items.first { $0.key == k } }
        let bots = items.filter { $0.key.hasPrefix("bot:") }
        let botBad = bots.filter { $0.status == .failed }
        let main = by("main"), routines = by("routines"), thread = by("thread")
        let jobName = model.jobs.first { $0.key == model.jobKey }?.name
        let n = model.members.count
        let anyFailed = items.contains { $0.status == .failed }
        return [
            Row(key: "main bot", value: model.leader,
                detail: main?.status == .failed ? "Not set up: \(main?.detail ?? "")" : nil,
                bad: main?.status == .failed, busy: main?.status == .running),
            Row(key: "team", value: "\(n) bot\(n == 1 ? "" : "s")",
                detail: botBad.isEmpty ? nil : "Not added: " + botBad.map { "\($0.label) (\($0.detail ?? ""))" }.joined(separator: "; "),
                bad: !botBad.isEmpty, busy: bots.contains { $0.status == .running || $0.status == .pending }),
            Row(key: "routines", value: "\(model.routinesOn) on",
                detail: routines?.status == .failed ? routines?.detail : nil,
                bad: routines?.status == .failed, busy: routines?.status == .running),
            Row(key: "thread #1", value: thread == nil ? "—" : (jobName ?? "—"),
                detail: thread?.status == .failed ? "Didn’t start: \(thread?.detail ?? "")" : nil,
                bad: thread?.status == .failed, busy: thread?.status == .running),
            Row(key: "status", value: model.running ? "setting up…" : anyFailed ? "needs attention" : "running",
                bad: !model.running && anyFailed, busy: model.running),
        ]
    }

    private var doneStep: some View {
        let anyBad = rows.contains { $0.bad }
        let clean = !model.running && !anyBad
        return VStack(alignment: .leading, spacing: 0) {
            h2(model.running ? "Setting up your team" : anyBad ? "Your team is mostly ready" : "Your team is ready")
                .padding(.bottom, 12)
            if clean { checklist } else { summary }
            let suggestions = model.suggestions
            if !suggestions.isEmpty {
                Text("Works well with your team")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(MeetPalette.ink)
                    .padding(.top, 16)
                Text("Your bots hand work to these. Tap one to add it.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(MeetPalette.mut)
                    .padding(.bottom, 8)
                VStack(spacing: 8) {
                    ForEach(suggestions) { t in
                        if model.adding.contains(t.id) {
                            templateCard(t, on: true) {}.overlay(alignment: .trailing) { ProgressView().padding(.trailing, 14) }
                        } else {
                            templateCard(t, on: model.isMade(t.id)) { if !model.isMade(t.id) { model.addLater(t.id) } }
                                .disabled(model.running || model.mainFailed)
                        }
                    }
                }
            }
        }
    }

    /// A mono log of what really happened (web `.mb-sum`).
    private var summary: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { n, row in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("▸ \(row.key)").foregroundStyle(Color(hex: "#7C7C86")).frame(width: 96, alignment: .leading)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(row.value).foregroundStyle(row.bad ? Color(hex: "#F19A9A") : SetupPalette.termFg)
                        if let d = row.detail {
                            Text(d).font(MeetPalette.mono(11.5, .regular)).foregroundStyle(Color(hex: "#7C7C86"))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    Spacer(minLength: 0)
                    if row.key == "status" && !row.bad {
                        Text("●").foregroundStyle(row.busy ? MeetPalette.coral : Color(hex: "#5FD39B"))
                            .opacity(row.busy && !reduceMotion ? 0.8 : 1)
                    }
                }
                .font(MeetPalette.mono(12.5, .regular))
                .lineSpacing(3)
                .accessibilityElement(children: .combine)
                .transition(.opacity.combined(with: .offset(x: -6)))
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(MeetPalette.term, in: RoundedRectangle(cornerRadius: 16))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Setup summary")
    }

    /// Once everything finished cleanly: what was made, ticked, then the next useful step (web OnboardingChecklist).
    private var checklist: some View {
        let botNames = model.items.filter { $0.key.hasPrefix("bot:") }.map(\.label)
        let thread = model.items.first { $0.key == "thread" }
        let jobName = model.jobs.first { $0.key == model.jobKey }?.name
        let n = model.members.count
        var steps: [(String, String?)] = [
            ("Main bot: \(model.leader)", "Answers first and hands work to the team."),
            ("Team: \(n) bot\(n == 1 ? "" : "s")", botNames.isEmpty ? nil : botNames.joined(separator: ", ")),
            ("Routines: \(model.routinesOn) on", model.items.first { $0.key == "routines" }?.detail),
        ]
        if thread != nil { steps.append(("Thread #1 started\(jobName.map { " with \($0)" } ?? "")", thread?.detail)) }
        return VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Finish setting up your team").font(.system(size: 14, weight: .semibold)).foregroundStyle(MeetPalette.ink)
                Spacer()
                Text("\(steps.count)/\(steps.count + 1)").font(MeetPalette.mono(11.5, .semibold)).foregroundStyle(MeetPalette.mut)
            }
            .padding(.bottom, 10)
            ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                checkRow(done: true, title: step.0, detail: step.1)
            }
            HStack(alignment: .top, spacing: 10) {
                Circle().stroke(MeetPalette.lineDash, lineWidth: 1.5).frame(width: 18, height: 18).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Connect ChatGPT or Claude").font(.system(size: 13.5, weight: .medium)).foregroundStyle(MeetPalette.ink)
                    Text("Your bots use your own subscription instead of credits.").font(.system(size: 12)).foregroundStyle(MeetPalette.mut)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Open Connections") { connectionsOpen = true }
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(MeetPalette.bg)
                        .padding(.horizontal, 12)
                        .frame(height: 30)
                        .background(MeetPalette.ink, in: RoundedRectangle(cornerRadius: 8))
                        .padding(.top, 6)
                }
            }
            .padding(.vertical, 8)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(MeetPalette.line))
    }

    private func checkRow(done: Bool, title: String, detail: String?) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "checkmark.circle.fill").font(.system(size: 18)).foregroundStyle(SetupPalette.ok)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13.5, weight: .medium)).foregroundStyle(MeetPalette.sub).strikethrough(false)
                if let detail, !detail.isEmpty {
                    Text(detail).font(.system(size: 12)).foregroundStyle(MeetPalette.mut).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityValue("Done")
    }
}

/// Press feedback for the card's buttons (web `:active { scale(.97) }`).
private struct MeetPressStyle: ButtonStyle {
    var scale: CGFloat = 0.97
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

/// Wrapping row of chips ("Or try").
private struct MeetFlow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, row: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(ProposedViewSize(width: width, height: nil))
            if x > 0 && x + size.width > width { x = 0; y += row + spacing; row = 0 }
            x += size.width + spacing
            row = max(row, size.height)
        }
        return CGSize(width: width == .infinity ? x : width, height: y + row)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, row: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(ProposedViewSize(width: bounds.width, height: nil))
            if x > bounds.minX && x + size.width > bounds.maxX { x = bounds.minX; y += row + spacing; row = 0 }
            s.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(width: min(size.width, bounds.width), height: size.height))
            x += size.width + spacing
            row = max(row, size.height)
        }
    }
}

private extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
