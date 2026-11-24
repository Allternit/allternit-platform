import SwiftUI

// -----------------------------------------------------------------------------
// The layered runtime / brain / bot picker (Harness & Bots 2.0 M1, plan D9,
// D11; approved mockup a5, web AgentCard on ai feat/harness-bots-w3).
//
// Header: the bot's face, name and kind, and the View switch (Allternit /
// Runtime; no Split on a phone). Then search and the List · Grid · Deck switch
// (`OverviewLayoutSwitch`, the same control Connections uses), then three
// layers on a rail over the Allternit control layer. A closed layer is one row
// with its current choice as a chip; the open layer shows its choices. A
// plain sentence at the bottom says what the bot runs on.
//
// Picking a runtime that can't run the current brain switches to the first
// brain that fits and says so in one line (contract "Compatibility"). A
// Connected bot locks Runtime and Brain to "Run by <Vendor>".
//
// Runtimes come from `RuntimeRegistryStore` (cloud-api /api/v1/runtimes,
// cached). Brains come from `ModelStore.automationModels`, the list the bot
// form's model picker uses (never person-only models); the access badge is
// read from the provider id and the sign-in state from `ConnectionsStore`.
//
// Bot Home saves on Save (whole config, unknown keys kept). The bot form gets
// the result through `BotFormModel.applyLayers` and saves with the bot.
//
// Plain chats (the composer's one Runtime·Brain chip) open the `.chat` target:
// Runtime and Brain only, no Bot layer and no View switch. Runtime rows say
// what each runtime needs (pair a computer, not installed) from
// `ChatRuntimeStore`; the Brain layer lists the chat models `ModelPickerSheet`
// offers (plan and subscription models included), with a last row that opens
// `ModelPickerSheet` for effort, limits and device models. Picks apply on tap
// to `ChatRuntimeStore.runtimeId` and `ModelStore.selectedModelId`; a runtime
// pick in a chat already under way opens a fresh chat on it.
// -----------------------------------------------------------------------------

struct BotBrainPickerSheet: View {
    private enum Target {
        case agent(AgentRecord)
        case form(BotFormModel)
        /// A plain chat; `hasActiveSession` = a runtime pick opens a fresh chat.
        case chat(hasActiveSession: Bool)
    }

    /// One layer of the picker; the composer chips open the sheet on one.
    enum Layer: String, CaseIterable, Identifiable {
        case runtime = "Runtime", brain = "Brain", bot = "Bot"
        var id: String { rawValue }
    }

    @State private var target: Target
    /// In a bot chat (composer chips) the Bot layer opens the other bot's
    /// chat instead of moving the picker to it.
    private var onPickBot: (@MainActor (AgentRecord) -> Void)? = nil
    @StateObject private var registry = RuntimeRegistryStore.shared
    @StateObject private var modelStore = ModelStore.shared
    @StateObject private var hubStore = AgentHubStore.shared
    @ObservedObject private var connections = ConnectionsStore.shared
    @ObservedObject private var chatRuntime = ChatRuntimeStore.shared
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @Environment(\.dismiss) private var dismiss
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @AppStorage("agentCard.layout") private var layoutRaw = OverviewLayout.grid.rawValue

    /// The bot's `config.botBrain`, whole (keys this app doesn't model stay).
    @State private var brain: [String: JSONValue]
    @State private var view: String
    @State private var look: String?
    @State private var openLayer: Layer? = .runtime
    @State private var query = ""
    @State private var note: String?
    @State private var isSaving = false
    @State private var saveError: String?
    @State private var dirty = false
    /// Chat target: the runtime row whose "what it needs" line is shown.
    @State private var focusedRuntime: String?
    @State private var isPairingPresented = false
    @State private var isModelOptionsPresented = false
    @State private var creditsRequest: CloudCreditsRequest?

    /// Bot Home: picks save to the bot. The composer chips pass the layer
    /// that was tapped, so its choices are open.
    init(agent: AgentRecord, open layer: Layer = .runtime, onPickBot: (@MainActor (AgentRecord) -> Void)? = nil) {
        self.onPickBot = onPickBot
        _target = State(initialValue: .agent(agent))
        _openLayer = State(initialValue: layer)
        let start = Self.startState(agent)
        _brain = State(initialValue: start.brain)
        _view = State(initialValue: start.view)
        _look = State(initialValue: start.look)
    }

    /// A bot's stored brain and view. Older bots carry the brain only in the
    /// top-level model/provider, so that becomes its modelRef.
    private static func startState(_ agent: AgentRecord) -> (brain: [String: JSONValue], view: String, look: String?) {
        let config = agent.config ?? [:]
        var initial = config["botBrain"]?.jObject ?? [:]
        if initial["modelRef"] == nil, !agent.model.isEmpty, !agent.provider.isEmpty {
            initial["modelRef"] = .object(["providerID": .string(agent.provider), "modelID": .string(agent.model)])
        }
        let agentView = config["agentView"]?.jObject
        return (initial, agentView?["view"]?.jString == "runtime" ? "runtime" : "allternit", agentView?["look"]?.jString)
    }

