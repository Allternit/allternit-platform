import Messages
import SwiftUI
import UIKit

/// The live iMessage card for a phone or computer run (live-status-pill plan, decisions 9 and 10,
/// contract "iMessage card"). One `MSMessage` on one `MSSession`: each update replaces the bubble
/// in place. Everyone in the conversation sees the caption (the goal) and the status line; only the
/// sender's panel shows the step stream.

// MARK: - Text

enum IMRunText {
    static let terminal: Set<String> = ["done", "denied", "approval_timeout", "phone_locked", "failed", "offline"]

    /// The subcaption and `summaryText` (contract "Text mapping", iMessage column).
    static func line(status: String, stepLabel: String?, summary: String?, computer: Bool) -> String {
        switch status {
        case "starting", "running":
            let label = stepLabel?.trimmingCharacters(in: .whitespacesAndNewlines)
            let step = (label?.isEmpty == false ? label : nil) ?? "Starting"
            return "\(computer ? "Using computer" : "Using phone") · \(step)"
        case "waiting_approval": return "Needs your OK · tap to review"
        case "done":
            if let s = summary?.trimmingCharacters(in: .whitespacesAndNewlines), !s.isEmpty { return "✓ Done · \(s)" }
            return "✓ Done"
        case "denied": return "Denied"
        default: return "Couldn't finish"
        }
    }
}

// MARK: - The card

enum IMRunMessage {
    static let queryKey = "allternit_phone_run"

    /// `https://ai.allternit.com/?allternit_phone_run=<runId>`; the plain site before the run has an id.
    static func url(runId: String?) -> URL {
        var c = URLComponents(string: "https://ai.allternit.com/")!
        if let runId, !runId.isEmpty { c.queryItems = [URLQueryItem(name: queryKey, value: runId)] }
        return c.url!
    }

    /// The run id a card carries, if it is an Allternit run card.
    static func runId(of message: MSMessage?) -> String? {
        guard let url = message?.url, let c = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        return c.queryItems?.first(where: { $0.name == queryKey })?.value
    }

    @MainActor static func make(session: MSSession, goal: String, line: String, runId: String?) -> MSMessage {
        let layout = MSMessageTemplateLayout()
        layout.image = cardImage
        layout.caption = goal
        layout.subcaption = line
        let message = MSMessage(session: session)
        message.layout = layout
        message.url = url(runId: runId)
        message.summaryText = line
        return message
    }

    /// The black A:// card. Drawn once; the same in light and dark mode.
    @MainActor static let cardImage: UIImage = {
        let size = CGSize(width: 600, height: 300)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 2
        format.opaque = true
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            UIColor.black.setFill()
            ctx.fill(CGRect(origin: .zero, size: size))
            let mark = NSAttributedString(string: "A://", attributes: [
                .font: UIFont.monospacedSystemFont(ofSize: 96, weight: .heavy),
                .foregroundColor: UIColor.white,
            ])
            let m = mark.size()
            mark.draw(at: CGPoint(x: (size.width - m.width) / 2, y: (size.height - m.height) / 2 - 12))
            let name = NSAttributedString(string: "ALLTERNIT", attributes: [
                .font: UIFont.systemFont(ofSize: 20, weight: .semibold),
                .foregroundColor: UIColor.white.withAlphaComponent(0.55),
                .kern: 4,
            ])
            let n = name.size()
            name.draw(at: CGPoint(x: (size.width - n.width) / 2, y: size.height - n.height - 36))
        }
    }()
}

// MARK: - Send limits (decision 9)

/// One run's card in one conversation, archived in the App Group so the session survives the
/// extension being torn down between visits.
struct IMRunThread: Codable, Equatable, Sendable {
    var conversationKey: String
    var goal: String
    var startedAt: Date
    var runId: String?
    var kind: String = "phone"
    /// `NSKeyedArchiver` data of the card's `MSSession`.
    var session: Data
    var sends: Int = 0
    var lastStatus: String = ""
    var lastStep: Int = -1
    var lastSentAt: Date = .distantPast
    var lastLine: String = ""
    /// An update Messages refused without a recent tap; it is staged in the input field.
    var pendingLine: String?

    var finished: Bool { IMRunText.terminal.contains(lastStatus) && pendingLine == nil }
}

enum IMSendRule {
    static let maxSends = 12
    static let stepGap: TimeInterval = 8

    /// Every status change; a step change at most once per 8 s; at most 12 sends per run. The last
    /// send is kept for a status change so the closing card always fits.
    static func due(_ t: IMRunThread, status: String, step: Int, now: Date) -> Bool {
        guard t.sends < maxSends else { return false }
        if status != t.lastStatus { return true }
        if step != t.lastStep, t.sends < maxSends - 1, now.timeIntervalSince(t.lastSentAt) >= stepGap { return true }
        return false
    }
}

