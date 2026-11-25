import SwiftUI

/// Live run status for a phone or computer run (live-status-pill plan, decisions 5 to 8; web
/// `allternit-ai` `src/components/phones/RunStatus.tsx`): a dark pill on the reply's top-left
/// corner and the step list under the reply text. Data comes from the `phone_task` /
/// `computer_task` cards (`stepLabel`, `trail`) or the live runs rows (`step_label`, `trail`).

/// One step of a run's stream. `ok` is only known from a trail.
struct RunStep: Equatable, Hashable, Sendable, Identifiable {
    let n: Int
    var label: String
    var ok: Bool? = nil
    /// Replayed from the app's map, no model call (App Maps). False on older runtimes.
    var map: Bool = false
    var id: Int { n }

    /// `[{n, label, ok}]` from a ledger card. Malformed entries are dropped; sorted by `n`.
    static func list(_ value: JSONValue?) -> [RunStep] {
        guard case .array(let items)? = value else { return [] }
        let steps = items.compactMap { item -> RunStep? in
            guard case .object(let o) = item, case .number(let n)? = o["n"], case .string(let raw)? = o["label"] else { return nil }
            let label = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !label.isEmpty, n.isFinite else { return nil }
            var ok: Bool?
            if case .bool(let b)? = o["ok"] { ok = b }
            var map = false
            if case .bool(let m)? = o["map"] { map = m }
            return RunStep(n: Int(n), label: label, ok: ok, map: map)
        }
        return steps.sorted { $0.n < $1.n }
    }