    /// Create bot / Edit bot: picks go back to the form, which saves them.
    init(form: BotFormModel) {
        _target = State(initialValue: .form(form))
        _brain = State(initialValue: form.layersBrain)
        _view = State(initialValue: form.agentViewMode)
        _look = State(initialValue: form.agentLook)
    }

    /// A plain chat (the composer's Runtime·Brain chip): picks apply on tap to
    /// the chat stores; there is nothing to save.
    init(chat hasActiveSession: Bool, open layer: Layer = .runtime) {
        _target = State(initialValue: .chat(hasActiveSession: hasActiveSession))
        _openLayer = State(initialValue: layer == .bot ? .runtime : layer)
        _brain = State(initialValue: [:])
        _view = State(initialValue: "allternit")
        _look = State(initialValue: nil)
    }

    // MARK: Derived state

    private var isChat: Bool { if case .chat = target { return true } else { return false } }
    private var chatHasSession: Bool { if case .chat(let s) = target { return s } else { return false } }
    /// Layers this target shows: a plain chat has no Bot layer.
    private var layers: [Layer] { isChat ? [.runtime, .brain] : Layer.allCases }

    private var layout: OverviewLayout { OverviewLayout(stored: layoutRaw) ?? .grid }

    private var resolved: [String: JSONValue] { RuntimeRegistry.resolveRuntime(brain, in: registry.runtimes) }
    private var runtimeId: String {
        if isChat { return chatRuntime.runtimeId }
        return resolved["runtime"]?.jString ?? "allternit"
    }
    private var runtime: RuntimeEntry? { registry.runtime(runtimeId) }
    private var runtimeName: String {
        if runtimeId == "custom" { return "Custom harness" }
        return runtime?.name ?? runtimeId
    }

    private var modelRef: (provider: String, model: String)? {
        if isChat {
            guard let id = modelStore.selectedModelId, let slash = id.firstIndex(of: "/") else { return nil }
            return (String(id[..<slash]), String(id[id.index(after: slash)...]))
        }
        guard let ref = brain["modelRef"]?.jObject,
              let p = ref["providerID"]?.jString, let m = ref["modelID"]?.jString, !p.isEmpty, !m.isEmpty else { return nil }
        return (p, m)
    }

    private var botName: String {
        switch target {
        case .agent(let a): return a.botDisplayName
        case .chat: return "This chat"
        case .form(let f):
            let name = f.displayName.trimmingCharacters(in: .whitespacesAndNewlines)
            return name.isEmpty ? "This bot" : name
        }
    }

    private var connectedVendor: String? {
        switch target {
        case .agent(let a): return Self.connectedVendor(a)
        case .form(let f): return f.connectedVendor
        case .chat: return nil
        }
    }
    private var locked: Bool { connectedVendor != nil }

    struct BrainChoice: Identifiable, Equatable {
        let id: String
        let provider: String
        let model: String
        let name: String
        let access: BrainAccess
        let needsSignIn: Bool
    }

    /// The bot form's model list (never person-only), default first. A plain
    /// chat lists what `ModelPickerSheet` offers, the person's plans included.
    private var brains: [BrainChoice] {
        if isChat { return chatBrains }
        var seen = Set<String>()
        let rows = modelStore.automationModels
            .filter { !$0.isHidden && !$0.provider.hasPrefix("openrouter") }
            .sorted { ($0.isDefault ? 0 : 1, $0.rank ?? 999) < ($1.isDefault ? 0 : 1, $1.rank ?? 999) }
        var out: [BrainChoice] = []
        for m in rows {
            let model = m.shortName
            let key = "\(m.provider)/\(model)"
            guard seen.insert(key).inserted else { continue }
            out.append(BrainChoice(id: key, provider: m.provider, model: model, name: m.name,
                                   access: RuntimeRegistry.accessOf(providerId: m.provider),
                                   needsSignIn: needsSignIn(m.provider)))
        }
        return out
    }

    /// `ModelPickerSheet`'s rows (hidden ones only when current), the person's
    /// plans first, then Allternit Cloud, providers and local, each vendor's
    /// default first. The id is the full catalog id `selectedModelId` takes.
    private var chatBrains: [BrainChoice] {
        let selected = modelStore.selectedModelId
        var seen = Set<String>()
        var out: [BrainChoice] = []
        let rows = modelStore.chatModels
            .filter { !$0.isHidden || $0.id == selected }
            .sorted { Self.chatOrder($0).lexicographicallyPrecedes(Self.chatOrder($1)) }
        for m in rows where seen.insert(m.id).inserted {
            out.append(BrainChoice(id: m.id, provider: m.provider, model: m.shortName, name: m.name,
                                   access: RuntimeRegistry.accessOf(providerId: m.provider),
                                   needsSignIn: needsSignIn(m.provider)))
        }
        return out
    }