enum IMRunThreadStore {
    private static let key = "surfaces.messages.runThreads"

    static func all() -> [String: IMRunThread] {
        guard let data = AppGroup.defaults?.data(forKey: key) else { return [:] }
        let d = JSONDecoder(); d.dateDecodingStrategy = .iso8601
        return (try? d.decode([String: IMRunThread].self, from: data)) ?? [:]
    }

    static func load(_ conversationKey: String) -> IMRunThread? { all()[conversationKey] }

    static func save(_ t: IMRunThread) {
        var map = all().filter { Date().timeIntervalSince($0.value.startedAt) < 86_400 }
        map[t.conversationKey] = t
        let e = JSONEncoder(); e.dateEncodingStrategy = .iso8601
        AppGroup.defaults?.set(try? e.encode(map), forKey: key)
    }
}

// MARK: - Controller

/// Follows the run this conversation started: polls `phoneRun` every 3 s while the panel is
/// visible and stages the one card per the send rule. Messages does not post an extension's
/// message without a fresh tap, so each update waits in the message field for the user's send,
/// and the panel says so.
@MainActor
final class IMRunController: ObservableObject {
    @Published private(set) var thread: IMRunThread?
    @Published private(set) var run: StatusSnapshot.PhoneRun?
    /// The card the user tapped: its run id, caption and status line, and whether this user sent it.
    @Published private(set) var selected: Selected?

    struct Selected: Equatable {
        var runId: String?
        var caption: String
        var line: String
        var mine: Bool
    }

    private weak var conversation: MSConversation?
    private var poll: Task<Void, Never>?
    private var client = StatusSummaryClient()

    static let pollEvery: Duration = .seconds(3)

    var line: String {
        guard let t = thread else { return "" }
        if let run { return IMRunText.line(status: run.status, stepLabel: run.stepLabel, summary: run.summary, computer: run.kind == "computer") }
        return t.lastLine
    }

    static func key(_ c: MSConversation) -> String {
        let remote = c.remoteParticipantIdentifiers.map(\.uuidString).sorted().joined(separator: ",")
        return "\(c.localParticipantIdentifier.uuidString)|\(remote)"
    }

    func attach(_ conversation: MSConversation) {
        self.conversation = conversation
        thread = IMRunThreadStore.load(Self.key(conversation))
        select(conversation.selectedMessage, in: conversation)
    }

    /// A card was tapped (or the panel opened on one).
    func select(_ message: MSMessage?, in conversation: MSConversation) {
        guard let message, message.url?.host == "ai.allternit.com" else { selected = nil; return }
        let layout = message.layout as? MSMessageTemplateLayout
        let mine = message.senderParticipantIdentifier == conversation.localParticipantIdentifier
        let id = IMRunMessage.runId(of: message)
        selected = Selected(runId: id, caption: layout?.caption ?? "", line: layout?.subcaption ?? "", mine: mine)
        // The tapped bubble's session is the card's session: keep using it.
        if mine, let session = message.session, var t = thread, id == nil || t.runId == nil || t.runId == id,
           let data = try? NSKeyedArchiver.archivedData(withRootObject: session, requiringSecureCoding: true) {
            t.session = data
            if t.runId == nil { t.runId = id }
            persist(t)
        }
    }

    func clearSelection() { selected = nil }

    // MARK: Lifecycle