    /// The same from a `JSONSerialization` row (`GET /phones/history` `runs[].trail`).
    static func list(any value: Any?) -> [RunStep] {
        guard let items = value as? [[String: Any]] else { return [] }
        let steps = items.compactMap { o -> RunStep? in
            guard let n = (o["n"] as? NSNumber)?.intValue,
                  let label = (o["label"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty else { return nil }
            return RunStep(n: n, label: label, ok: (o["ok"] as? NSNumber)?.boolValue, map: (o["map"] as? NSNumber)?.boolValue ?? false)
        }
        return steps.sorted { $0.n < $1.n }
    }

    /// `trail` (finished steps) plus the current step: the stream of a run in progress. A run
    /// waiting for your OK shows the "Waiting for your OK to …" label on its current step.
    static func stream(trail: [RunStep], step: Int, label: String?) -> [RunStep] {
        var out = trail
        guard let label = label?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty, step > 0 else { return out }
        if let i = out.firstIndex(where: { $0.n == step }) {
            out[i].label = label
        } else {
            out.append(RunStep(n: step, label: label))
            out.sort { $0.n < $1.n }
        }
        return out
    }
}

/// One row of the step list: a plain step, or consecutive map steps folded into one (App Maps C1,
/// web `groupSteps` in `PhoneRunsPanel.tsx`).
enum RunStepRow: Equatable, Identifiable {
    case step(RunStep)
    case map(first: Int, last: Int, destination: String, count: Int)

    var id: Int {
        switch self {
        case .step(let s): return s.n
        case .map(let first, _, _, _): return first
        }
    }

    /// "1–3. Went to Calls & SMS" (a single step reads "2. Went to Wi-Fi").
    var mapLabel: String? {
        guard case .map(let first, let last, let dest, _) = self else { return nil }
        return "\(first == last ? "\(first)" : "\(first)–\(last)"). Went to \(dest)"
    }

    /// Consecutive successful map steps fold into one row. A failed step never folds, so a fallback
    /// stays visible as its own row. Runs with no map flags come out unchanged.
    static func fold(_ steps: [RunStep]) -> [RunStepRow] {
        var out: [RunStepRow] = []
        for s in steps {
            guard s.isMapStep else { out.append(.step(s)); continue }
            if case .map(let first, _, _, let count)? = out.last {
                out[out.count - 1] = .map(first: first, last: s.n, destination: s.mapDestination, count: count + 1)
            } else {
                out.append(.map(first: s.n, last: s.n, destination: s.mapDestination, count: 1))
            }
        }
        return out
    }
}

extension RunStep {
    var isMapStep: Bool { map && ok != false }

    /// The screen a map step went to: "go to Calls & SMS" names it.
    var mapDestination: String {
        let stripped = label.replacingOccurrences(of: "^go to\\s+", with: "", options: [.regularExpression, .caseInsensitive])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return stripped.isEmpty ? label : stripped
    }
}

/// The run-detail title meta and the task-card line (App Maps C1/C2). Both read nothing when the
/// report has no `map_steps`, so older runs look as before.
enum AppMapCopy {
    /// "com.android.settings" becomes "Settings".
    static func appName(fromPackage pkg: String?) -> String? {
        guard let last = (pkg ?? "").split(separator: ".").last, let first = last.first else { return nil }
        return first.uppercased() + last.dropFirst()
    }

    /// `app_label` as sent, else the short name of a package.
    static func mapAppName(_ raw: String?) -> String? {
        let t = (raw ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if t.isEmpty { return nil }
        return t.contains(".") ? appName(fromPackage: t) : t
    }

    /// "3 from the Settings map · 2 model calls", or nil without map steps.
    static func runMeta(mapSteps: Int?, modelCalls: Int?, app: String?) -> String? {
        guard let n = mapSteps, n > 0 else { return nil }
        var out = "\(n) from the \(mapAppName(app) ?? "app") map"
        if let m = modelCalls { out += " · \(m) model \(m == 1 ? "call" : "calls")" }
        return out
    }

    /// "3 steps came from the Settings map, so only 2 needed the model."
    static func cardLine(mapSteps: Int?, modelCalls: Int?, app: String?) -> String? {
        guard let n = mapSteps, n > 0 else { return nil }
        let from = mapAppName(app).map { "the \($0) map" } ?? "the app’s map"
        var out = "\(n) step\(n == 1 ? "" : "s") came from \(from)"
        if let m = modelCalls { out += ", so only \(m) needed the model" }
        return out + "."
    }
}

extension PhoneTaskCardModel.Status {
    /// The stream's last step is current while the run is live; waiting for your OK is live.
    var isActive: Bool { self == .running || self == .waitingApproval }
}

/// What the chat pill says (contract "Text mapping").
enum RunPillKind: Equatable {
    case running, waiting, done, denied, failed

    init(_ status: PhoneTaskCardModel.Status) {
        switch status {
        case .running: self = .running
        case .waitingApproval: self = .waiting
        case .done: self = .done
        case .denied: self = .denied
        case .approvalTimeout, .phoneLocked, .failed, .offline: self = .failed
        }
    }

    func text(computer: Bool) -> String {
        switch self {
        case .running: return computer ? "Using computer" : "Using phone"
        case .waiting: return "Needs your OK"
        case .done: return "Task completed"
        case .denied: return "Denied"
        case .failed: return "Couldn't finish"
        }
    }
}

@MainActor
extension PhoneTaskCardModel {
    /// Opens the existing Review: the hold's inbox item, else the inbox.
    func openReview() {
        if let id = inboxId, !id.isEmpty { DeepLinkRouter.shared.enqueue(.inboxItem(id)) } else { DeepLinkRouter.shared.enqueue(.inbox) }
    }
}

/// The dark status pill. Inverse tokens (TextPrimary fill, BgPrimary text), so it flips in dark mode.
/// "Needs your OK" is a button that opens Review.
struct RunStatusPill: View {
    let card: PhoneTaskCardModel
    var onReview: (() -> Void)? = nil

    @Environment(\.colorScheme) private var colorScheme
    @ReducedMotion private var reduceMotion

    private var kind: RunPillKind { RunPillKind(card.status) }
    private var text: String { kind.text(computer: card.kind == .computer) }

    var body: some View {
        if kind == .waiting {
            Button { if let onReview { onReview() } else { card.openReview() } } label: { capsule }
                .buttonStyle(.plain)
                .contentShape(Rectangle().inset(by: -10))
                .accessibilityLabel(text)
                .accessibilityHint("Opens Review")
        } else {
            capsule
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(kind == .done ? "Task completed" : text)
                .accessibilityAddTraits(.updatesFrequently)
        }
    }

    private var capsule: some View {
        HStack(spacing: 5) {
            leading
            Text(text)
                .font(.system(size: 12, weight: .semibold))
                .lineLimit(1)
                .id(text)
                .transition(.opacity)
        }
        .padding(.leading, kind == .running ? 4 : 8)
        .padding(.trailing, 10)
        .frame(height: 24)
        .foregroundColor(Color("BgPrimary"))
        .background(Color("TextPrimary"), in: Capsule())
        .fixedSize()
        .animation(reduceMotion ? .linear(duration: 0.1) : .easeOut(duration: 0.15), value: text)
    }

    @ViewBuilder
    private var leading: some View {
        switch kind {
        case .running:
            // The orb draws in the opposite ink of the page, like the pill.
            ThinkingOrb(state: .working, size: .inline, paused: reduceMotion, label: text)
                .environment(\.colorScheme, colorScheme == .dark ? .light : .dark)
                .accessibilityHidden(true)
        case .done:
            Text("✓").font(.system(size: 12, weight: .bold)).accessibilityHidden(true)
        case .waiting, .denied, .failed:
            Circle()
                .fill(kind == .waiting ? Color.orange : kind == .failed ? Color.red : Color("BgPrimary").opacity(0.55))
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
        }
    }
}

/// The step list under the reply text: the current step shows › in dark text, done steps ✓ in
/// grey. At most 6 rows; older ones sit behind "Show N earlier steps". Hidden without labels.
struct RunThoughtStream: View {
    let steps: [RunStep]
    let active: Bool

    @State private var showAll = false
    @ReducedMotion private var reduceMotion

    static let maxRows = 6

    var body: some View {
        if !steps.isEmpty {
            let hidden = showAll ? 0 : max(0, steps.count - Self.maxRows)
            let rows = RunStepRow.fold(Array(steps.suffix(steps.count - hidden)))
            let lastN = steps.last?.n
            VStack(alignment: .leading, spacing: 5) {
                if hidden > 0 {
                    Button { showAll = true } label: {
                        Text("Show \(hidden) earlier \(hidden == 1 ? "step" : "steps")")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(minHeight: 28)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                ForEach(rows) { item in
                    Group {
                        switch item {
                        case .step(let step): row(step, current: active && step.n == lastN)
                        case .map(_, let last, _, _): mapRow(item, current: active && last == lastN)
                        }
                    }
                        .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: 4)))
                }
            }
            .padding(.top, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .top) {
                Rectangle().fill(Color("BorderSubtle")).frame(height: 0.5)
            }
            .animation(reduceMotion ? .linear(duration: 0.1) : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.18), value: steps)
            // Consecutive map steps folding into the route row settle with a spring (no travel under Reduce Motion).
            .animation(reduceMotion ? .linear(duration: 0.1) : .spring(response: 0.35, dampingFraction: 0.85), value: rows)
            .animation(reduceMotion ? .linear(duration: 0.1) : .easeOut(duration: 0.12), value: active)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Steps")
        }
    }

    /// Folded map steps: no model, so one row with the map symbol. (No screen thumbnails: the iOS
    /// run list keeps labels only.)
    private func mapRow(_ item: RunStepRow, current: Bool) -> some View {
        let label = item.mapLabel ?? ""
        return HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "map")
                .font(.system(size: 11, weight: .semibold))
                .frame(width: 12, alignment: .center)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .font(.system(size: 13, weight: current ? .medium : .regular))
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                Text("From the map · no model")
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .foregroundColor(current ? Color("TextPrimary") : Color("TextSecondary"))
        .contentTransition(.numericText())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(current ? "Now" : "Done"): \(label), from the map, no model")
    }