    /// Sort key for chat brains: provider group, vendor default, vendor rank.
    private static func chatOrder(_ m: RuntimeModel) -> [Int] {
        [ModelProviderNames.rank(m.provider), m.isDefault ? 0 : 1, m.rank ?? 999]
    }

    /// From the Connections rows: a sign-in that expired or needs fixing.
    private func needsSignIn(_ pid: String) -> Bool {
        let ids: Set<String> = [pid, "\(pid)-key", "plan-\(pid)"]
        return connections.rows.values.contains { ids.contains($0.id) && ($0.state == .needs || $0.reasonCode == "login_expired") }
    }

    private var currentBrain: BrainChoice? {
        if isChat { return brains.first { $0.id == modelStore.selectedModelId } }
        guard let ref = modelRef else { return nil }
        return brains.first { $0.id == "\(ref.provider)/\(ref.model)" }
    }

    private var brainName: String {
        if isChat { return ComposerModelLabel.label(modelStore) }
        if let currentBrain { return currentBrain.name }
        if let ref = modelRef { return modelStore.models.first { $0.id == "\(ref.provider)/\(ref.model)" }?.name ?? ref.model }
        return "Default model"
    }

    private var brainAccess: BrainAccess? {
        currentBrain?.access
            ?? brain["access"]?.jString.flatMap(BrainAccess.init(rawValue:))
            ?? modelRef.map { RuntimeRegistry.accessOf(providerId: $0.provider) }
    }

    private var sentence: String {
        if let vendor = connectedVendor {
            return "\(botName) is run by \(vendor). Allternit shows its work and keeps the record."
        }
        if isChat {
            return "This chat works with \(runtimeName) and thinks with \(brainName), \(RuntimeRegistry.accessPhrase(brainAccess, providerId: modelRef?.provider))."
        }
        return "\(botName) works with \(runtimeName) and thinks with \(brainName), \(RuntimeRegistry.accessPhrase(brainAccess, providerId: modelRef?.provider))."
    }

    private func matches(_ fields: String?...) -> Bool {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if q.isEmpty { return true }
        return fields.contains { ($0 ?? "").lowercased().contains(q) }
    }

    private var runtimeChoices: [RuntimeEntry] {
        let searching = !query.trimmingCharacters(in: .whitespaces).isEmpty
        return registry.runtimes
            .filter { !$0.more || $0.id == runtimeId || searching }
            .filter { matches($0.name, $0.vendor, $0.categoryLabel) }
    }

    private var brainChoices: [BrainChoice] {
        brains.filter { matches($0.name, ModelProviderNames.name($0.provider), $0.access.label) }
    }

    private var bots: [AgentRecord] {
        hubStore.agents.filter { $0.isBot }.filter { matches($0.botDisplayName, $0.description) }
    }

