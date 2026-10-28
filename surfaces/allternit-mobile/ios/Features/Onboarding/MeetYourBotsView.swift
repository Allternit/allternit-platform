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

/// "Meet your bots" for the iPhone — the web card (allternit-ai
/// `src/components/bots/meet/`) as a phone step flow: start → team → main
/// bot (first run only) → routines → notifications → first job → done.
/// Nothing is created until "Start the job"; the done step reports each
/// item's real result, with Retry for what didn't finish.
struct MeetYourBotsView: View {
    /// The bot whose chat should open (thread #1's bot, or the main bot), or nil.
    let onFinish: (_ openBotId: String?) -> Void

    @StateObject private var model = MeetYourBotsModel()
    @ObservedObject private var hub = AgentHubStore.shared
    @State private var notificationsOn = false
    @State private var askingNotifications = false
    @State private var leadSheet: LeadSheet?
    @State private var editing: RoutineEditTarget?

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
        VStack(spacing: 0) {
            header
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Color.clear.frame(height: 0).id("top")
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
                    .padding(.horizontal, 20)
                    .padding(.top, 8)
                    .padding(.bottom, 24)
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: model.stepIndex) { _, _ in proxy.scrollTo("top", anchor: .top) }
            }
            footer
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .animation(.easeOut(duration: 0.22), value: model.stepIndex)
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
        .task {
            hub.fetchAgentsIfNeeded()
            await model.loadRoster()
            let status = await PushNotificationManager.shared.authorizationStatus()
            notificationsOn = status == .authorized || status == .provisional
        }
    }

    // MARK: Chrome

    private var header: some View {
        HStack(spacing: 12) {
            HStack(spacing: 6) {
                ForEach(Array(model.steps.enumerated()), id: \.offset) { n, _ in
                    Capsule()
                        .fill(n <= model.stepIndex ? Color("AccentPrimary") : Color("TextSecondary").opacity(0.25))
                        .frame(width: n == model.stepIndex ? 22 : 8, height: 6)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Step \(model.stepIndex + 1) of \(model.steps.count)")
            Spacer()
            if model.stepIndex > 0 && model.step != .done {
                Button("Back") { model.back() }
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
            }
            if model.step != .start && model.step != .done {
                Button("Skip") { onFinish(nil) }
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    @ViewBuilder
    private var footer: some View {
        VStack(spacing: 10) {
            switch model.step {
            case .start:
                primaryButton(model.entry == .setup ? "Build my team" : "Let’s go") { model.next() }
                secondaryButton(model.entry == .setup ? "Skip for now" : "I’ll look around") { onFinish(nil) }
            case .team:
                let n = model.members.count
                primaryButton(n == 0 ? "Add at least one bot" : "Continue with \(n) bot\(n > 1 ? "s" : "")", disabled: n == 0) { model.next() }
            case .main:
                primaryButton("Continue") { model.next() }
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
                if !notificationsOn { secondaryButton("Not now") { model.next() } }
            case .job:
                primaryButton("Start the job", disabled: !model.canStart) { model.startJob() }
            case .done:
                primaryButton(model.running ? "Setting up…" : "Open my bots", disabled: model.running) {
                    onFinish(model.openBotId)
                }
                if model.failed { secondaryButton("Retry what didn’t finish") { model.retry() } }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 8)
        .padding(.bottom, 16)
    }

    private func primaryButton(_ title: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.headline)
                .foregroundColor(.white)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 15)
                .background(Color("AccentPrimary").opacity(disabled ? 0.45 : 1))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
        }
        .disabled(disabled)
    }

    private func secondaryButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(title, action: action)
            .font(.subheadline)
            .foregroundColor(Color("TextSecondary"))
    }

    // MARK: Pieces

    private func title(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 26, weight: .bold))
            .foregroundColor(Color("TextPrimary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func sub(_ text: String) -> some View {
        Text(text)
            .font(.subheadline)
            .foregroundColor(Color("TextSecondary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func card<Content: View>(selected: Bool = false, @ViewBuilder _ content: () -> Content) -> some View {
        content()
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous)
                    .stroke(selected ? Color("AccentPrimary") : Color.clear, lineWidth: 1.5)
            )
    }

    private func templateTile(_ t: MeetBotTemplate, size: CGFloat = 40) -> some View {
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
    private func leaderTile(size: CGFloat = 40) -> some View {
        if model.entry == .botmode, let main = model.mainBot {
            botTile(main, size: size)
        } else {
            switch model.mainChoice {
            case .primary:
                MeetPrimaryTile(size: size)
            case .look(let id, _):
                if let look = model.catalog.looks.first(where: { $0.id == id }) {
                    MeetTemplateTile(color: look.field, spriteURL: look.spriteURL, size: size, label: look.id)
                } else {
                    MeetPrimaryTile(size: size)
                }
            case .bot(let id):
                if let bot = model.roster.first(where: { $0.id == id }) { botTile(bot, size: size) } else { MeetPrimaryTile(size: size) }
            }
        }
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
            card {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Your bots live on your computer")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("We couldn’t reach it right now (\(why)). You can still pick your team; it’s made when your computer is online.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Button("Try again") { Task { await model.loadRoster() } }
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
        }
    }

    // MARK: Steps

    private var startStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(model.entry == .setup ? "BOTS" : "BOT MODE · \(model.leader.uppercased())")
                .font(.caption.weight(.semibold).monospaced())
                .foregroundColor(Color("TextSecondary"))
            Text(model.entry == .setup ? "Your bots hand work to each other." : "Welcome to your team.")
                .font(.system(size: 30, weight: .bold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            sub(model.entry == .setup
                ? "Pick ready-made bots, choose who leads them, and see when they’ll work. You approve what goes out."
                : "\(model.leader.capitalizedFirst) leads your bots. Add a few, see their week, and give \(model.leader) a first job.")
            if model.rosterState == .loading {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading your bots…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
            }
            offlineCard
        }
    }

    private var teamStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            title(model.entry == .setup ? "Who’s on your team?" : "Add bots to \(model.leader)’s team")
            sub("Tap a bot to add it. Each one is a ready-made template you can change later.")
            offlineCard
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    chip("All", value: "all", count: model.catalog.templates.count)
                    ForEach(model.catalog.categories, id: \.self) { c in
                        chip(MeetBotCatalog.categoryLabel(c), value: c, count: nil)
                    }
                }
            }
            ForEach(model.shownTemplates) { t in
                let on = model.isOnTeam(t.id)
                let have = model.existingTemplateIds.contains(t.id)
                Button { model.toggle(t.id) } label: {
                    card(selected: on) {
                        HStack(spacing: 12) {
                            templateTile(t)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(t.name).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                Text(have ? "Already on your account" : t.tagline)
                                    .font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2)
                                    .multilineTextAlignment(.leading)
                            }
                            Spacer(minLength: 8)
                            Image(systemName: on ? "checkmark.circle.fill" : "plus.circle")
                                .font(.system(size: 20))
                                .foregroundColor(on ? Color("AccentPrimary") : Color("TextSecondary"))
                        }
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(t.name), \(t.tagline)")
                .accessibilityValue(on ? "On your team" : "Not on your team")
                .accessibilityHint(on ? "Removes it from the team" : "Adds it to the team")
            }
        }
    }

    private func chip(_ label: String, value: String, count: Int?) -> some View {
        let on = model.category == value
        return Button { model.category = value } label: {
            Text(count.map { "\(label) \($0)" } ?? label)
                .font(.footnote.weight(.semibold))
                .foregroundColor(on ? .white : Color("TextPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(on ? Color("AccentPrimary") : Color("BgSecondary"))
                .clipShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private var mainStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            title("Who leads your team?")
            sub("Your main bot talks to you and hands each job to the bot that owns it.")
            card {
                HStack(alignment: .top, spacing: 10) {
                    leaderTile(size: 28)
                    Text(mainBubble)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Text("ALLTERNIT").font(.caption2.weight(.semibold).monospaced()).foregroundColor(Color("TextSecondary"))
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 72), spacing: 10)], spacing: 12) {
                pick(label: "A://", selected: model.mainChoice == .primary) {
                    model.mainChoice = .primary
                } tile: { MeetPrimaryTile(size: 52) }
                ForEach(model.catalog.looks) { look in
                    pick(label: look.label, selected: isLook(look.id)) {
                        if model.lookName.isEmpty || model.catalog.looks.contains(where: { $0.label == model.lookName }) {
                            model.lookName = look.label
                        }
                        model.mainChoice = .look(id: look.id, name: model.lookName)
                    } tile: { MeetTemplateTile(color: look.field, spriteURL: look.spriteURL, size: 52, live: isLook(look.id), label: look.id) }
                }
            }
            if case .look = model.mainChoice {
                TextField("Name", text: $model.lookName)
                    .textInputAutocapitalization(.words)
                    .padding(12)
                    .background(Color("BgSecondary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
                    .accessibilityLabel("Main bot name")
            }
            if !model.roster.isEmpty {
                Text("BOTS YOU ALREADY HAVE").font(.caption2.weight(.semibold).monospaced()).foregroundColor(Color("TextSecondary"))
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 72), spacing: 10)], spacing: 12) {
                    ForEach(model.roster, id: \.id) { bot in
                        pick(label: bot.displayName, selected: model.mainChoice == .bot(id: bot.id)) {
                            model.mainChoice = .bot(id: bot.id)
                        } tile: { botTile(bot, size: 52) }
                    }
                }
            }
            Text("OR BRING ONE").font(.caption2.weight(.semibold).monospaced()).foregroundColor(Color("TextSecondary"))
            leadPath("Connect a vendor bot", detail: "ChatGPT, Claude, Gemini, Grok and more — your own subscription, as a bot.",
                     icon: "link") { leadSheet = .vendor }
            leadPath("Make a new bot", detail: "Give it a job, a name and a look.", icon: "plus.circle") { leadSheet = .new }
            leadPath("Import a bot", detail: "Bring in a team file or bot package you already have.", icon: "square.and.arrow.down") { leadSheet = .importBot }
            if let note = model.setupNote {
                Text(note).font(.caption).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
            }
            offlineCard
        }
    }

    private func leadPath(_ title: String, detail: String, icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            card {
                HStack(spacing: 12) {
                    Image(systemName: icon)
                        .font(.system(size: 17, weight: .medium))
                        .foregroundColor(Color("AccentPrimary"))
                        .frame(width: 36, height: 36)
                        .background(Color("AccentPrimary").opacity(0.12))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                        Text(detail).font(.caption).foregroundColor(Color("TextSecondary")).multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.right").font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .buttonStyle(.plain)
        .disabled(model.rosterState != .loaded)
    }

    private func isCustom(_ id: String?) -> Bool {
        id.flatMap { id in model.routines.first { $0.id == id }?.custom } ?? false
    }

    private func edit(_ r: MeetRoutineDraft) {
        editing = RoutineEditTarget(id: r.id, draftId: r.id, title: r.title,
                                    rule: r.rule ?? RoutineRule(repeatKind: .weekdays, at: 9 * 60, days: RoutineRule.weekdays),
                                    botKey: r.botKey)
    }

    /// RoutinesStep "+ Add a routine": the next whole hour, at most 9 PM, weekdays or today.
    private func addRoutine() {
        let at = min(21 * 60, (Calendar.current.component(.hour, from: Date()) + 1) * 60)
        let dow = Calendar.current.component(.weekday, from: Date()) - 1
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
            VStack(spacing: 6) {
                tile()
                    .overlay(
                        RoundedRectangle(cornerRadius: 15, style: .continuous)
                            .stroke(selected ? Color("AccentPrimary") : Color.clear, lineWidth: 2)
                            .padding(-3)
                    )
                Text(label)
                    .font(.caption)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var routinesStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            title("When your bots work")
            sub("Your bots come with these routines. Turn off any you don’t want, or tap one to change its time.")
            if model.routines.isEmpty {
                card {
                    Text("None of your bots run on a schedule yet.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            ForEach(model.routines) { r in
                card {
                    HStack(spacing: 12) {
                        Button { edit(r) } label: {
                            HStack(spacing: 12) {
                                tile(forKey: r.botKey, size: 32)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(r.title).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                        .multilineTextAlignment(.leading)
                                    Text("\(model.routineBotName(r.botKey)) · \(r.scheduleLabel)")
                                        .font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2)
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint("Change when it runs")
                        Spacer(minLength: 8)
                        Toggle("", isOn: Binding(get: { r.enabled }, set: { _ in model.toggleRoutine(r.id) }))
                            .labelsHidden()
                            .tint(Color("AccentPrimary"))
                            .accessibilityLabel("\(r.title), \(r.scheduleLabel)")
                    }
                }
            }
            Button { addRoutine() } label: {
                Label("Add a routine", systemImage: "plus")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("AccentPrimary"))
            }
            if model.routines.count > 0 {
                sub("\(model.routinesOn) of \(model.routines.count) on. Tap one to change when it runs. Routines run on your computer, even while your phone is in your pocket.")
            }
        }
    }

    private var notificationsStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Image(systemName: notificationsOn ? "bell.badge.fill" : "bell.badge")
                .font(.system(size: 30, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 64, height: 64)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG, style: .continuous))
                .accessibilityHidden(true)
            title("Know when a bot needs you")
            sub(notificationsOn
                ? "Notifications are on. You’ll hear when a bot asks for your OK, finishes a job, or messages you. Pick which ones in Settings."
                : "Bots work while you’re away. We’ll tell you when one asks for your OK, finishes a job, or messages you — nothing else. You choose which ones in Settings.")
        }
    }

    private var jobStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            title("Give \(model.leader) a first job")
            sub("Each bot comes with a first job that proves it works. Pick one and edit it if you like.")
            ForEach(model.jobs) { job in
                let on = model.jobKey == job.key
                card(selected: on) {
                    VStack(alignment: .leading, spacing: 10) {
                        Button { model.selectJob(job) } label: {
                            VStack(alignment: .leading, spacing: 6) {
                                HStack(spacing: 8) {
                                    tile(forKey: job.key, size: 28)
                                    Text(job.name).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                    if job.key == meetMainKey {
                                        Text("MAIN BOT")
                                            .font(.caption2.weight(.bold).monospaced())
                                            .foregroundColor(Color("AccentPrimary"))
                                    }
                                    Spacer(minLength: 0)
                                }
                                if !on {
                                    Text(job.prompt)
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                        .lineLimit(3)
                                        .multilineTextAlignment(.leading)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                        if on {
                            TextField("First job", text: $model.jobText, axis: .vertical)
                                .lineLimit(3...8)
                                .padding(10)
                                .background(Color("BgPrimary"))
                                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous))
                                .accessibilityLabel("First job for \(job.name)")
                            Text("You’ll get: \(job.expect)")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                                .fixedSize(horizontal: false, vertical: true)
                            if !job.starters.isEmpty {
                                Text("OR TRY").font(.caption2.weight(.semibold).monospaced()).foregroundColor(Color("TextSecondary"))
                                ForEach(job.starters, id: \.self) { s in
                                    Button { model.jobText = s } label: {
                                        Text(s)
                                            .font(.caption)
                                            .foregroundColor(Color("TextPrimary"))
                                            .multilineTextAlignment(.leading)
                                            .padding(.horizontal, 10)
                                            .padding(.vertical, 6)
                                            .overlay(Capsule().stroke(Color("TextSecondary").opacity(0.25), lineWidth: 1))
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                    }
                }
            }
            Label("Nothing is sent, posted or paid without your approval.", systemImage: "lock")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
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
                detail: thread?.status == .failed ? "Didn’t start: \(thread?.detail ?? "")" : (thread?.status == .ok ? "Running" : nil),
                bad: thread?.status == .failed, busy: thread?.status == .running),
        ]
    }

    private var doneStep: some View {
        let anyBad = rows.contains { $0.bad }
        return VStack(alignment: .leading, spacing: 14) {
            title(model.running ? "Setting up your team" : anyBad ? "Your team is mostly ready" : "Your team is ready")
            card {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(rows) { row in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Text("▸ \(row.key)")
                                .font(.caption.monospaced())
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 92, alignment: .leading)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(row.value)
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundColor(row.bad ? Theme.statusError : Color("TextPrimary"))
                                if let d = row.detail {
                                    Text(d)
                                        .font(.caption)
                                        .foregroundColor(row.bad ? Theme.statusError : Color("TextSecondary"))
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                            }
                            Spacer(minLength: 0)
                            if row.busy && model.running {
                                ProgressView().controlSize(.small)
                            } else if row.bad {
                                Image(systemName: "exclamationmark.circle.fill").foregroundColor(Theme.statusError)
                            } else {
                                Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
            .accessibilityElement(children: .contain)
            if !model.running && !anyBad {
                sub("Tip: connect ChatGPT or Claude in Settings → Subscriptions so your bots use your own subscription instead of credits.")
            }
            let suggestions = model.suggestions
            if !suggestions.isEmpty {
                Text("Works well with your team")
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.top, 6)
                sub("Your bots hand work to these.")
                ForEach(suggestions) { t in
                    card {
                        HStack(spacing: 12) {
                            templateTile(t)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(t.name).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                Text(t.tagline).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2)
                            }
                            Spacer(minLength: 8)
                            if model.adding.contains(t.id) {
                                ProgressView()
                            } else if model.isMade(t.id) {
                                Image(systemName: "checkmark.circle.fill").foregroundColor(Color("AccentPrimary"))
                                    .accessibilityLabel("Added")
                            } else {
                                Button("Add") { model.addLater(t.id) }
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundColor(Color("AccentPrimary"))
                                    .disabled(model.running || model.mainFailed)
                            }
                        }
                    }
                }
            }
        }
    }
}

private extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
