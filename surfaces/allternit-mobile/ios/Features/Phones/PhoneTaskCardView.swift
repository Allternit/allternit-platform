import SwiftUI
import UIKit

/// The `phone_task` / `computer_task` card on a thread (docs/api/phone-holds.mdx and the
/// live-status-pill contract): newest event per `runId` wins. Each step posts one `running` card
/// with `stepLabel`; `waiting_approval` carries "Waiting for your OK to …"; the closing card
/// carries `summary` and the `trail`. `offline` is client-side (the phone stopped answering).
struct PhoneTaskCardModel: Identifiable, Equatable, Sendable {
    enum Status: String, Sendable, CaseIterable {
        case running, waitingApproval = "waiting_approval", done, denied
        case approvalTimeout = "approval_timeout", phoneLocked = "phone_locked", failed, offline

        init(raw: String) {
            switch raw {
            case "max_steps", "timeout", "refused": self = .failed
            default: self = Status(rawValue: raw) ?? .failed
            }
        }

        var isFinal: Bool { self != .running && self != .waitingApproval }
    }

    enum Kind: String, Sendable { case phone, computer }

    var runId: String
    var serial: String
    var goal: String
    var status: Status
    var step: Int
    var inboxId: String?
    var phoneName: String
    /// What the step is doing, when known ("Adding spring rolls").
    var line: String?
    /// `computer_task` cards are a computer run (`serial` is the computer id).
    var kind: Kind = .phone
    /// The current step's one-line label (running / waiting cards). Absent on older runtimes.
    var stepLabel: String? = nil
    /// The run's own result sentence, on a closed card.
    var summary: String? = nil
    /// The stream: the closing trail, else the steps seen so far. Empty without labels.
    var steps: [RunStep] = []
    /// When the run's first event happened; places the run among the messages.
    var startedAt: Date? = nil
    /// False for runs started from the composer: their pill stays on the card (decision 5).
    var hostable: Bool = true
    var id: String { runId }
}

extension PhoneTaskCardModel.Status {
    var label: String {
        switch self {
        case .running: return "Running"
        case .waitingApproval: return "Needs you"
        case .done: return "Done"
        case .denied: return "Denied"
        case .approvalTimeout: return "Timed out"
        case .phoneLocked: return "Phone locked"
        case .failed: return "Failed"
        case .offline: return "Phone offline"
        }
    }

    var tint: Color {
        switch self {
        case .running: return .blue
        case .waitingApproval: return .orange
        case .done: return .green
        case .denied, .failed: return .red
        case .approvalTimeout, .phoneLocked, .offline: return Color("TextSecondary")
        }
    }
}

extension PhoneTaskCardModel {
    /// Folds a live runs row (`GET /phones/history`) into the card. A row without `status` means
    /// the run ended (the runtime drops its live entry): done, or failed when a step failed.
    mutating func apply(_ run: PhoneTaskClient.RunSummary) {
        if serial.isEmpty { serial = run.serial }
        if goal.isEmpty { goal = run.goal }
        if !run.status.isEmpty {
            status = .init(raw: run.status)
            step = max(step, run.step)
            stepLabel = run.stepLabel ?? stepLabel
            line = stepLabel
            let live = RunStep.stream(trail: run.trail, step: run.step, label: run.stepLabel)
            if !live.isEmpty { steps = live }
            if let approval = run.approvalId, !approval.isEmpty { inboxId = "approval:\(approval)" }
        } else if status.isActive {
            status = run.failed > 0 ? .failed : .done
            step = max(step, run.stepCount)
        }
    }
}

/// Starts phone tasks from the composer and keeps each chat's cards.
@MainActor
final class PhoneRunStore: ObservableObject {
    static let shared = PhoneRunStore()

    struct ArmedPhone: Equatable { let serial: String; let name: String }

    /// The "On <phone>" chip: the next send runs on this phone.
    @Published var armed: ArmedPhone?
    @Published private(set) var cards: [String: [PhoneTaskCardModel]] = [:]
    @Published var startError: String?

    private var names: [String: String] = [:]
    private var polls: [String: Task<Void, Never>] = [:]
    /// Runs started from this device's composer (never hosted by a reply).
    private var composerRuns: Set<String> = []
    /// Ledger state per bot: the newest sequence read, and every run folded so far.
    private var cursors: [String: Int] = [:]
    private var folds: [String: [String: RunFold]] = [:]

