import SwiftUI
import UIKit

/// The `phone_task` card on a thread (docs/api/phone-holds.mdx): newest event
/// per `runId` wins. `offline` is client-side (the phone stopped answering).
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

    var runId: String
    var serial: String
    var goal: String
    var status: Status
    var step: Int
    var inboxId: String?
    var phoneName: String
    /// What the step is doing, when known ("Adding spring rolls").
    var line: String?
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

    static func key(_ sessionId: String?) -> String { sessionId ?? "draft" }

    func cards(for sessionId: String?) -> [PhoneTaskCardModel] { cards[Self.key(sessionId)] ?? [] }

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
                upsert(PhoneTaskCardModel(runId: runId, serial: phone.serial, goal: goal, status: .running,
                                          step: 0, phoneName: phone.name), sessionId: sessionId)
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
                upsert(PhoneTaskCardModel(runId: runId, serial: phone.serial, goal: goal, status: .running, step: 0, phoneName: phone.name), sessionId: "messages")
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

    /// Reads the thread's `phone_task` events from the bot ledger.
    func load(sessionId: String?, botId: String?) async {
        guard let sessionId, let botId else { return }
        guard let page = try? await BotEventsClient.shared.fetchEvents(botId: botId, limit: 200, eventTypes: ["phone_task"]) else { return }
        var latest: [String: (Int, PhoneTaskCardModel)] = [:]
        for event in page.events where event.sessionId == nil || event.sessionId == sessionId {
            guard let card = model(from: event.payload) else { continue }
            if let seen = latest[card.runId], seen.0 > event.sequence { continue }
            latest[card.runId] = (event.sequence, card)
        }
        guard !latest.isEmpty else { return }
        for (_, pair) in latest.sorted(by: { $0.value.0 < $1.value.0 }) { upsert(pair.1, sessionId: sessionId) }
    }

    private func model(from payload: [String: JSONValue]) -> PhoneTaskCardModel? {
        func str(_ k: String) -> String? { if case .string(let v)? = payload[k] { return v } else { return nil } }
        guard str("type") == nil || str("type") == "phone_task", let runId = str("runId") else { return nil }
        var step = 0
        if case .number(let n)? = payload["step"] { step = Int(n) }
        let serial = str("serial") ?? ""
        return PhoneTaskCardModel(runId: runId, serial: serial, goal: str("goal") ?? "",
                                  status: .init(raw: str("status") ?? "running"), step: step,
                                  inboxId: str("inboxId"), phoneName: names[serial] ?? "your phone")
    }

    private func upsert(_ card: PhoneTaskCardModel, sessionId: String?) {
        let key = Self.key(sessionId)
        var list = cards[key] ?? []
        if let i = list.firstIndex(where: { $0.runId == card.runId }) {
            var merged = card
            if merged.phoneName == "your phone" { merged.phoneName = list[i].phoneName }
            list[i] = merged
        } else {
            list.append(card)
        }
        cards[key] = list
    }

    /// Follows a run for up to 6 minutes: the ledger on a bot thread, else
    /// the run history (a plain chat has no thread for the runtime to post on).
    private func follow(runId: String, sessionId: String?, botId: String?) {
        polls[runId]?.cancel()
        polls[runId] = Task { [weak self] in
            for _ in 0..<90 {
                try? await Task.sleep(for: .seconds(4))
                guard let self, !Task.isCancelled else { return }
                if botId != nil, sessionId != nil {
                    await self.load(sessionId: sessionId, botId: botId)
                } else if let run = try? await PhoneTaskClient.shared.run(runId), !run.status.isEmpty,
                          var card = self.cards(for: sessionId).first(where: { $0.runId == runId }) {
                    card.status = .init(raw: run.status)
                    self.upsert(card, sessionId: sessionId)
                    if sessionId == "messages", var shared = SharedPhoneRun.load(), shared.runId == runId {
                        shared.status = card.status.rawValue
                        shared.updatedAt = Date()
                        shared.save()
                    }
                }
                if self.cards(for: sessionId).first(where: { $0.runId == runId })?.status.isFinal == true { break }
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

    @State private var thumb: UIImage?
    @State private var thumbLoaded = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            thumbnail
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: "iphone")
                        .font(.system(size: 11, weight: .semibold))
                    Text(card.phoneName)
                        .font(.system(size: 11, weight: .semibold))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    pill
                }
                .foregroundColor(Color("TextSecondary"))
                Text(card.goal)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
                if card.status == .running || card.status == .waitingApproval {
                    ProgressView(value: Double(min(card.step, 15)), total: 15)
                        .tint(card.status.tint)
                        .accessibilityLabel("Step \(card.step)")
                }
                Text(stepLine)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
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

    private var pill: some View {
        Text(card.status.label)
            .font(.system(size: 11, weight: .semibold))
            .foregroundColor(card.status.tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(card.status.tint.opacity(0.12), in: Capsule())
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
        case .running: return card.line.map { "Step \(n) · \($0)" } ?? (n == 0 ? "Starting on \(card.phoneName)…" : "Step \(n)")
        case .waitingApproval: return "Step \(n) · Stopped before a sensitive step. Waiting for your OK."
        case .done: return card.line ?? "Finished in \(n) steps."
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
                actionButton("Review", prominent: true) {
                    if let id = card.inboxId { DeepLinkRouter.shared.enqueue(.inboxItem(id)) } else { DeepLinkRouter.shared.enqueue(.inbox) }
                }
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
                })
            }
        }
        .padding(.horizontal, 16)
        .task(id: sessionId) { await store.load(sessionId: sessionId, botId: bot?.id) }
    }
}