    private func row(_ step: RunStep, current: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            // Radio while the step runs, a check once it is done (the dot settles into the tick).
            Group {
                if current {
                    Image(systemName: "circle")
                        .font(.system(size: 10, weight: .semibold))
                        .transition(.opacity)
                } else {
                    Image(systemName: "checkmark")
                        .font(.system(size: 10, weight: .bold))
                        .transition(reduceMotion ? .opacity : .opacity.combined(with: .scale(scale: 0.6)))
                }
            }
            .id(current)
            .frame(width: 12, alignment: .center)
            .accessibilityHidden(true)
            Text(step.label)
                .font(.system(size: 13, weight: current ? .medium : .regular))
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .foregroundColor(current ? Color("TextPrimary") : Color("TextSecondary"))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(current ? "Now: \(step.label)" : "Done: \(step.label)")
    }
}

/// Which run each bot reply hosts (decision 5, web `placeByTime` + `hostRuns`): a run lands after
/// the last message written at or before its first event; when that message is a bot reply, the
/// reply hosts it (the newest such run wins). Runs started from the composer are never hosted.
enum PhoneRunHosting {
    static func hosts(messages: [MessageRecord], cards: [PhoneTaskCardModel]) -> [String: PhoneTaskCardModel] {
        var out: [String: PhoneTaskCardModel] = [:]
        let placed = cards.compactMap { card in card.startedAt.map { (card, $0) } }.sorted { $0.1 < $1.1 }
        for (card, at) in placed where card.hostable {
            guard let host = messages.last(where: { $0.createdAt <= at }),
                  host.role == "assistant", host.voiceSummary == nil else { continue }
            out[host.id] = card
        }
        return out
    }
}

