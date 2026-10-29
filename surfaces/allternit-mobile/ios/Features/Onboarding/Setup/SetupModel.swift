import Foundation

/// First-run setup — the pure model behind the Gizzi + A:// setup, ported
/// from allternit-ai `src/components/onboarding/setup/setupModel.ts` (the
/// m.allternit.com phone flow is the spec).
///
/// Kept free of SwiftUI and network so the step machine, the result cards
/// and the progress blocks stay testable. Every card line is built from what
/// ACTUALLY happened (an outcome reported by the real pair/connect/theme
/// calls), never from the choice alone: picking "Your computer" with no
/// computer online does not print "Paired".

// MARK: - Steps

enum SetupStep: String, CaseIterable, Sendable {
    case `where`, style, ai, mainbot, connected

    var label: String {
        switch self {
        case .where: return "Where it runs"
        case .style: return "Style"
        case .ai: return "Your AI"
        case .mainbot: return "Main bot"
        case .connected: return "Connected"
        }
    }

    /// The choice field a step asks for (connected and the main-bot slot ask nothing).
    var field: SetupField? {
        switch self {
        case .where: return .where
        case .style: return .style
        case .ai: return .ai
        case .mainbot, .connected: return nil
        }
    }
}

/// The main-bot step ("Meet your bots") runs between Your AI and Connected,
/// like the web (MAIN_BOT_STEP_ENABLED, Eoj 2026-10-16).
let setupMainBotStepEnabled = true

func setupStepOrder(mainBot: Bool = setupMainBotStepEnabled) -> [SetupStep] {
    mainBot ? [.where, .style, .ai, .mainbot, .connected] : [.where, .style, .ai, .connected]
}

enum SetupField: String, CaseIterable, Sendable {
    case `where`, style, ai

    var title: String {
        switch self {
        case .where: return "Where it runs"
        case .style: return "Style"
        case .ai: return "Your AI"
        }
    }
}

enum WhereChoice: String, CaseIterable, Sendable {
    case mac, cloud, server

    /// Phone names (the phone never says "This Mac").
    var label: String {
        switch self {
        case .mac: return "Your computer"
        case .cloud: return "Allternit Cloud computer"
        case .server: return "Your own server"
        }
    }

    /// Node names in the "Your workspace" graph.
    var graphName: String {
        switch self {
        case .mac: return "Your computer"
        case .cloud: return "Cloud computer"
        case .server: return "Your own server"
        }
    }
}

enum StyleChoice: String, CaseIterable, Sendable {
    case light, dark, system

    var label: String {
        switch self {
        case .light: return "Light"
        case .dark: return "Dark"
        case .system: return "System"
        }
    }
}

/// `subs-*` = Subscription Fabric brains on the user's Sessions computer;
/// `phone-apps` = Phone Link (set up from Desktop).
enum BrainChoice: String, CaseIterable, Sendable {
    case subsChatgpt = "subs-chatgpt"
    case subsClaude = "subs-claude"
    case subsKimi = "subs-kimi"
    case phoneApps = "phone-apps"
    case allternit, claude, chatgpt, gemini, local, other

    static let subscriptions: [BrainChoice] = [.subsChatgpt, .subsClaude, .subsKimi]

    var isSubscription: Bool { rawValue.hasPrefix("subs-") }

    /// `subs-chatgpt` → `chatgpt` (the fabric provider id).
    var subscriptionProvider: String? {
        isSubscription ? String(rawValue.dropFirst("subs-".count)) : nil
    }

    var label: String {
        switch self {
        case .subsChatgpt: return "ChatGPT (your subscription)"
        case .subsClaude: return "Claude (your subscription)"
        case .subsKimi: return "Kimi (your subscription)"
        case .phoneApps: return "Apps on your phone"
        case .allternit: return "Allternit Cloud"
        case .claude: return "Claude"
        case .chatgpt: return "ChatGPT"
        case .gemini: return "Gemini"
        case .local: return "Local model"
        case .other: return "Other brain"
        }
    }

    /// "ChatGPT (your subscription)" → "ChatGPT".
    var shortName: String { label.replacingOccurrences(of: " (your subscription)", with: "") }