    static func key(_ sessionId: String?) -> String { sessionId ?? "draft" }

    func cards(for sessionId: String?) -> [PhoneTaskCardModel] { cards[Self.key(sessionId)] ?? [] }

    /// A run's card in any chat (the `allternit://phone-run` sheet).
    func card(runId: String) -> PhoneTaskCardModel? {
        for list in cards.values { if let c = list.first(where: { $0.runId == runId }) { return c } }
        return nil
    }

    func arm(_ phone: ACIComputersClient.CloudPhone) {
        names[phone.serial] = phone.name
        armed = ArmedPhone(serial: phone.serial, name: phone.name)
    }

    /// Debug gallery only.
    func setCards(_ list: [PhoneTaskCardModel], sessionId: String?) { cards[Self.key(sessionId)] = list }

    /// Starts `goal` on the armed phone with `source: "chat"`; the card shows
    /// at once and follows the run.
    func start(goal: String, sessionId: String?, bot: AgentRecord?) async {
        guard let phone = armed else { return }
        let goal = goal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return }
        startError = nil
        let threadId = BotModeStore.shared.threads.first { $0.currentSessionId == sessionId && sessionId != nil }?.id
        do {
            switch try await PhoneTaskClient.shared.start(serial: phone.serial, goal: goal, source: "chat",
                                                          sessionId: sessionId, threadId: threadId, botId: bot?.id) {
            case .refused(let message):
                startError = message
            case .started(let runId):
                composerRuns.insert(runId)
                upsert(PhoneTaskCardModel(runId: runId, serial: phone.serial, goal: goal, status: .running,
                                          step: 0, phoneName: phone.name, startedAt: Date()), key: Self.key(sessionId))
                follow(runId: runId, sessionId: sessionId, botId: bot?.id)
            }
        } catch {
            startError = error.localizedDescription
        }
    }

    func runAgain(_ card: PhoneTaskCardModel, sessionId: String?, bot: AgentRecord?) async {
        let previous = armed
        armed = ArmedPhone(serial: card.serial, name: card.phoneName)
        await start(goal: card.goal, sessionId: sessionId, bot: bot)
        armed = previous
    }

    /// The iMessage app's "Run on my phone": starts on the first live phone and
    /// keeps `SharedPhoneRun` in the App Group current for the panel's card.
    func startForMessages(goal: String, source: String) async {
        var shared = SharedPhoneRun(runId: nil, goal: goal, status: "starting", step: 0, phoneName: "your phone", updatedAt: Date())
        func fail(_ problem: String, _ message: String) { shared.problem = problem; shared.message = message; shared.status = "failed"; shared.save() }
        do {
            let phones = try await PhoneTaskClient.shared.phones()
            guard let phone = phones.first(where: \.isLive) else {
                return phones.isEmpty ? fail("noPhone", "No phone is linked yet. Link one in Phone Link.")
                                      : fail("offline", "\(phones[0].name) is offline.")
            }
            shared.phoneName = phone.name
            names[phone.serial] = phone.name
            switch try await PhoneTaskClient.shared.start(serial: phone.serial, goal: goal, source: source) {
            case .refused(let message): fail("refused", message)
            case .started(let runId):
                shared.runId = runId
                shared.status = "running"
                shared.save()
                composerRuns.insert(runId)
                upsert(PhoneTaskCardModel(runId: runId, serial: phone.serial, goal: goal, status: .running, step: 0,
                                          phoneName: phone.name, startedAt: Date()), key: "messages")
                follow(runId: runId, sessionId: "messages", botId: nil)
            }
        } catch PhoneTaskClient.Failure.quota {
            fail("quota", "Your phone minutes are used up this month.")
        } catch PhoneTaskClient.Failure.signedOut {
            fail("signedOut", "Sign in to Allternit to run tasks on your phone.")
        } catch {
            fail("failed", error.localizedDescription)
        }
    }

    // MARK: Ledger

    /// One run folded from its ledger events (web `reducePhoneTaskCards`).
    private struct RunFold {
        var seq: Int
        var card: PhoneTaskCardModel
        var sessionId: String?
        /// Step labels from `running` events: step → (sequence, label).
        var live: [Int: (Int, String)] = [:]
        /// The newest trail and its sequence.
        var trail: (Int, [RunStep])?

        /// The card with its stream: the newest trail, plus live steps newer than it; a run
        /// waiting for your OK shows the waiting label on its current step.
        var resolved: PhoneTaskCardModel {
            var out = card
            var steps = trail?.1 ?? []
            let trailSeq = trail?.0 ?? Int.min
            for (n, entry) in live where entry.0 > trailSeq || !steps.contains(where: { $0.n == n }) {
                if let i = steps.firstIndex(where: { $0.n == n }) { steps[i].label = entry.1 } else { steps.append(RunStep(n: n, label: entry.1)) }
            }
            steps.sort { $0.n < $1.n }
            if card.status == .waitingApproval { steps = RunStep.stream(trail: steps, step: card.step, label: card.stepLabel) }
            out.steps = steps
            out.line = card.status.isActive ? card.stepLabel : card.summary
            return out
        }
    }

    /// Reads the thread's run cards from the bot ledger. The ledger pages oldest first, so the
    /// store keeps a cursor per bot and folds only new events after the first read.
    func load(sessionId: String?, botId: String?) async {
        guard let sessionId, let botId else { return }
        var after = cursors[botId]
        for _ in 0..<25 {
            guard let page = try? await BotEventsClient.shared.fetchEvents(botId: botId, afterSequence: after, limit: 200,
                                                                         eventTypes: ["phone_task", "computer_task"]) else { break }
            for event in page.events {
                fold(event, botId: botId)
                after = max(after ?? 0, event.sequence)
            }
            if !page.hasMore || page.events.isEmpty { break }
        }
        if let after { cursors[botId] = max(cursors[botId] ?? 0, after) }
        let runs = (folds[botId] ?? [:]).values.filter { $0.sessionId == nil || $0.sessionId == sessionId }
        for run in runs.sorted(by: { ($0.card.startedAt ?? .distantPast) < ($1.card.startedAt ?? .distantPast) }) {
            upsert(run.resolved, key: Self.key(sessionId))
        }
    }

    private func fold(_ event: BotEvent, botId: String) {
        guard let card = model(from: event.payload, eventType: event.eventType, at: Self.date(event.occurredAt)) else { return }
        var runs = folds[botId] ?? [:]
        var run = runs[card.runId] ?? RunFold(seq: Int.min, card: card, sessionId: event.sessionId)
        if event.sequence >= run.seq {
            var newer = card
            if newer.goal.isEmpty { newer.goal = run.card.goal }
            if newer.serial.isEmpty { newer.serial = run.card.serial }
            newer.startedAt = run.card.startedAt
            run.card = newer
            run.seq = event.sequence
        }
        if let at = card.startedAt, at < (run.card.startedAt ?? .distantFuture) { run.card.startedAt = at }
        if run.sessionId == nil { run.sessionId = event.sessionId }
        if !card.steps.isEmpty, event.sequence >= (run.trail?.0 ?? Int.min) { run.trail = (event.sequence, card.steps) }
        if card.status == .running, card.step > 0, let label = card.stepLabel, event.sequence >= (run.live[card.step]?.0 ?? Int.min) {
            run.live[card.step] = (event.sequence, label)
        }
        runs[card.runId] = run
        folds[botId] = runs
    }

    /// One ledger card → a model; `steps` holds the card's own `trail`.
    private func model(from payload: [String: JSONValue], eventType: String, at: Date?) -> PhoneTaskCardModel? {
        func str(_ k: String) -> String? {
            if case .string(let v)? = payload[k] { let t = v.trimmingCharacters(in: .whitespacesAndNewlines); return t.isEmpty ? nil : t }
            return nil
        }
        let type = str("type") ?? eventType
        guard type == "phone_task" || type == "computer_task", let runId = str("runId") else { return nil }
        var step = 0
        if case .number(let n)? = payload["step"], n.isFinite { step = max(0, Int(n)) }
        let serial = str("serial") ?? ""
        let computer = type == "computer_task"
        return PhoneTaskCardModel(runId: runId, serial: serial, goal: str("goal") ?? "",
                                  status: .init(raw: str("status") ?? "running"), step: step,
                                  inboxId: str("inboxId"),
                                  phoneName: names[serial] ?? (computer ? "your computer" : "your phone"),
                                  kind: computer ? .computer : .phone,
                                  stepLabel: str("stepLabel"), summary: str("summary"),
                                  steps: RunStep.list(payload["trail"]), startedAt: at)
    }

    private static func date(_ raw: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: raw) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: raw)
    }

    private func upsert(_ card: PhoneTaskCardModel, key: String) {
        var list = cards[key] ?? []
        var merged = card
        merged.hostable = !composerRuns.contains(card.runId)
        if let i = list.firstIndex(where: { $0.runId == card.runId }) {
            let old = list[i]
            if merged.phoneName == "your phone" || merged.phoneName == "your computer" { merged.phoneName = old.phoneName }
            if merged.steps.isEmpty { merged.steps = old.steps }
            if let a = old.startedAt, a < (merged.startedAt ?? .distantFuture) { merged.startedAt = a }
            if merged.goal.isEmpty { merged.goal = old.goal }
            guard merged != old else { return }
            list[i] = merged
        } else {
            list.append(merged)
        }
        cards[key] = list
    }

    // MARK: Live runs rows

    /// Folds the run's live runs row into its card (in whichever chat holds it, else its own
    /// slot for the `allternit://phone-run` sheet).
    func refreshFromHistory(runId: String) async {
        guard let run = try? await PhoneTaskClient.shared.run(runId) else { return }
        let key = cards.first(where: { $0.value.contains { $0.runId == runId } })?.key ?? "run:\(runId)"
        var card = cards[key]?.first(where: { $0.runId == runId })
            ?? PhoneTaskCardModel(runId: runId, serial: run.serial, goal: run.goal, status: .running, step: 0,
                                  phoneName: names[run.serial] ?? "your phone")
        card.apply(run)
        upsert(card, key: key)
        if key == "messages", var shared = SharedPhoneRun.load(), shared.runId == runId {
            shared.status = card.status.rawValue
            shared.step = card.step
            shared.updatedAt = Date()
            shared.save()
        }
    }

    /// Follows a run started here that has no thread (a plain chat, the iMessage app) through the
    /// live runs rows: every 2 s while running, 4 s while waiting, for up to 15 minutes. A bot
    /// thread's cards are followed by `PhoneTaskCardsSection` through the ledger.
    private func follow(runId: String, sessionId: String?, botId: String?) {
        guard botId == nil || sessionId == nil else { return }
        polls[runId]?.cancel()
        polls[runId] = Task { [weak self] in
            let deadline = Date().addingTimeInterval(15 * 60)
            var status = PhoneTaskCardModel.Status.running
            while Date() < deadline {
                try? await Task.sleep(for: .seconds(status == .waitingApproval ? 4 : 2))
                guard let self, !Task.isCancelled else { return }
                await self.refreshFromHistory(runId: runId)
                status = self.card(runId: runId)?.status ?? .running
                if status.isFinal { break }
            }
            self?.polls[runId] = nil
        }
    }
}