    func resume() {
        poll?.cancel()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.tick()
                if self.thread?.finished ?? true { return }
                try? await Task.sleep(for: Self.pollEvery)
            }
        }
    }

    func pause() {
        poll?.cancel()
        poll = nil
    }

    /// The user's "Run" tap: a new session and the first card, staged in the message field.
    func start(goal: String, computer: Bool = false) {
        guard let conversation else { return }
        let session = MSSession()
        guard let data = try? NSKeyedArchiver.archivedData(withRootObject: session, requiringSecureCoding: true) else { return }
        let t = IMRunThread(conversationKey: Self.key(conversation), goal: goal, startedAt: Date(),
                            kind: computer ? "computer" : "phone", session: data)
        run = nil
        persist(t)
        deliver(status: "starting", step: 0, line: IMRunText.line(status: "starting", stepLabel: nil, summary: nil, computer: computer))
        resume()
    }

    /// "Update card": stage the newest status again (the input field may have been cleared).
    func updateNow() {
        guard let t = thread else { return }
        let status = run?.status ?? t.lastStatus
        deliver(status: status, step: run?.step ?? t.lastStep, line: line, force: true)
    }

    /// The user sent the staged card: that send counts toward the 12.
    func didSend(_ message: MSMessage) {
        guard var t = thread, message.url?.host == "ai.allternit.com", let sent = message.summaryText else { return }
        t.pendingLine = nil
        t.lastLine = sent
        t.sends += 1
        persist(t)
    }

    // MARK: Poll

    func tick() async {
        guard var t = thread, !t.finished else { return }
        let snap = await fetch()
        // The app writes the run id it started for this goal into the App Group.
        if t.runId == nil, let shared = SharedPhoneRun.load(), shared.goal == t.goal, shared.updatedAt >= t.startedAt.addingTimeInterval(-5) {
            if let id = shared.runId { t.runId = id; persist(t) }
            else if shared.status == "failed" {
                deliver(status: "failed", step: 0, line: IMRunText.line(status: "failed", stepLabel: nil, summary: nil, computer: false))
                return
            }
        }
        guard let pr = snap?.phoneRun, !pr.runId.isEmpty else { return }
        let ours = t.runId.map { $0 == pr.runId } ?? (pr.goal == t.goal && Date().timeIntervalSince(t.startedAt) < 900)
        guard ours else { return }
        if t.runId == nil { t.runId = pr.runId; persist(t) }
        run = pr
        let line = IMRunText.line(status: pr.status, stepLabel: pr.stepLabel, summary: pr.summary, computer: pr.kind == "computer")
        if IMSendRule.due(t, status: pr.status, step: pr.step, now: Date()) {
            deliver(status: pr.status, step: pr.step, line: line)
        }
    }

    private func fetch() async -> StatusSnapshot? {
        #if DEBUG
        if let fixture = Self.fixture() { return fixture }
        #endif
        if case .success(let snap) = await client.fetch() { return snap }
        return nil
    }

    // MARK: Send

    /// Stages the card on the run's session. Proven in the iOS 26.4 simulator (2026-11-12):
    /// `send` without a fresh tap never posts (no completion, nothing in the transcript), so every
    /// update is inserted into the input field and the user's send tap posts it; Messages then
    /// replaces the session's earlier bubble in place.
    private func deliver(status: String, step: Int, line: String, force: Bool = false) {
        guard let conversation, var t = thread else { return }
        guard force || t.sends < IMSendRule.maxSends else { return }
        let session = (try? NSKeyedUnarchiver.unarchivedObject(ofClass: MSSession.self, from: t.session)) ?? MSSession()
        let message = IMRunMessage.make(session: session, goal: t.goal, line: line, runId: t.runId ?? run?.runId)
        t.lastStatus = status
        t.lastStep = step
        t.lastSentAt = Date()
        t.pendingLine = line
        persist(t)
        conversation.insert(message, completionHandler: nil)
    }

    private func persist(_ t: IMRunThread) {
        thread = t
        IMRunThreadStore.save(t)
    }

    #if DEBUG
    /// Capture fixture only (debug builds): `<App Group>/Fixtures/imessage-status.json`, a status
    /// snapshot with a `phoneRun`, stands in for `GET /api/v1/status/summary`.
    static func fixture() -> StatusSnapshot? {
        guard let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier),
              let data = try? Data(contentsOf: dir.appendingPathComponent("Fixtures/imessage-status.json")) else { return nil }
        return StatusSummaryClient.decodeNewest(data)
    }

    static var fixtureActive: Bool { fixture() != nil }
    #endif
}

// MARK: - Panel views

/// The panel's live run: pill, goal, the step stream, and the card's update state.
struct IMLiveRunSection: View {
    @ObservedObject var live: IMRunController
    var open: (URL) -> Void = { _ in }

    var body: some View {
        if let sel = live.selected, !sel.mine || live.thread == nil || (sel.runId != nil && live.thread?.runId != sel.runId) {
            // A recipient sees the caption only; the sender's older card also links to its run.
            IMCaptionOnly(caption: sel.caption, line: sel.line)
            if sel.mine, let id = sel.runId, let url = URL(string: "allternit://phone-run/\(id)") {
                Button("Open in Allternit") { open(url) }
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("BgPrimary"))
                    .padding(.horizontal, 12).frame(minHeight: 30)
                    .background(Color("TextPrimary"), in: Capsule())
            }
        } else if let t = live.thread {
            IMRunStreamCard(goal: t.goal, status: live.run?.status ?? (t.lastStatus.isEmpty ? "starting" : t.lastStatus),
                            computer: (live.run?.kind ?? t.kind) == "computer",
                            steps: IMRunStep.stream(live.run), line: live.line, pending: t.pendingLine != nil,
                            sends: t.sends, runId: t.runId ?? live.run?.runId,
                            onUpdate: { live.updateNow() }, open: open)
        }
    }
}