    /// Which brain row a saved provider id belongs to.
    static func forProvider(_ provider: String?) -> BrainChoice? {
        guard let provider, !provider.isEmpty else { return nil }
        let p = provider.lowercased()
        // Before the name matches below: "subs-claude" is the subscription, not the Claude CLI/API.
        if p.hasPrefix("subs-") { return BrainChoice(rawValue: p) ?? .other }
        if p == "anthropic" || p == "claude-cli" || p.hasPrefix("claude") { return .claude }
        if p == "openai" || p == "codex-cli" || p.hasPrefix("codex") || p.hasPrefix("gpt") { return .chatgpt }
        if p == "google" || p == "antigravity" || p.hasPrefix("gemini") { return .gemini }
        if ["ollama", "lmstudio", "sidecar", "local"].contains(p) { return .local }
        if p.hasPrefix("allternit") { return .allternit }
        return .other
    }
}

struct SetupChoices: Equatable, Sendable {
    var `where`: WhereChoice?
    var style: StyleChoice?
    var ai: BrainChoice?

    func isSet(_ field: SetupField) -> Bool {
        switch field {
        case .where: return `where` != nil
        case .style: return style != nil
        case .ai: return ai != nil
        }
    }

    func raw(_ field: SetupField) -> String? {
        switch field {
        case .where: return `where`?.rawValue
        case .style: return style?.rawValue
        case .ai: return ai?.rawValue
        }
    }
}

/// Status of the real action behind a choice.
enum OutcomeStatus: String, Sendable {
    case idle        // chosen, nothing attempted yet
    case working     // checking / connecting
    case detected    // found, not yet registered
    case paired      // registered / connected
    case requested   // asked for, still provisioning
    case waiting     // user still has to do something elsewhere
    case connected   // brain authenticated and set as default
    case set         // preference applied
    case notFound = "not-found"
    case error

    var isFinished: Bool { self == .paired || self == .connected || self == .set }
}

struct StepOutcome: Equatable, Sendable {
    var status: OutcomeStatus
    /// Human label of what got set up, e.g. "MacBook Pro", "Claude (your subscription)".
    var label: String?
    /// Real detail lines for the result card (no invented values).
    var detail: [String] = []
}

struct SetupState: Equatable, Sendable {
    var order: [SetupStep] = setupStepOrder()
    var step: SetupStep = .where
    var choices = SetupChoices()
    var outcomes: [SetupField: StepOutcome] = [:]
    var skipped: [SetupField: Bool] = [:]
    /// Main bot made/picked in the Meet-your-bots step, when one really was.
    var mainBotId: String?
    /// Incremented on every choice — keys the haptic ripple.
    var ripple = 0
    /// Origin cell of the last ripple, in rail cells (set by the view).
    var rippleOrigin: RippleOrigin?

    func index(of step: SetupStep) -> Int { order.firstIndex(of: step) ?? -1 }

    /// Whether Next is allowed from the current step.
    var canAdvance: Bool {
        if step == .mainbot { return true }
        guard let field = step.field else { return false }
        return choices.isSet(field)
    }

    /// The step after the current one is the connected view.
    var nextIsConnected: Bool {
        let i = index(of: step)
        return i >= 0 && i + 1 < order.count && order[i + 1] == .connected
    }
}

struct RippleOrigin: Equatable, Sendable {
    var col: Int
    var row: Int
}

enum SetupAction: Sendable {
    case chooseWhere(WhereChoice, RippleOrigin?)
    case chooseStyle(StyleChoice, RippleOrigin?)
    case chooseAI(BrainChoice, RippleOrigin?)
    /// Picks a brain without a ripple (a cancelled connect falls back).
    case setAIQuietly(BrainChoice)
    case outcome(SetupField, StepOutcome)
    case mainBot(String?)
    case next, back, skip
    case goto(SetupStep)

    /// Haptic block squares fire ONLY when a setup choice is picked — never
    /// on Next, Back, Skip, connect buttons or sign-in.
    var firesHaptic: Bool {
        switch self {
        case .chooseWhere, .chooseStyle, .chooseAI: return true
        default: return false
        }
    }
}