    // MARK: Body

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    header
                    searchRow
                    rail
                    if let note {
                        noteLine(note, icon: "info.circle")
                    }
                    Text(sentence)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("agentCardSentence")
                    if let saveError {
                        noteLine(saveError, icon: "exclamationmark.triangle", tone: Theme.statusWarning)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .animation(reduceMotion ? nil : Motion.fade, value: layoutRaw)
                .animation(reduceMotion ? nil : Motion.fade, value: openLayer)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Runtime and brain")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if isChat {
                    // Chat picks already applied on tap: nothing to cancel or save.
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Done") { dismiss() }
                            .fontWeight(.semibold)
                            .accessibilityIdentifier("runtimeBrainDone")
                    }
                } else {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Cancel") { dismiss() }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        Button(action: save) {
                            if isSaving { ProgressView() } else { Text(isForm ? "Done" : "Save").fontWeight(.semibold) }
                        }
                        .disabled(isSaving || (!dirty && !isForm))
                    }
                }
            }
            .task {
                registry.refreshIfNeeded()
                modelStore.fetchModelsIfNeeded()
                if isChat {
                    chatRuntime.refreshIfNeeded()
                } else if hubStore.agents.isEmpty {
                    hubStore.fetchAgentsIfNeeded()
                }
            }
            .sheet(isPresented: $isPairingPresented, onDismiss: { chatRuntime.refreshIfNeeded(force: true) }) {
                RuntimePairingView()
            }
            .sheet(isPresented: $isModelOptionsPresented) {
                ModelPickerSheet(modelStore: modelStore)
            }
            .sheet(item: $creditsRequest) { request in
                CloudCreditsSheet(request: request)
            }
        }
    }

    private var isForm: Bool { if case .form = target { return true } else { return false } }

    // MARK: Header

    @ViewBuilder
    private var header: some View {
        if isChat {
            Text("Where this chat runs and the brain it thinks with. Allternit runs anywhere; the others run on your computer.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        } else {
            botHeader
        }
    }

    private var botHeader: some View {
        HStack(spacing: 12) {
            botFace(size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text(botName)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(locked ? "Connected bot · \(connectedVendor ?? "")" : "Allternit bot")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            .accessibilityElement(children: .combine)
            Spacer(minLength: 8)
            Picker("View", selection: Binding(get: { view }, set: { v in
                Haptics.selection()
                view = v
                if look == nil { look = runtime?.defaultLook }
                dirty = true
            })) {
                Image(systemName: "a.circle").accessibilityLabel("Allternit view").tag("allternit")
                Image(systemName: "terminal").accessibilityLabel("Runtime view").tag("runtime")
            }
            .pickerStyle(.segmented)
            .frame(width: 104)
            .accessibilityLabel("View")
            .accessibilityValue(view == "runtime" ? "Runtime" : "Allternit")
        }
    }

    @ViewBuilder
    private func botFace(size: CGFloat) -> some View {
        switch target {
        case .agent(let a):
            BotAvatarView(bot: a, size: size).accessibilityHidden(true)
        case .chat:
            EmptyView()
        case .form(let f):
            Text(String(botName.prefix(1)).uppercased())
                .font(.system(size: size * 0.45, weight: .semibold))
                .foregroundColor(.white)
                .frame(width: size, height: size)
                .background(Circle().fill(Color(hex: f.accent)))
                .accessibilityHidden(true)
        }
    }

    private var searchRow: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextTertiary"))
                    .accessibilityHidden(true)
                TextField("Search", text: $query)
                    .font(.system(size: 15))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Search runtimes, brains and bots")
                if !query.isEmpty {
                    Button { query = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundColor(Color("TextTertiary"))
                            .frame(width: 44, height: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
                }
            }
            .padding(.leading, 12)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
            OverviewLayoutSwitch(raw: $layoutRaw, fallback: .grid)
        }
    }

    // MARK: Rail

    private var rail: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(layers) { layer in
                layerSection(layer)
            }
            HStack(alignment: .top, spacing: 12) {
                railDot(filled: true)
                Text("Allternit control layer · policy, tool approvals, memory, provenance, billing")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 2)
            }
            .padding(.top, 6)
            .accessibilityElement(children: .combine)
        }
        .background(alignment: .leading) {
            Rectangle().fill(Color("BorderSubtle")).frame(width: 1.5).padding(.leading, 6.25).padding(.vertical, 18)
                .accessibilityHidden(true)
        }
    }

    private func railDot(filled: Bool) -> some View {
        Circle()
            .strokeBorder(Color("TextPrimary"), lineWidth: 1.5)
            .background(Circle().fill(filled ? Color("TextPrimary") : Color("BgPrimary")))
            .frame(width: 14, height: 14)
            .padding(.top, 3)
            .accessibilityHidden(true)
    }

    private func layerLocked(_ layer: Layer) -> Bool {
        switch layer {
        case .runtime, .brain: return locked
        case .bot: return isForm
        }
    }

    @ViewBuilder
    private func layerSection(_ layer: Layer) -> some View {
        let open = openLayer == layer && !layerLocked(layer)
        VStack(alignment: .leading, spacing: 8) {
            Button {
                guard !layerLocked(layer) else { return }
                Haptics.selection()
                openLayer = open ? nil : layer
            } label: {
                HStack(spacing: 12) {
                    railDot(filled: open)
                    Text(layer.rawValue)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer(minLength: 8)
                    layerChip(layer, open: open)
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(layer.rawValue): \(chipText(layer))")
            .accessibilityHint(layerLocked(layer) ? "" : (open ? "Closes the choices" : "Shows the choices"))
            .accessibilityAddTraits(layerLocked(layer) ? [] : .isButton)

            if open {
                VStack(alignment: .leading, spacing: 8) {
                    layerContent(layer)
                }
                .padding(.leading, 26)
                .transition(reduceMotion ? .identity : .opacity)
            }
        }
        .padding(.vertical, 4)
    }

    private func chipText(_ layer: Layer) -> String {
        switch layer {
        case .runtime: return locked ? "Run by \(connectedVendor ?? "")" : runtimeName
        case .brain: return locked ? "Run by \(connectedVendor ?? "")" : brainName
        case .bot: return botName
        }
    }

    private func layerChip(_ layer: Layer, open: Bool) -> some View {
        HStack(spacing: 6) {
            switch layer {
            case .runtime where !locked: ModelProviderMark(providerId: runtimeId, size: 18)
            case .brain where !locked: ModelProviderMark(providerId: modelRef?.provider ?? "allternit", size: 18)
            case .bot: botFace(size: 18)
            default: EmptyView()
            }
            Text(chipText(layer))
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color(layerLocked(layer) && layer != .bot ? "TextSecondary" : "TextPrimary"))
                .lineLimit(1)
            Image(systemName: layerLocked(layer) ? "lock.fill" : (open ? "chevron.up" : "chevron.down"))
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(Color("TextTertiary"))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Capsule().fill(Color("FillNeutral")))
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    // MARK: Layer content

    @ViewBuilder
    private func layerContent(_ layer: Layer) -> some View {
        switch layer {
        case .runtime: runtimeContent
        case .brain: brainContent
        case .bot: botContent
        }
    }

    @ViewBuilder
    private var runtimeContent: some View {
        switch registry.status {
        case .error:
            issueNotice("Couldn't load the latest runtimes. Showing the built-in list.")
        case .stale:
            issueNotice("You're offline. Showing the saved list of runtimes.")
        default:
            EmptyView()
        }
        if registry.status == .loading && !registry.isLoaded {
            skeleton
        } else if runtimeChoices.isEmpty {
            emptyLine("No runtimes match “\(query)”.")
        } else {
            choices(runtimeChoices.map { r in
                if isChat { return chatRuntimeChoice(r) }
                var badges: [Badge] = []
                if r.hasLiveApp { badges.append(Badge(text: "Live app", tone: .live)) }
                badges.append(Badge(text: r.brainsLabel, tone: .plain))
                if let inst = registry.installed?[r.id], !inst.installed, r.tool != nil {
                    badges.append(Badge(text: "Not installed", tone: .warn))
                }
                return Choice(id: r.id, name: r.name, detail: r.status?.label ?? "\(r.categoryLabel) · \(r.vendor)",
                              mark: .provider(r.id), badges: badges, ok: true, selected: r.id == runtimeId)
            }, idPrefix: isChat ? "chatRuntime-" : nil) { id in
                if let r = registry.runtime(id) { isChat ? pickChatRuntime(r) : pickRuntime(r) }
            }
            if isChat { runtimeNeeds }
        }
    }

    /// Chat target: a runtime row with what it needs right now (the old
    /// runtime sheet's lines). Rows that can't run stay tappable: a tap shows
    /// how to fix it below the list.
    private func chatRuntimeChoice(_ r: RuntimeEntry) -> Choice {
        let a = chatRuntime.availability(r, installed: registry.installed)
        var badges: [Badge] = []
        if r.hasLiveApp { badges.append(Badge(text: "Live app", tone: .live)) }
        switch a {
        case .needsComputer: badges.append(Badge(text: "Needs a computer", tone: .warn))
        case .notInstalled: badges.append(Badge(text: "Not installed", tone: .warn))
        case .ready, .checking: badges.append(Badge(text: r.brainsLabel, tone: .plain))
        }
        return Choice(id: r.id, name: r.name, detail: chatRuntimeDetail(r, a), mark: .provider(r.id),
                      badges: badges, ok: true, selected: r.id == runtimeId)
    }

    private func chatRuntimeDetail(_ r: RuntimeEntry, _ a: ChatRuntimeAvailability) -> String {
        switch a {
        case .ready: return r.id == "allternit" ? "Runs anywhere · \(r.brainsLabel)" : "\(r.categoryLabel) · \(r.brainsLabel)"
        case .checking: return "Checking your computer…"
        case .needsComputer: return "Needs a computer"
        case .notInstalled(let computer): return "Not installed on \(computer)"
        }
    }

    /// The tapped (or picked) runtime that can't run yet: why, and the fix.
    @ViewBuilder
    private var runtimeNeeds: some View {
        let id = focusedRuntime ?? runtimeId
        if let r = registry.runtime(id),
           case let a = chatRuntime.availability(r, installed: registry.installed),
           let message = a.message(r.name) {
            VStack(alignment: .leading, spacing: 8) {
                Label(message, systemImage: a == .needsComputer ? "desktopcomputer" : "exclamationmark.triangle")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    if a == .needsComputer {
                        needsButton("Pair a computer") { isPairingPresented = true }
                    }
                    needsButton("Check again") { chatRuntime.refreshIfNeeded(force: true) }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
            .accessibilityElement(children: .contain)
        }
    }

    private func needsButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(Capsule().fill(Color("BgPanel")))
                .overlay(Capsule().stroke(Color("BorderSubtle")))
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var brainContent: some View {
        if modelStore.isLoading && modelStore.models.isEmpty {
            skeleton
        } else if let error = modelStore.loadError, modelStore.models.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                noteLine(error, icon: "exclamationmark.triangle", tone: Theme.statusWarning)
                Button("Try again") { modelStore.fetchModelsIfNeeded(force: true) }
                    .font(.system(size: 13, weight: .medium))
                    .frame(minHeight: 44)
            }
            if isChat { modelOptionsRow }
        } else if brains.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("No brains yet. Connect a plan, an API key or a local model.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                NavigationLink {
                    ConnectionsSettingsView()
                } label: {
                    Text("Open Connections")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 44)
                        .background(Capsule().fill(Color("FillNeutral")))
                        .overlay(Capsule().stroke(Color("BorderSubtle")))
                }
                .buttonStyle(.plain)
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            if isChat { modelOptionsRow }
        } else if brainChoices.isEmpty {
            emptyLine("No brains match “\(query)”.")
            if isChat { modelOptionsRow }
        } else {
            choices(brainChoices.prefix(48).map { b in
                let ok = runtime.map { RuntimeRegistry.fits($0, providerId: b.provider) } ?? true
                var badges = [Badge(text: b.access.label, tone: .access(b.access))]
                if b.needsSignIn { badges.append(Badge(text: "Needs sign-in", tone: .warn)) }
                let phrase = RuntimeRegistry.accessPhrase(b.access, providerId: b.provider)
                if isChat, modelStore.isCloudLocked(b.id) { badges.append(Badge(text: "Needs credits", tone: .warn)) }
                let selected = isChat ? b.id == modelStore.selectedModelId
                    : (modelRef.map { "\($0.provider)/\($0.model)" == b.id } ?? false)
                return Choice(id: b.id, name: b.name,
                              detail: ok ? phrase.prefix(1).uppercased() + phrase.dropFirst() : "Not with \(runtimeName)",
                              mark: .provider(b.provider), badges: badges, ok: ok, selected: selected)
            }, idPrefix: isChat ? "chatBrain-" : nil) { id in
                if let b = brains.first(where: { $0.id == id }) { isChat ? pickChatBrain(b) : pickBrain(b) }
            }
            if isChat { modelOptionsRow }
        }
    }

    /// Chat target: everything else `ModelPickerSheet` has (effort, limit
    /// rings, sign-in rows, models on this iPhone and other devices).
    private var modelOptionsRow: some View {
        Button {
            Haptics.selection()
            isModelOptionsPresented = true
        } label: {
            HStack(spacing: 12) {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 28, height: 28)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
                    .accessibilityHidden(true)
                Text("Model options, effort and limits")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 6)
                Image(systemName: "chevron.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextTertiary"))
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 52)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens every model with its effort and limits")
        .accessibilityIdentifier("runtimeBrainModelOptions")
    }

    @ViewBuilder
    private var botContent: some View {
        if hubStore.isLoading && hubStore.agents.isEmpty {
            skeleton
        } else if bots.isEmpty {
            emptyLine(query.isEmpty ? "No other bots yet." : "No bots match “\(query)”.")
        } else {
            let currentId: String? = { if case .agent(let a) = target { return a.id } else { return nil } }()
            choices(bots.map { a in
                let vendor = Self.connectedVendor(a)
                return Choice(id: a.id, name: a.botDisplayName,
                              detail: vendor.map { "Run by \($0)" } ?? (a.botProfile?.tagline.flatMap { $0.isEmpty ? nil : $0 } ?? a.description ?? "Allternit bot"),
                              mark: .bot(a), badges: [Badge(text: vendor == nil ? "Allternit" : "Connected", tone: .plain)],
                              ok: true, selected: a.id == currentId)
            }) { id in
                if let a = hubStore.agents.first(where: { $0.id == id }) { switchBot(a) }
            }
        }
    }

    // MARK: Picks

    private func pickRuntime(_ r: RuntimeEntry) {
        var next = brain
        next["runtime"] = .string(r.id)
        next = RuntimeRegistry.resolveRuntime(next, in: registry.runtimes)
        next.removeValue(forKey: "autoSwitchedFrom")
        var message: String?
        if let ref = modelRef, !RuntimeRegistry.fits(r, providerId: ref.provider) {
            next["autoSwitchedFrom"] = .string("\(ref.provider)/\(ref.model)")
            if let fb = brains.first(where: { RuntimeRegistry.fits(r, providerId: $0.provider) }) {
                next["modelRef"] = .object(["providerID": .string(fb.provider), "modelID": .string(fb.model)])
                next["access"] = .string(fb.access.rawValue)
                let provider: String = { if case .list(let ids) = r.brains, let first = ids.first { return first } else { return fb.provider } }()
                message = RuntimeRegistry.switchNote(runtime: r, provider: provider, brainName: fb.name)
            } else {
                next.removeValue(forKey: "modelRef")
                next.removeValue(forKey: "access")
                message = "\(r.name) can't run \(brainName), so it will use its own default model."
            }
        }
        Haptics.selection()
        brain = next
        if look == nil { look = r.defaultLook }
        note = message
        dirty = true
    }

    /// Chat target: the old runtime sheet's path. A runtime that can't run yet
    /// shows its fix; in a chat already under way a pick opens a fresh chat on
    /// it (`ChatRuntimeStore.start`), otherwise the Brain layer opens next.
    private func pickChatRuntime(_ r: RuntimeEntry) {
        Haptics.selection()
        let availability = chatRuntime.availability(r, installed: registry.installed)
        guard availability == .ready else {
            focusedRuntime = r.id
            return
        }
        let selected = r.id == chatRuntime.runtimeId
        let fresh = chatHasSession && !selected
        chatRuntime.start(r, chatHasSession: fresh)
        focusedRuntime = nil
        if fresh {
            dismiss()
            return
        }
        // The brain stays the person's pick; say so when it can't run here.
        if let ref = modelRef, !RuntimeRegistry.fits(r, providerId: ref.provider) {
            note = "\(r.name) can't run \(brainName). Pick a brain that fits."
        } else {
            note = nil
        }
        openLayer = .brain
    }

    /// Chat target: the model sheet's path (a locked Cloud row opens the
    /// credits sheet and keeps the old selection).
    private func pickChatBrain(_ b: BrainChoice) {
        Haptics.selection()
        if modelStore.isCloudLocked(b.id) {
            creditsRequest = CloudCreditsGate.request(modelId: b.id, reason: AccountPlanStore.shared.cloudModels?.reason)
            return
        }
        modelStore.userSelectedModel(b.id)
        note = nil
    }

    private func pickBrain(_ b: BrainChoice) {
        var next = brain
        next.removeValue(forKey: "autoSwitchedFrom")
        next["modelRef"] = .object(["providerID": .string(b.provider), "modelID": .string(b.model)])
        next["access"] = .string(b.access.rawValue)
        Haptics.selection()
        brain = RuntimeRegistry.resolveRuntime(next, in: registry.runtimes)
        note = nil
        dirty = true
    }

    /// Bot Home: the Bot layer moves the picker to another bot.
    private func switchBot(_ a: AgentRecord) {
        if case .agent(let current) = target, current.id == a.id { return }
        Haptics.selection()
        if let onPickBot {
            dismiss()
            onPickBot(a)
            return
        }
        let start = Self.startState(a)
        target = .agent(a)
        brain = start.brain
        view = start.view
        look = start.look
        note = nil
        dirty = false
        openLayer = .runtime
    }

    private func save() {
        let finalBrain = RuntimeRegistry.resolveRuntime(brain, in: registry.runtimes)
        let resolvedLook = look ?? runtime?.defaultLook ?? "terminal"
        switch target {
        case .form(let form):
            form.applyLayers(brain: finalBrain, view: view, look: resolvedLook)
            dismiss()
        case .chat:
            dismiss()
        case .agent(let agent):
            isSaving = true
            saveError = nil
            var config = agent.config ?? [:]
            config["botBrain"] = .object(finalBrain)
            config["agentView"] = .object(["view": .string(view), "look": .string(resolvedLook)])
            let ref = finalBrain["modelRef"]?.jObject
            Task {
                do {
                    try await hubStore.updateAgent(id: agent.id, model: ref?["modelID"]?.jString,
                                                   provider: ref?["providerID"]?.jString, config: config)
                    agentModeStore.fetchAgentsIfNeeded(force: true)
                    dismiss()
                } catch {
                    saveError = "Couldn't save. \(FriendlyErrorMessage.from(error.localizedDescription))"
                }
                isSaving = false
            }
        }
    }

    // MARK: Choices (List · Grid · Deck)

    private enum Mark {
        case provider(String)
        case bot(AgentRecord)
    }

    private struct Badge: Hashable {
        enum Tone: Hashable { case plain, warn, live, access(BrainAccess) }
        let text: String
        let tone: Tone
    }

    private struct Choice: Identifiable {
        let id: String
        let name: String
        let detail: String
        let mark: Mark
        let badges: [Badge]
        let ok: Bool
        let selected: Bool
    }

    @ViewBuilder
    private func choices(_ items: [Choice], idPrefix: String? = nil, onPick: @escaping (String) -> Void) -> some View {
        switch layout {
        case .list:
            VStack(spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    if index > 0 { Divider().padding(.leading, 52) }
                    choiceButton(item, idPrefix: idPrefix, onPick: onPick) { listRow(item) }
                }
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle")))
        case .grid:
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                ForEach(items) { item in
                    choiceButton(item, idPrefix: idPrefix, onPick: onPick) { card(item, deck: false) }
                }
            }
        case .deck:
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 10) {
                    ForEach(items) { item in
                        choiceButton(item, idPrefix: idPrefix, onPick: onPick) { card(item, deck: true).frame(width: 172) }
                    }
                }
                .padding(.vertical, 2)
            }
        }
    }

    private func choiceButton<Content: View>(_ item: Choice, idPrefix: String?, onPick: @escaping (String) -> Void,
                                             @ViewBuilder content: () -> Content) -> some View {
        Button { onPick(item.id) } label: { content() }
            .buttonStyle(.plain)
            .disabled(!item.ok)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(([item.name, item.detail] + item.badges.map(\.text)).joined(separator: ", "))
            .accessibilityAddTraits(item.selected ? [.isButton, .isSelected] : .isButton)
            .accessibilityIdentifier(idPrefix.map { $0 + item.id } ?? "")
    }

    @ViewBuilder
    private func markView(_ mark: Mark, size: CGFloat) -> some View {
        switch mark {
        case .provider(let id): ModelProviderMark(providerId: id, size: size)
        case .bot(let a): BotAvatarView(bot: a, size: size)
        }
    }

    private func listRow(_ item: Choice) -> some View {
        HStack(spacing: 12) {
            markView(item.mark, size: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.name).font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                Text(item.detail).font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
            }
            Spacer(minLength: 6)
            if let first = item.badges.first { badge(first) }
            if item.selected {
                Image(systemName: "checkmark").font(.system(size: 13, weight: .bold)).foregroundColor(Color("TextPrimary"))
            }
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
        .opacity(item.ok ? 1 : 0.45)
    }

    private func card(_ item: Choice, deck: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top) {
                markView(item.mark, size: deck ? 40 : 28)
                Spacer()
                if item.selected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 18))
                        .foregroundColor(Color("TextPrimary"))
                }
            }
            Text(item.name)
                .font(.system(size: deck ? 16 : 14, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Text(item.detail)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            ProjectChipWrap(spacing: 4) {
                ForEach(item.badges, id: \.self) { badge($0) }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: deck ? 210 : 132, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .stroke(item.selected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: item.selected ? 1.5 : 1))
        .contentShape(Rectangle())
        .opacity(item.ok ? 1 : 0.45)
    }

    private func badge(_ b: Badge) -> some View {
        let tint: Color = {
            switch b.tone {
            case .plain: return Color("TextSecondary")
            case .warn: return Theme.statusWarning
            case .live: return Theme.statusSuccess
            case .access(let a):
                switch a {
                case .plan: return Theme.statusSuccess
                case .key: return Theme.statusWarning
                case .local: return Color("AccentPrimary")
                case .credits: return Color("TextPrimary")
                }
            }
        }()
        return Text(b.text)
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(tint)
            .lineLimit(1)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(Capsule().fill(tint.opacity(0.12)))
    }

    // MARK: States

    private var skeleton: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
            ForEach(0..<4, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(Color("FillNeutral"))
                    .frame(height: layout == .list ? 56 : 132)
            }
        }
        .accessibilityElement()
        .accessibilityLabel("Loading")
    }

    private func issueNotice(_ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.circle").font(.system(size: 13)).accessibilityHidden(true)
            Text(text).font(.system(size: 12)).fixedSize(horizontal: false, vertical: true)
            Button("Try again") { registry.refreshIfNeeded(force: true) }
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44)
        }
        .foregroundColor(Color("TextSecondary"))
        .accessibilityElement(children: .contain)
    }

    private func emptyLine(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(Color("TextSecondary"))
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
    }

    private func noteLine(_ text: String, icon: String, tone: Color = Color("TextSecondary")) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: icon).font(.system(size: 13)).accessibilityHidden(true)
            Text(text).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
        }
        .foregroundColor(tone)
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .combine)
    }

    // MARK: Helpers

    /// A Connected bot (plan D4: a vendor binding exists): its vendor's name.
    /// The server's `botKind` (platform #2776) decides when present; older
    /// servers leave it out and `config.botProfile.vendorPackId` decides.
    static func connectedVendor(_ config: [String: JSONValue]?, botKind: String? = nil) -> String? {
        if botKind == "allternit" { return nil }
        let profile = config?["botProfile"]?.jObject
        let pack = profile?["vendorPackId"]?.jString ?? ""
        if botKind != "connected" && pack.isEmpty { return nil }
        if let name = profile?["vendorName"]?.jString, !name.isEmpty { return name }
        guard !pack.isEmpty else { return "its vendor" }
        let first = pack.split(separator: "-").first.map(String.init) ?? pack
        return first.prefix(1).uppercased() + first.dropFirst()
    }

    /// `connectedVendor` for a bot record.
    static func connectedVendor(_ agent: AgentRecord) -> String? {
        connectedVendor(agent.config, botKind: agent.botKind)
    }
}