/// The phone task status card, Option B: screen thumbnail, step progress,
/// status pill, and the actions for its state.
struct PhoneTaskCardView: View {
    let card: PhoneTaskCardModel
    /// nil in the debug gallery (it passes `fixedThumb`).
    var loadThumb: (@Sendable (String) async -> UIImage?)? = { await PhoneTaskClient.shared.thumb(runId: $0) }
    var fixedThumb: UIImage? = nil
    var onRunAgain: () -> Void = {}
    /// A bot reply above hosts this run's pill and stream (decision 5): the card drops its own
    /// pill and progress, and keeps the text, thumbnail, Review and Run again.
    var hosted: Bool = false

    @State private var thumb: UIImage?
    @State private var thumbLoaded = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            thumbnail
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: card.kind == .computer ? "desktopcomputer" : "iphone")
                        .font(.system(size: 11, weight: .semibold))
                    Text(card.phoneName)
                        .font(.system(size: 11, weight: .semibold))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if !hosted { RunStatusPill(card: card) }
                }
                .foregroundColor(Color("TextSecondary"))
                Text(card.goal)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
                if !hosted, card.steps.isEmpty, card.status.isActive {
                    ProgressView(value: Double(min(card.step, 15)), total: 15)
                        .tint(card.status.tint)
                        .accessibilityLabel("Step \(card.step)")
                }
                Text(stepLine)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                if !hosted {
                    RunThoughtStream(steps: card.steps, active: card.status.isActive)
                }
                actions
            }
        }
        .padding(12)
        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
        .task(id: "\(card.runId)-\(card.status.rawValue)-\(card.step)") {
            guard let loadThumb else { return }
            thumb = await loadThumb(card.runId)
            thumbLoaded = true
        }
    }


    @ViewBuilder
    private var thumbnail: some View {
        let shape = RoundedRectangle(cornerRadius: 10, style: .continuous)
        ZStack {
            shape.fill(Color("FillNeutral"))
            if let image = thumb ?? fixedThumb {
                Image(uiImage: image).resizable().scaledToFill()
            } else if loadThumb != nil && !thumbLoaded {
                ProgressView().controlSize(.small)
            } else {
                // No kept frame (404) or not the owner: a quiet placeholder, never an error.
                Image(systemName: "iphone.gen3")
                    .font(.system(size: 22))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .frame(width: 64, height: 116)
        .clipShape(shape)
        .overlay(shape.stroke(Color("BorderSubtle")))
        .accessibilityHidden(true)
    }

    private var stepLine: String {
        let n = card.step
        switch card.status {
        case .running: return (card.stepLabel ?? card.line).map { "Step \(n) · \($0)" } ?? (n == 0 ? "Starting on \(card.phoneName)…" : "Step \(n)")
        case .waitingApproval: return card.stepLabel.map { "Step \(n) · \($0)" } ?? "Step \(n) · Stopped before a sensitive step. Waiting for your OK."
        case .done: return card.summary ?? card.line ?? "Finished in \(n) steps."
        case .denied: return "You denied step \(n). Nothing was done on your phone."
        case .approvalTimeout: return "Nobody answered in 5 minutes at step \(n), so nothing was done."
        case .phoneLocked: return "\(card.phoneName) locked at step \(n). Unlock it, then run again."
        case .failed: return card.line.map { "Step \(n) · \($0)" } ?? "The task stopped at step \(n)."
        case .offline: return "\(card.phoneName) stopped answering at step \(n)."
        }
    }

    @ViewBuilder
    private var actions: some View {
        HStack(spacing: 8) {
            switch card.status {
            case .running:
                EmptyView()
            case .waitingApproval:
                actionButton("Review", prominent: true) { card.openReview() }
            case _ where card.kind == .computer:
                EmptyView()
            case .offline:
                actionButton("Open Phone Link", prominent: true) { DeepLinkRouter.shared.enqueue(.computers) }
            case .done, .denied, .approvalTimeout, .phoneLocked, .failed:
                actionButton("Run again", prominent: card.status != .done) { onRunAgain() }
                actionButton("Open Phone Link", prominent: false) { DeepLinkRouter.shared.enqueue(.computers) }
            }
        }
        .padding(.top, 2)
    }

    private func actionButton(_ title: String, prominent: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: .semibold))
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .foregroundColor(prominent ? Color("BgPrimary") : Color("TextPrimary"))
                .background(prominent ? Color("TextPrimary") : Color("FillNeutral"), in: Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// The chat's phone cards, under its messages.
struct PhoneTaskCardsSection: View {
    let sessionId: String?
    let bot: AgentRecord?
    /// Runs whose pill and stream sit on a bot reply (`PhoneRunHosting`).
    var hostedRunIds: Set<String> = []
    @ObservedObject private var store = PhoneRunStore.shared

    var body: some View {
        let list = store.cards(for: sessionId)
        VStack(spacing: 10) {
            if let error = store.startError {
                Text(error)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .onTapGesture { store.startError = nil }
            }
            ForEach(list) { card in
                PhoneTaskCardView(card: card, onRunAgain: {
                    Task { await store.runAgain(card, sessionId: sessionId, bot: bot) }
                }, hosted: hostedRunIds.contains(card.runId))
            }
        }
        .padding(.horizontal, 16)
        .task(id: "\(sessionId ?? "")|\(bot?.id ?? "")") {
            // The thread's ledger: every 2 s while a run moves, 4 s while it waits for your
            // OK, 30 s otherwise (a bot can start a run at any time). Stops with the view.
            guard sessionId != nil, bot != nil else { return }
            while !Task.isCancelled {
                await store.load(sessionId: sessionId, botId: bot?.id)
                let list = store.cards(for: sessionId)
                let secs = list.contains { $0.status == .running } ? 2 : list.contains { $0.status == .waitingApproval } ? 4 : 30
                try? await Task.sleep(for: .seconds(secs))
            }
        }
    }
}