enum SetupReducer {
    static func reduce(_ state: SetupState, _ action: SetupAction) -> SetupState {
        var s = state
        switch action {
        case .chooseWhere(let v, let origin):
            choose(&s, .where, changed: s.choices.where != v, origin: origin)
            s.choices.where = v
        case .chooseStyle(let v, let origin):
            choose(&s, .style, changed: s.choices.style != v, origin: origin)
            s.choices.style = v
        case .chooseAI(let v, let origin):
            choose(&s, .ai, changed: s.choices.ai != v, origin: origin)
            s.choices.ai = v
        case .setAIQuietly(let v):
            if s.choices.ai != v { s.outcomes[.ai] = nil }
            s.choices.ai = v
        case .outcome(let field, let outcome):
            if s.outcomes[field] == outcome { return state }
            s.outcomes[field] = outcome
        case .mainBot(let id):
            s.mainBotId = id
        case .next:
            guard s.canAdvance else { return state }
            s.step = s.order[min(s.index(of: s.step) + 1, s.order.count - 1)]
        case .skip:
            let i = s.index(of: s.step)
            if s.step != .mainbot {
                guard let field = s.step.field else { return state }
                // Skipping keeps any real outcome that already happened (a
                // brain that did connect) but marks the step answered.
                s.skipped[field] = !s.choices.isSet(field)
            }
            s.step = s.order[min(i + 1, s.order.count - 1)]
        case .back:
            let i = s.index(of: s.step)
            if i > 0 { s.step = s.order[i - 1] }
        case .goto(let target):
            // Only completed steps (and the current one) are reachable.
            let t = s.index(of: target)
            guard t >= 0, t <= s.index(of: s.step) else { return state }
            s.step = target
        }
        return s
    }

    private static func choose(_ s: inout SetupState, _ field: SetupField, changed: Bool, origin: RippleOrigin?) {
        // A new choice discards the previous choice's outcome; the real flow
        // for the new choice reports its own.
        if changed { s.outcomes[field] = nil }
        s.skipped[field] = false
        s.ripple += 1
        s.rippleOrigin = origin
    }
}

// MARK: - Progress

let setupProgressBlocks = 30

enum ProgressBlock: Sendable { case off, on, lead }

/// Lit blocks per step (4 steps: 7 · 15 · 22 · 30).
func litBlocks(_ step: SetupStep, order: [SetupStep]) -> Int {
    guard let i = order.firstIndex(of: step) else { return 0 }
    return ((i + 1) * setupProgressBlocks) / order.count
}

func progressBlocks(_ step: SetupStep, order: [SetupStep]) -> [ProgressBlock] {
    let lit = litBlocks(step, order: order)
    return (0..<setupProgressBlocks).map { b in
        if b < lit - 1 { return .on }
        if b == lit - 1 { return lit == setupProgressBlocks ? .on : .lead }
        return .off
    }
}

/// What A:// is doing while Gizzi asks (prototype choreography).
func orbState(for step: SetupStep, busy: Bool) -> AllternitOrbState {
    if busy { return .working }
    switch step {
    case .where: return .searching
    case .style: return .writing
    case .ai: return .thinking
    case .mainbot: return .working
    case .connected: return .idle
    }
}

// MARK: - Copy

enum SetupCopy {
    static func question(_ field: SetupField) -> String {
        switch field {
        case .where: return "Hi, I'm Gizzi. First: where should I run your agents?"
        case .style: return "Got it. Light, dark, or match your device?"
        case .ai: return "Last one. Which brain should I use by default?"
        }
    }
}

// MARK: - Result cards

enum ChipTone: Sendable { case ok, warn, soft, error }

struct ResultCard: Equatable, Sendable {
    var field: SetupField
    var title: String
    /// Short value for the collapsed log row.
    var summary: String
    var chip: String
    var tone: ChipTone
    var lines: [String]
    /// Mark the last line done (✓) — only when the action really finished.
    var done: Bool
    var kvKey: String
    var kvValue: String
}

private func chip(for status: OutcomeStatus) -> (String, ChipTone) {
    switch status {
    case .idle: return ("Chosen", .soft)
    case .working: return ("Working", .warn)
    case .detected: return ("Detected", .warn)
    case .paired: return ("Paired", .ok)
    case .requested: return ("Requested", .warn)
    case .waiting: return ("Waiting", .warn)
    case .connected: return ("Connected", .ok)
    case .set: return ("Set", .ok)
    case .notFound: return ("Not found", .error)
    case .error: return ("Error", .error)
    }
}

