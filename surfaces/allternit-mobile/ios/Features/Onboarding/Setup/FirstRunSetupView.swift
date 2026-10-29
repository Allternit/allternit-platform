import SwiftUI
import UIKit

/// What the real flows set while setup runs (web WizardData, phone fields).
struct SetupWizardData: Equatable {
    var theme: StyleChoice
    /// `local` (a computer you own) or `cloud` — recorded as the wizard's run mode.
    var infraType = "local"
    var defaultProvider: String?
    var defaultModelId: String?

    /// `provider/model`, or the provider alone (web completeFirstRun).
    var defaultModel: String? {
        guard let p = defaultProvider else { return nil }
        return defaultModelId.map { "\(p)/\($0)" } ?? p
    }
}

/// FirstRunSetup — Gizzi + A:// set up the workspace with you, on the phone
/// (allternit-ai `setup/FirstRunSetup.tsx`, stacked layout): the "Your
/// workspace" graph on top, then Gizzi + A://, the 30-block progress,
/// Gizzi's question, choice tiles, the result card for the choice, the real
/// connect panel, the stacked log of finished steps, then "Meet your bots"
/// and the connected view.
///
/// Every tag and result is driven by real state (SetupSignals and the
/// panels' reported outcomes). The haptic block ripple fires only on a choice.
struct FirstRunSetupView: View {
    var onFinish: @MainActor (_ state: SetupState, _ data: SetupWizardData, _ fix: String?) async -> Void

    @StateObject private var signals = SetupSignals()
    @ObservedObject private var appearance = AppearanceStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var state = SetupState()
    @State private var data: SetupWizardData
    @State private var pickedRuntime: String?
    @State private var subsDeclined: String?
    @State private var termsPrompt: TermsPrompt?
    /// Tile frames in the rail (for the ripple origin). A reference box: scrolling must not re-render setup.
    @State private var tileFrames = TileFrames()
    @State private var opening = false
    /// The account's main bot when the Meet-your-bots step opened (to tell a new one apart).
    @State private var mainBotBefore: String??

    private static let cell: CGFloat = 41

    private struct TermsPrompt: Identifiable {
        let brain: BrainChoice
        let terms: SetupSubscriptionStatus.Terms
        var id: String { brain.rawValue }
    }

    init(onFinish: @escaping @MainActor (_ state: SetupState, _ data: SetupWizardData, _ fix: String?) async -> Void) {
        self.onFinish = onFinish
        let current = StyleChoice(rawValue: AppearanceStore.shared.theme.rawValue) ?? .system
        _data = State(initialValue: SetupWizardData(theme: current))
    }

    var body: some View {
        Group {
            switch state.step {
            case .connected:
                SetupConnectedView(
                    facts: ConnectedInventory.SessionFacts(effective),
                    themeLabel: state.choices.style?.label ?? "\(data.theme.label) (unchanged)",
                    signals: signals,
                    onBack: { dispatch(.back) },
                    onOpen: { fix in Task { await open(fix: fix) } })
                .transition(.opacity)
            case .mainbot:
                MeetYourBotsView(onFinish: { _ in finishMeetYourBots() })
                    .task { await snapshotMainBot() }
                    .transition(.opacity)
            default:
                rail
            }
        }
        .background(SetupPalette.bg.ignoresSafeArea())
        .animation(reduceMotion ? nil : .easeOut(duration: 0.3), value: state.step)
        .onAppear { signals.start() }
        .sheet(item: $termsPrompt) { prompt in termsSheet(prompt) }
    }

    // MARK: - Derived outcomes

    private var effective: SetupState {
        var s = state
        if state.choices.where == .cloud {
            s.outcomes[.where] = cloudOutcome
        } else if state.choices.where == .mac {
            s.outcomes[.where] = computerOutcome
        }
        if state.choices.ai == .phoneApps {
            // Phone Link is set up from Allternit Desktop (a cable to the phone once).
            s.outcomes[.ai] = StepOutcome(status: .waiting, label: BrainChoice.phoneApps.label,
                                          detail: ["Set up from Allternit Desktop", "The first setup needs a cable to your phone"])
        } else {
            s.outcomes[.ai] = brainOutcome
        }
        if let style = state.choices.style {
            s.outcomes[.style] = StepOutcome(status: .set, label: style.label,
                                             detail: ["Theme set to \(style.label)", "Saved on this device · change it in Settings → Appearance"])
        }
        return s
    }