/// `.sheet(item:)` id for `allternit://phone-run/<runId>`.
struct PhoneRunLink: Identifiable, Equatable {
    let id: String
}

/// `allternit://phone-run/<runId>`: that run's pill, stream and Review, in a sheet (no rail tab).
/// Follows the run through the live runs rows: every 2 s while running, 4 s while waiting.
struct PhoneRunSheet: View {
    let runId: String
    /// Gallery only: draw this card and skip the network.
    var fixedCard: PhoneTaskCardModel? = nil

    @ObservedObject private var store = PhoneRunStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var loaded = false

    private var card: PhoneTaskCardModel? { fixedCard ?? store.card(runId: runId) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if let card {
                        content(card)
                    } else if !loaded {
                        ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                    } else {
                        Text("This run isn't in your computer's history, or your computer is offline.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle(card?.kind == .computer ? "Computer run" : "Phone run")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
        .task(id: runId) { await follow() }
    }

    @ViewBuilder
    private func content(_ card: PhoneTaskCardModel) -> some View {
        RunStatusPill(card: card, onReview: { review(card) })
        Text(card.goal.isEmpty ? "Phone task" : card.goal)
            .font(.system(size: 17, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .fixedSize(horizontal: false, vertical: true)
        if let meta = AppMapCopy.runMeta(mapSteps: card.mapSteps, modelCalls: card.modelCalls, app: card.mapApp) {
            Text(meta)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
        }
        if let summary = card.summary, card.status.isFinal {
            Text(summary)
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        if card.steps.isEmpty {
            Text(card.step > 0 ? "Step \(card.step)" : "Starting…")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
        } else {
            RunThoughtStream(steps: card.steps, active: card.status.isActive)
        }
        if card.status == .waitingApproval {
            Button { review(card) } label: {
                Text("Review")
                    .font(.system(size: 15, weight: .semibold))
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .foregroundColor(Color("BgPrimary"))
                    .background(Color("TextPrimary"), in: Capsule())
            }
            .buttonStyle(.plain)
            .padding(.top, 4)
        }
    }

    private func review(_ card: PhoneTaskCardModel) {
        guard fixedCard == nil else { return }
        dismiss()
        // One sheet at a time: Review opens once this one is down.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { card.openReview() }
    }

    private func follow() async {
        guard fixedCard == nil else { loaded = true; return }
        let deadline = Date().addingTimeInterval(15 * 60)
        while !Task.isCancelled, Date() < deadline {
            await store.refreshFromHistory(runId: runId)
            loaded = true
            guard let status = store.card(runId: runId)?.status, status.isActive else { return }
            try? await Task.sleep(for: .seconds(status == .waitingApproval ? 4 : 2))
        }
    }
}

/// "0:30" / "1:05:09". Whole seconds, never negative.
enum RunClock {
    static func text(_ seconds: TimeInterval) -> String {
        let t = max(0, Int(seconds))
        let h = t / 3600, m = (t % 3600) / 60, s = t % 60
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }
}

/// The run's timer: ticks each second while it runs (start time is the run's first event); once the
/// run closed it shows the time this device saw it finish, or nothing when that was not seen.
struct RunElapsedText: View {
    let card: PhoneTaskCardModel

    var body: some View {
        if let start = card.startedAt {
            if card.status.isActive {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    Text(RunClock.text(ctx.date.timeIntervalSince(start)))
                }
                .monospacedDigit()
                .accessibilityLabel("Running for \(RunClock.text(Date().timeIntervalSince(start)))")
            } else if let end = card.endedAt {
                Text(RunClock.text(end.timeIntervalSince(start))).monospacedDigit()
            }
        }
    }
}

/// "n steps" bar for a live run. The runtime's step cap (15) is the bar's end, as before; the
/// label never claims a total the runtime did not send.
struct RunStepBar: View {
    let step: Int
    let tint: Color
    static let cap = 15

    @ReducedMotion private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("FillNeutral"))
                    Capsule().fill(tint)
                        .frame(width: geo.size.width * CGFloat(min(max(step, 0), Self.cap)) / CGFloat(Self.cap))
                        .animation(reduceMotion ? .linear(duration: 0.1) : .spring(response: 0.35, dampingFraction: 0.85), value: step)
                }
            }
            .frame(height: 4)
            Text("\(step) \(step == 1 ? "step" : "steps")")
                .font(.system(size: 11))
                .monospacedDigit()
                .foregroundColor(Color("TextSecondary"))
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(step)")
    }
}