func resultCard(_ state: SetupState, _ field: SetupField) -> ResultCard? {
    let outcome = state.outcomes[field]
    let key: String = {
        switch field {
        case .where: return "Computer"
        case .style: return "Theme"
        case .ai: return state.choices.ai == .phoneApps ? "Phone" : "Default brain"
        }
    }()

    guard state.choices.isSet(field) || outcome != nil else {
        guard state.skipped[field] == true else { return nil }
        return ResultCard(field: field, title: field.title, summary: "Skipped", chip: "Skipped", tone: .soft,
                          lines: ["Skipped — change it any time in Settings"], done: false, kvKey: key, kvValue: "—")
    }

    let name: String = outcome?.label ?? {
        switch field {
        case .where: return state.choices.where?.label ?? "—"
        case .style: return state.choices.style?.label ?? "—"
        case .ai: return state.choices.ai?.label ?? "—"
        }
    }()
    let status = outcome?.status ?? .idle
    let (chipText, tone) = chip(for: status)
    let lines = (outcome?.detail.isEmpty == false) ? outcome!.detail : defaultLines(field, name, status)
    return ResultCard(field: field, title: field.title, summary: name, chip: chipText, tone: tone,
                      lines: lines, done: status.isFinished, kvKey: key, kvValue: name)
}

private func defaultLines(_ field: SetupField, _ name: String, _ status: OutcomeStatus) -> [String] {
    switch field {
    case .style:
        return ["Theme set to \(name)", "Change it any time in Settings → Appearance"]
    case .where:
        switch status {
        case .paired: return ["Paired \(name) to your account"]
        case .working: return ["Connecting \(name) …"]
        case .detected: return ["Found \(name)", "Press Use this to pair it"]
        case .notFound: return ["Looked for Allternit on \(name)", "Not running here yet"]
        case .error: return ["Could not pair \(name)"]
        default: return ["Chose \(name)", "Finish below, or later in Settings → Computers"]
        }
    case .ai:
        switch status {
        case .connected: return ["Default brain: \(name)", "Switch any time from the model picker"]
        case .working: return ["Connecting \(name) …"]
        case .error: return ["Could not connect \(name)"]
        default: return ["Chose \(name)", "Connect it below, or later in Settings → Models"]
        }
    }
}

/// The stacked log of completed steps shown above the current question.
func completedCards(_ state: SetupState) -> [ResultCard] {
    let now = state.index(of: state.step)
    return SetupField.allCases.compactMap { f in
        guard let step = SetupStep(rawValue: f.rawValue), state.index(of: step) < now else { return nil }
        return resultCard(state, f) ?? ResultCard(field: f, title: f.title, summary: "Skipped", chip: "Skipped",
                                                  tone: .soft, lines: [], done: false, kvKey: "", kvValue: "")
    }
}

// MARK: - Haptic ripple geometry

struct RippleCell: Sendable {
    var origin: Bool
    var delay: Double
}

/// Square ring ripple: delay grows with Chebyshev distance from the origin cell.
func rippleCells(cols: Int, rows: Int, origin: RippleOrigin, step: Double = 0.045) -> [RippleCell] {
    var cells: [RippleCell] = []
    cells.reserveCapacity(cols * rows)
    for r in 0..<rows {
        for c in 0..<cols {
            let d = max(abs(c - origin.col), abs(r - origin.row))
            cells.append(RippleCell(origin: d == 0, delay: Double(d) * step))
        }
    }
    return cells
}

// MARK: - Gate

/// What the phone gate shows (allternit-ai `PhoneFirstRun.tsx` decidePhoneSetup).
enum PhoneSetupDecision: Sendable { case checking, run, skip }

enum SetupServerAnswer: Sendable { case pending, run, done, failed }

/// Pure gate decision. The server's setup config decides; when it can't
/// answer (no computer online to relay it), setup only shows for an account
/// with no computer paired at all — an existing user whose computer is just
/// asleep is never stopped by setup on their phone.
func decidePhoneSetup(
    completedLocally: Bool,
    usedAppBefore: Bool,
    server: SetupServerAnswer,
    runtimesLoaded: Bool,
    runtimeCount: Int
) -> PhoneSetupDecision {
    switch server {
    case .pending: return .checking
    case .done: return .skip
    case .run: return completedLocally ? .skip : .run
    case .failed:
        if completedLocally || usedAppBefore { return .skip }
        if !runtimesLoaded { return .checking }
        return runtimeCount == 0 ? .run : .skip
    }
}