    private var computerOutcome: StepOutcome {
        if let rt = signals.runtimes.first(where: { $0.id == pickedRuntime }) {
            return StepOutcome(status: .paired, label: rt.displayName,
                               detail: ["Using \(rt.displayName) for your agents", "Status: \(rt.status)", "Switch any time in Settings → Computers"])
        }
        if !signals.runtimesSettled {
            return StepOutcome(status: .working, label: "Your computer", detail: ["Looking for your paired computers …"])
        }
        if signals.onlineRuntimes.isEmpty {
            return StepOutcome(status: .notFound, label: "Your computer",
                               detail: ["No computer is paired and online", "Sign in to Allternit Desktop with this account to pair one"])
        }
        return StepOutcome(status: .idle, label: "Your computer", detail: ["Pick a computer below"])
    }

    private var cloudOutcome: StepOutcome {
        let sig = signals.cloud
        let label = "Allternit Cloud computer"
        if sig.loading { return StepOutcome(status: .working, label: label, detail: ["Checking your plan …"]) }
        if !sig.signedIn { return StepOutcome(status: .error, label: label, detail: ["Sign in to use an Allternit Cloud computer"]) }
        let plan = "Checked your plan … \(sig.plan?.name ?? "unknown")"
        if let c = sig.computer {
            if c.isRunning {
                return StepOutcome(status: .paired, label: label,
                                   detail: [plan, "Found your cloud computer\(c.specs.map { " · \($0)" } ?? "")", "Linked to your account"])
            }
            return StepOutcome(status: .requested, label: label,
                               detail: [plan, "Your cloud computer is \(c.status)", "It shows in Settings → Computers when ready"])
        }
        if let error = sig.error { return StepOutcome(status: .error, label: label, detail: [error]) }
        guard let p = sig.plan else {
            return StepOutcome(status: .error, label: label, detail: ["Couldn't check your plan right now", "See Settings → Billing after setup"])
        }
        if p.isFree {
            return StepOutcome(status: .waiting, label: label,
                               detail: [plan, "A cloud computer comes with a paid plan", "Open Allternit takes you to Billing to pick one"])
        }
        return StepOutcome(status: .waiting, label: label, detail: [plan, "No cloud computer on this account yet", "Check Settings → Computers after setup"])
    }

    private static let pretty: [String: String] = ["qwen": "Qwen", "kimi": "Kimi", "zai": "Z.ai", "antigravity": "Antigravity",
                                                   "claude-cli": "Claude", "codex-cli": "ChatGPT"]

    private var brainOutcome: StepOutcome? {
        guard let choice = state.choices.ai else { return nil }
        let p = data.defaultProvider
        if let provider = choice.subscriptionProvider {
            let st = signals.subscriptionState(provider)
            let label = choice.label
            // Never connected unless the fabric login is actually ready AND it is the default.
            if st == .ready && p == choice.rawValue {
                return StepOutcome(status: .connected, label: label,
                                   detail: ["Signed in on your Sessions computer", "Default brain: \(label)", "Switch any time from the model picker"])
            }
            switch st {
            case .ready:
                return subsDeclined == provider
                    ? StepOutcome(status: .waiting, label: label, detail: ["Signed in on your Sessions computer", "Accept the subscription disclosure to use it"])
                    : StepOutcome(status: .working, label: label, detail: ["Signed in on your Sessions computer", "Setting it as your default …"])
            case .noComputer: return StepOutcome(status: .waiting, label: label, detail: ["No Sessions computer yet", "Subscriptions run on a Sessions computer — set one up below"])
            case .offline: return StepOutcome(status: .waiting, label: label, detail: ["Your Sessions computer is offline", "Start it below to sign in"])
            case .signin: return StepOutcome(status: .waiting, label: label, detail: ["Sign-in pending on your Sessions computer", "Finish signing in below"])
            case .connect: return StepOutcome(status: .idle, label: label, detail: ["Connect your \(choice.shortName) plan below", "You sign in on the vendor’s own page"])
            case .error: return StepOutcome(status: .error, label: label, detail: [signals.subsError ?? "Could not reach your Sessions computer"])
            case .checking: return StepOutcome(status: .working, label: label, detail: ["Checking your subscriptions …"])
            }
        }
        if let p, BrainChoice.forProvider(p) == choice {
            let label: String
            switch choice {
            case .allternit:
                let m = signals.catalog.first { $0.id.replacingOccurrences(of: "allternit/", with: "") == data.defaultModelId }
                label = "Allternit Cloud · \(m?.name ?? data.defaultModelId ?? "default")"
            case .other: label = Self.pretty[p] ?? p
            default: label = choice.label
            }
            let model = choice != .allternit ? data.defaultModelId.map { ["Model: \($0)"] } ?? [] : []
            return StepOutcome(status: .connected, label: label, detail: ["Default brain: \(label)"] + model + ["Switch any time from the model picker"])
        }
        if choice == .allternit {
            if signals.brainsLoading { return StepOutcome(status: .working, label: "Allternit Cloud", detail: ["Checking Allternit Cloud models …"]) }
            if signals.catalog.isEmpty {
                return StepOutcome(status: .error, label: "Allternit Cloud",
                                   detail: ["Couldn't reach Allternit Cloud models", "Pick another brain, or try again from the model picker"])
            }
        }
        return nil
    }