struct IMCaptionOnly: View {
    let caption: String
    let line: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(caption).font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(line).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

struct IMRunStep: Identifiable, Equatable {
    let n: Int
    let label: String
    var id: Int { n }

    /// Trail plus the current step (same as the app's `RunStep.stream`).
    static func stream(_ run: StatusSnapshot.PhoneRun?) -> [IMRunStep] {
        guard let run else { return [] }
        var out = run.trail.map { IMRunStep(n: $0.n, label: $0.label) }
        if let label = run.stepLabel?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty, run.step > 0 {
            if let i = out.firstIndex(where: { $0.n == run.step }) { out[i] = IMRunStep(n: run.step, label: label) }
            else { out.append(IMRunStep(n: run.step, label: label)); out.sort { $0.n < $1.n } }
        }
        return out
    }
}

/// The stream card (decisions 6 to 8, minimal copy of the app's `RunStatusPill` and
/// `RunThoughtStream`: the extension can't link the app's orb).
struct IMRunStreamCard: View {
    let goal: String
    let status: String
    let computer: Bool
    let steps: [IMRunStep]
    let line: String
    let pending: Bool
    let sends: Int
    let runId: String?
    var onUpdate: () -> Void = {}
    var open: (URL) -> Void = { _ in }

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showAll = false

    private var active: Bool { status == "running" || status == "waiting_approval" || status == "starting" }
    private var pillText: String {
        switch status {
        case "starting", "running": return computer ? "Using computer" : "Using phone"
        case "waiting_approval": return "Needs your OK"
        case "done": return "✓ Task completed"
        case "denied": return "Denied"
        default: return "Couldn't finish"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 5) {
                if status == "waiting_approval" || !active {
                    if status != "done" {
                        Circle().fill(status == "waiting_approval" ? Color.orange : status == "denied" ? Color("BgPrimary").opacity(0.55) : Color.red)
                            .frame(width: 6, height: 6)
                    }
                } else {
                    IMWorkingDot(paused: reduceMotion)
                }
                Text(pillText).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            }
            .padding(.horizontal, 10).frame(height: 24)
            .foregroundColor(Color("BgPrimary"))
            .background(Color("TextPrimary"), in: Capsule())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(pillText)

            Text(goal).font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(2)
            stream
            if pending {
                HStack(spacing: 8) {
                    Text("The update is in the message field. Tap send to change the card in this chat.")
                        .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 4)
                    Button("Update card", action: onUpdate)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .padding(.horizontal, 12).frame(minHeight: 30)
                        .background(Color("TextPrimary"), in: Capsule())
                }
            }
            HStack {
                Text(sends >= IMSendRule.maxSends ? "Card updates paused (12 sent)" : "One card in this chat updates as it runs")
                    .font(.system(size: 11)).foregroundColor(Color("TextTertiary"))
                Spacer(minLength: 4)
                if let runId, let url = URL(string: "allternit://phone-run/\(runId)") {
                    Button("Open in Allternit") { open(url) }
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12).frame(minHeight: 30)
                        .overlay(Capsule().stroke(Color("BorderSubtle")))
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle")))
    }

    @ViewBuilder
    private var stream: some View {
        if !steps.isEmpty {
            let hidden = showAll ? 0 : max(0, steps.count - 6)
            let rows = Array(steps.suffix(steps.count - hidden))
            VStack(alignment: .leading, spacing: 5) {
                if hidden > 0 {
                    Button("Show \(hidden) earlier \(hidden == 1 ? "step" : "steps")") { showAll = true }
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(minHeight: 28)
                }
                ForEach(rows) { step in
                    let current = active && step.n == steps.last?.n
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(current ? "›" : "✓").font(.system(size: 13, weight: .semibold)).frame(width: 12)
                        Text(step.label).font(.system(size: 13, weight: current ? .medium : .regular)).lineLimit(2)
                    }
                    .foregroundColor(current ? Color("TextPrimary") : Color("TextSecondary"))
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(current ? "Now: \(step.label)" : "Done: \(step.label)")
                    .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: 4)))
                }
            }
            .padding(.top, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 0.5) }
            .animation(reduceMotion ? .linear(duration: 0.1) : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.18), value: steps)
        } else {
            Text(line).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        }
    }
}

/// A small pulsing dot in the pill while the run works; static with Reduce Motion.
struct IMWorkingDot: View {
    let paused: Bool
    @State private var on = false

    var body: some View {
        Circle().fill(Color("BgPrimary"))
            .frame(width: 7, height: 7)
            .opacity(paused ? 0.9 : (on ? 1 : 0.35))
            .onAppear { if !paused { withAnimation(.easeInOut(duration: 0.7).repeatForever()) { on = true } } }
            .accessibilityHidden(true)
    }
}