/// One mono beam lap around a card edge while a run works (web `WorkingBeam`, colorVariant mono,
/// strength 0.8). Starts after 3 s of running; off under Reduce Motion.
struct RunBeamBorder: ViewModifier {
    let active: Bool
    let radius: CGFloat
    @ReducedMotion private var reduceMotion
    @State private var since: Date?
    @State private var shown = false

    func body(content: Content) -> some View {
        content
            .overlay {
                if shown && active && !reduceMotion {
                    TimelineView(.animation) { ctx in
                        let t = ctx.date.timeIntervalSinceReferenceDate
                        let angle = Angle.degrees((t.truncatingRemainder(dividingBy: 2.4) / 2.4) * 360)
                        RoundedRectangle(cornerRadius: radius, style: .continuous)
                            .strokeBorder(
                                AngularGradient(stops: [
                                    .init(color: Color("TextPrimary").opacity(0), location: 0),
                                    .init(color: Color("TextPrimary").opacity(0), location: 0.7),
                                    .init(color: Color("TextPrimary").opacity(0.8), location: 1)
                                ], center: .center, angle: angle),
                                lineWidth: 1.5)
                    }
                    .allowsHitTesting(false)
                    .transition(.opacity)
                    .accessibilityHidden(true)
                }
            }
            .task(id: active) {
                guard active else { shown = false; return }
                try? await Task.sleep(for: .seconds(3))
                if !Task.isCancelled { withAnimation(.easeOut(duration: 0.2)) { shown = true } }
            }
    }
}

extension View {
    func runBeam(active: Bool, radius: CGFloat = 16) -> some View { modifier(RunBeamBorder(active: active, radius: radius)) }
}