    // MARK: - Tiles

    private struct Tile: Identifiable {
        let id: String
        var group: String?
        var mono: String
        var icon: String?
        var swatch: StyleChoice?
        let name: String
        let tag: String
        var tagOk = false
    }

    private var whereTiles: [Tile] {
        let online = signals.onlineRuntimes.count
        let cloud = signals.cloud
        let cloudTag = cloud.loading ? "Checking…"
            : cloud.computer.map { $0.specs ?? $0.status }
            ?? (!cloud.signedIn ? "Sign in first" : cloud.plan == nil ? "Unavailable" : cloud.plan!.isFree ? "Paid plans" : "Not set up yet")
        return [
            Tile(id: WhereChoice.mac.rawValue, mono: "C", name: "Your computer",
                 tag: !signals.runtimesSettled ? "Checking…" : online > 0 ? "\(online) online" : "None online", tagOk: online > 0),
            Tile(id: WhereChoice.cloud.rawValue, mono: "A", name: "Allternit Cloud computer", tag: cloudTag,
                 tagOk: cloud.computer?.isRunning == true),
            Tile(id: WhereChoice.server.rawValue, mono: "S", name: "Your own server", tag: "Pair code"),
        ]
    }

    private var styleTiles: [Tile] {
        StyleChoice.allCases.map { Tile(id: $0.rawValue, mono: "", swatch: $0, name: $0.label, tag: "") }
    }

    private var aiTiles: [Tile] {
        func subsTag(_ s: SetupSignals.SubscriptionTileState) -> String {
            switch s {
            case .checking: return "Checking…"
            case .ready: return "Connected"
            case .signin: return "Sign in"
            case .noComputer: return "Needs Sessions computer"
            case .offline: return "Computer offline"
            case .connect: return "Connect"
            case .error: return "Unavailable"
            }
        }
        let authed = signals.authed
        func brain(_ ids: [String]) -> (String, Bool) { ids.contains(where: authed.contains) ? ("Connected", true) : ("Connect", false) }
        var tiles: [Tile] = BrainChoice.subscriptions.enumerated().map { i, b in
            let st = signals.subscriptionState(b.subscriptionProvider ?? "")
            return Tile(id: b.rawValue, group: i == 0 ? "Use your subscription" : nil, mono: String(b.shortName.prefix(1)),
                        name: b.shortName, tag: subsTag(st), tagOk: st == .ready)
        }
        tiles.append(Tile(id: BrainChoice.phoneApps.rawValue, group: "Use the apps on your devices", mono: "",
                          icon: "iphone", name: "Apps on your phone", tag: "From Desktop"))
        tiles.append(Tile(id: BrainChoice.allternit.rawValue, group: "Or connect another way", mono: "A", name: "Allternit Cloud",
                          tag: signals.brainsLoading ? "Checking…" : signals.catalog.isEmpty ? "Unavailable" : "\(signals.catalog.count) models",
                          tagOk: !signals.catalog.isEmpty))
        let claude = brain(["anthropic", "claude-cli"]), gpt = brain(["openai", "codex-cli"]), gem = brain(["google", "antigravity"])
        tiles.append(Tile(id: BrainChoice.claude.rawValue, mono: "C", name: "Claude", tag: claude.0, tagOk: claude.1))
        tiles.append(Tile(id: BrainChoice.chatgpt.rawValue, mono: "G", name: "ChatGPT", tag: gpt.0, tagOk: gpt.1))
        tiles.append(Tile(id: BrainChoice.gemini.rawValue, mono: "G", name: "Gemini", tag: gem.0, tagOk: gem.1))
        tiles.append(Tile(id: BrainChoice.local.rawValue, mono: "L", name: "Local model", tag: "From Desktop"))
        tiles.append(Tile(id: BrainChoice.other.rawValue, mono: "+", name: "More brains", tag: "From Desktop"))
        return tiles
    }

    // MARK: - Rail

    private var rail: some View {
        GeometryReader { geo in
            let eff = effective
            let field = state.step.field
            let busy = field.map { eff.outcomes[$0]?.status == .working } ?? false
            let orb = orbState(for: state.step, busy: busy)
            VStack(spacing: 0) {
                ZStack {
                    SetupBlockField()
                    SetupGraphView(state: eff, orb: orb, showBrains: state.step == .ai)
                }
                .frame(height: min(260, max(170, geo.size.height * 0.30)))
                .background(SetupPalette.bg)
                .clipShape(RoundedRectangle(cornerRadius: 16))
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(SetupPalette.line))

                duo(orb).padding(.top, 12)
                SetupProgressBlocks(step: state.step, order: state.order).padding(.top, 8)

                ScrollViewReader { proxy in
                    ScrollView {
                        content(eff, field: field, busy: busy)
                            .padding(.top, 4)
                            .padding(.bottom, 8)
                    }
                    .scrollIndicators(.hidden)
                    .scrollDismissesKeyboard(.interactively)
                    .onChange(of: state.ripple) { _, _ in
                        // After a choice, bring its real connect panel into view.
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) {
                            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.3)) { proxy.scrollTo("panel", anchor: .bottom) }
                        }
                    }
                    .onChange(of: state.step) { _, _ in proxy.scrollTo("top", anchor: .top) }
                }

                nav
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 14)
            .coordinateSpace(.named("setup-rail"))
            .overlay {
                SetupHapticRipple(rippleKey: state.ripple, origin: state.rippleOrigin, cell: Self.cell)
            }
        }
    }

    private func duo(_ orb: AllternitOrbState) -> some View {
        let emotion: SetupGizzi.Emotion = state.step == .where ? .curious : state.step == .style ? .pleased : .focused
        return HStack(spacing: 8) {
            SetupGizzi(size: 30, emotion: emotion)
            AllternitOrbView(state: orb, size: 20, label: "A:// is \(orb.rawValue)")
            Text("Gizzi + A://").font(.system(size: 13.5, weight: .bold)).foregroundStyle(SetupPalette.ink).lineLimit(1).fixedSize()
            HStack(spacing: 6) {
                SetupBlip()
                Text(orb.rawValue).font(SetupPalette.mono(10.5)).tracking(0.4).foregroundStyle(SetupPalette.mut).lineLimit(1)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("A:// is \(orb.rawValue)")
            Spacer(minLength: 0)
        }
    }

    @ViewBuilder
    private func content(_ eff: SetupState, field: SetupField?, busy: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Color.clear.frame(height: 0).id("top")
            let done = completedCards(eff)
            if !done.isEmpty {
                VStack(spacing: 6) { ForEach(done, id: \.field) { SetupDoneRow(card: $0) } }
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel("Done so far")
            }
            if let field {
                Text(SetupCopy.question(field))
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(SetupPalette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 10)
                    .padding(.bottom, 8)
                    .id("q-\(field.rawValue)")
                    .setupRise()
                tileList(field)
                if let card = resultCard(eff, field) {
                    SetupResultCardView(card: card, working: busy)
                        .id("card-\(field.rawValue)-\(state.choices.raw(field) ?? "")")
                        .padding(.top, 4)
                }
                panel(field).id("panel")
            }
        }
    }

    private func tileList(_ field: SetupField) -> some View {
        let tiles: [Tile] = field == .where ? whereTiles : field == .style ? styleTiles : aiTiles
        let current = state.choices.raw(field)
        return VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(tiles.enumerated()), id: \.element.id) { i, t in
                if let group = t.group {
                    Text(group.uppercased())
                        .font(SetupPalette.mono(10.5, .semibold)).tracking(0.8)
                        .foregroundStyle(SetupPalette.mut)
                        .padding(.leading, 2)
                        .padding(.top, i == 0 ? 0 : 6)
                }
                tileButton(t, on: current == t.id, field: field, index: i)
            }
        }
        .id("tiles-\(field.rawValue)")
        .accessibilityElement(children: .contain)
        .accessibilityLabel(field.title)
    }

    private func tileButton(_ t: Tile, on: Bool, field: SetupField, index: Int) -> some View {
        Button { choose(field, t.id) } label: {
            HStack(spacing: 12) {
                Group {
                    if let swatch = t.swatch {
                        SetupSwatch(style: swatch)
                    } else if let icon = t.icon {
                        Image(systemName: icon).font(.system(size: 14, weight: .semibold))
                    } else {
                        Text(t.mono).font(SetupPalette.mono(11, .bold))
                    }
                }
                .foregroundStyle(on ? SetupPalette.bg : SetupPalette.sub)
                .frame(width: 26, height: 26)
                .background(t.swatch == nil ? (on ? SetupPalette.ink : SetupPalette.fill) : .clear, in: RoundedRectangle(cornerRadius: 8))
                Text(t.name).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(SetupPalette.ink).lineLimit(1)
                Spacer(minLength: 4)
                if !t.tag.isEmpty {
                    Text(t.tag)
                        .font(SetupPalette.mono(10))
                        .foregroundStyle(on ? SetupPalette.coral : t.tagOk ? SetupPalette.ok : SetupPalette.mut)
                        .lineLimit(1)
                }
            }
            .padding(10)
            .frame(minHeight: 44)
            .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(on ? SetupPalette.ink : SetupPalette.line, lineWidth: 1))
            .background(RoundedRectangle(cornerRadius: 14).stroke(SetupPalette.coralHalo, lineWidth: on ? 3 : 0).padding(-2))
            .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(SetupTilePressStyle())
        .accessibilityLabel(t.tag.isEmpty ? t.name : "\(t.name), \(t.tag)")
        .accessibilityAddTraits(on ? [.isSelected] : [])
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .named("setup-rail")) } action: { tileFrames.byId[t.id] = $0 }
        .setupRise(6, delay: Double(index) * 0.03)
        .animation(.easeOut(duration: 0.2), value: on)
    }

    // MARK: - Panels

    @ViewBuilder
    private func panel(_ field: SetupField) -> some View {
        switch (field, state.choices.where, state.choices.ai) {
        case (.where, .mac?, _) where signals.onlineRuntimes.count > 1:
            SetupPanel(title: "Your computers") {
                VStack(spacing: 8) {
                    ForEach(signals.onlineRuntimes, id: \.id) { r in
                        Button { pickRuntime(r.id) } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(r.displayName).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                                    if let host = r.hostname { Text(host).font(.system(size: 12)).foregroundStyle(SetupPalette.sub) }
                                }
                                Spacer()
                                Text(r.status).font(SetupPalette.mono(10)).foregroundStyle(SetupPalette.ok)
                            }
                            .padding(10)
                            .frame(minHeight: 44)
                            .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
                            .overlay(RoundedRectangle(cornerRadius: 12).stroke(pickedRuntime == r.id ? SetupPalette.ink : SetupPalette.line))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(pickedRuntime == r.id ? [.isSelected] : [])
                    }
                }
            }
        case (.where, .server?, _):
            SetupPanel(title: "Your own server") {
                SetupServerPairPanel { outcome in dispatch(.outcome(.where, outcome)) }
            }
        case (.ai, _, let brain?) where brain.isSubscription:
            subscriptionPanel(brain)
        case (.ai, _, .phoneApps?):
            SetupPanel(title: "Apps on your phone") {
                SetupNote(text: "Phone Link uses the AI apps you’re signed in to on this phone. Set it up from Allternit Desktop on your computer: it needs a cable to your phone once, then works over Wi-Fi.")
            }
        case (.ai, _, .allternit?) where signals.catalog.count > 1:
            let selected = BrainChoice.forProvider(data.defaultProvider) == .allternit ? data.defaultModelId : nil
            SetupPanel(title: "Default model") {
                VStack(spacing: 6) {
                    ForEach(signals.catalog.prefix(8)) { m in
                        let id = m.id.replacingOccurrences(of: "allternit/", with: "")
                        Button {
                            data.defaultProvider = "allternit"
                            data.defaultModelId = id
                        } label: {
                            HStack {
                                Text(m.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(SetupPalette.ink).lineLimit(1)
                                Spacer()
                                Text(id).font(SetupPalette.mono(10)).foregroundStyle(selected == id ? SetupPalette.coral : SetupPalette.mut).lineLimit(1)
                            }
                            .padding(.horizontal, 10)
                            .frame(minHeight: 40)
                            .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 10))
                            .overlay(RoundedRectangle(cornerRadius: 10).stroke(selected == id ? SetupPalette.ink : SetupPalette.line))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(selected == id ? [.isSelected] : [])
                    }
                }
            }
        case (.ai, _, let brain?) where brain != .allternit:
            SetupPanel(title: "Connect \(brain == .other ? "another brain" : brain.label)") {
                SetupBrainConnect(brain: brain, signals: signals) { provider, model in
                    data.defaultProvider = provider
                    data.defaultModelId = model
                }
            }
        default:
            EmptyView()
        }
    }

    @ViewBuilder
    private func subscriptionPanel(_ brain: BrainChoice) -> some View {
        let provider = brain.subscriptionProvider ?? ""
        switch signals.subscriptionState(provider) {
        case .checking:
            EmptyView()
        case .ready:
            if data.defaultProvider != brain.rawValue {
                SetupPanel(title: brain.label) {
                    SetupNote(text: "Signed in on your Sessions computer.")
                    Button("Use it as my default brain") { makeSubscriptionDefault(brain) }
                        .buttonStyle(SetupInlineButtonStyle())
                }
            }
        default:
            SetupPanel(title: "Connect \(brain.shortName) subscription") {
                SetupSubscriptionConnect(brain: brain, signals: signals,
                                         onReady: { makeSubscriptionDefault(brain) },
                                         onCancel: { dispatch(.setAIQuietly(.allternit)) })
            }
        }
    }

    private func termsSheet(_ prompt: TermsPrompt) -> some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text(prompt.terms.text).font(.system(size: 14)).foregroundStyle(SetupPalette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(20)
            }
            .navigationTitle("About \(prompt.brain.shortName)’s terms")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        subsDeclined = prompt.brain.subscriptionProvider
                        termsPrompt = nil
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("I agree") {
                        let brain = prompt.brain
                        termsPrompt = nil
                        Task {
                            do {
                                if let version = signals.subsStatus?.disclosureVersion {
                                    try await SetupAPI.acknowledgeDisclosure(provider: brain.subscriptionProvider ?? "", version: version)
                                }
                                await signals.loadSubscriptions()
                                data.defaultProvider = brain.rawValue
                                data.defaultModelId = nil
                            } catch {
                                subsDeclined = brain.subscriptionProvider
                            }
                        }
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    // MARK: - Nav

    private var nav: some View {
        HStack(spacing: 8) {
            Button("Back") { dispatch(.back) }
                .buttonStyle(SetupGhostButtonStyle())
                .disabled(state.step == .where)
            Button("Skip") { dispatch(.skip) }
                .buttonStyle(SetupGhostButtonStyle())
            Button(state.nextIsConnected ? "See it all" : "Next") { dispatch(.next) }
                .buttonStyle(SetupPrimaryButtonStyle())
                .disabled(!state.canAdvance)
        }
        .padding(.top, 10)
    }

    // MARK: - Actions

    private func dispatch(_ action: SetupAction) {
        if action.firesHaptic { UIImpactFeedbackGenerator(style: .light).impactOccurred() }
        let next = SetupReducer.reduce(state, action)
        if reduceMotion { state = next } else { withAnimation(.easeOut(duration: 0.25)) { state = next } }
    }

    /// The only place the haptic ripple fires.
    private func choose(_ field: SetupField, _ value: String) {
        let origin = tileFrames.byId[value].map { f in
            RippleOrigin(col: max(0, Int(floor((f.minX + min(f.width / 2, 40)) / Self.cell))),
                         row: max(0, Int(floor(f.midY / Self.cell))))
        }
        switch field {
        case .where:
            guard let v = WhereChoice(rawValue: value) else { return }
            data.infraType = v == .cloud ? "cloud" : "local"
            if v == .mac {
                let online = signals.onlineRuntimes
                if online.count == 1 { pickRuntime(online[0].id) }
                else if pickedRuntime == nil, let active = RuntimeRelayStore.shared.selectedRuntimeId,
                        signals.runtimes.contains(where: { $0.id == active }) {
                    // Keep an already-pinned computer selected.
                    pickedRuntime = active
                }
            }
            dispatch(.chooseWhere(v, origin))
        case .style:
            guard let v = StyleChoice(rawValue: value) else { return }
            applyTheme(v)
            dispatch(.chooseStyle(v, origin))
        case .ai:
            guard let v = BrainChoice(rawValue: value) else { return }
            if let provider = v.subscriptionProvider, signals.subscriptionState(provider) == .ready { makeSubscriptionDefault(v) }
            if v == .allternit, let first = signals.catalog.first, BrainChoice.forProvider(data.defaultProvider) != .allternit {
                data.defaultProvider = "allternit"
                data.defaultModelId = first.id.replacingOccurrences(of: "allternit/", with: "")
            }
            dispatch(.chooseAI(v, origin))
        }
    }

    private func applyTheme(_ style: StyleChoice) {
        if let mode = AppearanceStore.Mode(rawValue: style.rawValue) { appearance.theme = mode }
        data.theme = style
    }

    /// The computer picked here is the one the phone relays to.
    private func pickRuntime(_ id: String) {
        pickedRuntime = id
        RuntimeRelayStore.shared.select(id)
    }

    /// A ready subscription becomes the default brain only after its
    /// provider-terms disclosure (D16) is accepted — same gate as the model picker.
    private func makeSubscriptionDefault(_ brain: BrainChoice) {
        subsDeclined = nil
        guard let provider = brain.subscriptionProvider else { return }
        if let terms = signals.terms(for: provider), !terms.acknowledged {
            termsPrompt = TermsPrompt(brain: brain, terms: terms)
            return
        }
        data.defaultProvider = brain.rawValue
        data.defaultModelId = nil
    }

    private func snapshotMainBot() async {
        guard mainBotBefore == nil else { return }
        let roster = try? await MeetAgentsAPI.roster()
        mainBotBefore = .some(roster.flatMap { $0.first(where: \.isMain)?.id })
    }

    /// "Meet your bots" Done or Skip: seen on this phone and the account, then on to Connected.
    private func finishMeetYourBots() {
        BotsIntroStore.shared.markSeen()
        dispatch(.next)
        Task {
            // The main bot counts as set up now only when it changed in that step.
            guard let main = (try? await MeetAgentsAPI.roster())?.first(where: \.isMain)?.id else { return }
            if case .some(let before) = mainBotBefore, before != main { dispatch(.mainBot(main)) }
        }
    }

    private func open(fix: String?) async {
        guard !opening else { return }
        opening = true
        // Cloud computer picked on the free plan: Billing opens once the workspace is up.
        let cloud = signals.cloud
        let goBilling = state.choices.where == .cloud && cloud.signedIn && cloud.computer == nil && cloud.plan?.isFree == true
        await onFinish(effective, data, fix ?? (goBilling ? "billing" : nil))
        opening = false
    }
}

private final class TileFrames {
    var byId: [String: CGRect] = [:]
}

/// Tile press feedback (`.as-tile:active { scale(.99) }`).
private struct SetupTilePressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.99 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

/// The Light / Dark / System swatch on a style tile.
private struct SetupSwatch: View {
    let style: StyleChoice

    var body: some View {
        ZStack {
            switch style {
            case .light: Color.white
            case .dark: Color(hex: "#141413")
            case .system:
                GeometryReader { g in
                    Path { p in p.move(to: .zero); p.addLine(to: CGPoint(x: g.size.width, y: 0)); p.addLine(to: CGPoint(x: 0, y: g.size.height)) }
                        .fill(Color.white)
                        .background(Color(hex: "#141413"))
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color(hex: style == .dark ? "#262626" : "#E4E4E7")))
    }
}

/// The blinking coral square next to A://'s state.
private struct SetupBlip: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        RoundedRectangle(cornerRadius: 2)
            .fill(SetupPalette.coral)
            .frame(width: 6, height: 6)
            .opacity(dim ? 0.35 : 1)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.6).repeatForever(autoreverses: true)) { dim = true }
            }
    }
}
