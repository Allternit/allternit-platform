import SwiftUI

/// Every question an agent is waiting on (across sessions) plus pending
/// Factory approvals — port of allternit-ai DecisionsDeck.tsx (Tasks →
/// Decisions) and the Factory approvals the PWA's FactoryNotificationHost
/// leads to. Polls while on screen; renders nothing when nothing waits.
@MainActor
final class DecisionsStore: ObservableObject {
    static let shared = DecisionsStore()

    @Published private(set) var questions: [AgentQuestionRequest] = []
    @Published private(set) var approvals: [FactoryApprovalsClient.Approval] = []
    @Published private(set) var questionsError: String? = nil
    @Published private(set) var approvalsError: String? = nil
    @Published private(set) var hasLoaded = false
    /// Who asked, by session id: the session's agent name, else its bot.
    @Published private(set) var agentNames: [String: String] = [:]

    private let asks: AgentAsksClient
    private let factory: FactoryApprovalsClient
    private var pollTask: Task<Void, Never>? = nil
    private var watchers = 0
    private var resolvedApprovals: Set<String> = []
    /// Bumped by `reset()`; a fetch started before it is not applied.
    private var generation = 0

    /// Web POLL_MS.
    private static let pollNs: UInt64 = 5_000_000_000

    init(asks: AgentAsksClient = .shared, factory: FactoryApprovalsClient = .shared) {
        self.asks = asks
        self.factory = factory
    }

    var waitingCount: Int { visibleQuestions.count + approvals.count }

    /// Done / Later on the deck (this phone only; the agent keeps waiting and the
    /// question stays in its chat). `snoozedUntil` is when a Later question returns.
    @Published private(set) var handled: Set<String> = []
    @Published private(set) var snoozedUntil: [String: Date] = [:]

    var visibleQuestions: [AgentQuestionRequest] {
        let now = Date()
        return questions.filter {
            !AgentAskStore.shared.isResolved($0.id)
                && !handled.contains($0.id)
                && (snoozedUntil[$0.id].map { $0 <= now } ?? true)
        }
    }

    /// Done: the card leaves the deck. The question is not answered or rejected.
    func markDone(_ id: String) { handled.insert(id) }

    /// Later: the card returns after `minutes`.
    func snooze(_ id: String, minutes: Int) {
        snoozedUntil[id] = Date().addingTimeInterval(TimeInterval(minutes * 60))
    }

    func startWatching() {
        watchers += 1
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            var tick = 0
            while !Task.isCancelled {
                guard let self else { return }
                await self.refreshQuestions()
                // Approvals change slower; every third poll (15 s).
                if tick % 3 == 0 { await self.refreshApprovals() }
                self.hasLoaded = true
                tick += 1
                try? await Task.sleep(nanoseconds: Self.pollNs)
            }
        }
    }

    func stopWatching() {
        watchers = max(0, watchers - 1)
        guard watchers == 0 else { return }
        pollTask?.cancel()
        pollTask = nil
    }

    func retry() {
        Task {
            await refreshQuestions()
            await refreshApprovals()
        }
    }

    private func refreshQuestions() async {
        let started = generation
        do {
            let fetched = try await asks.listQuestions()
            guard started == generation else { return }
            questions = fetched
            questionsError = nil
            // Drop the Done / Later marks of questions that are gone, and expired snoozes
            // (the assignment also wakes the deck so a returned card shows).
            let ids = Set(fetched.map(\.id))
            handled = handled.intersection(ids)
            let now = Date()
            snoozedUntil = snoozedUntil.filter { ids.contains($0.key) && $0.value > now }
            await resolveAgentNames()
        } catch {
            guard started == generation else { return }
            questionsError = "Couldn't load decisions. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func resolveAgentNames() async {
        let missing = Set(questions.map(\.sessionID)).subtracting(agentNames.keys)
        let started = generation
        guard !missing.isEmpty,
              let sessions = try? await AgentChatClient().listSessions(),
              started == generation else { return }
        var names = agentNames
        for session in sessions where missing.contains(session.id) {
            if let name = session.agentName, !name.isEmpty {
                names[session.id] = name
            } else if let agentId = session.agentId, let agent = AgentHubStore.shared.agent(withId: agentId) {
                names[session.id] = agent.botDisplayName
            }
        }
        agentNames = names
    }

    private func refreshApprovals() async {
        let started = generation
        do {
            let pending = try await factory.listPending()
            guard started == generation else { return }
            let listed = Set(pending.map(\.id))
            resolvedApprovals = resolvedApprovals.intersection(listed)
            approvals = pending.filter { !resolvedApprovals.contains($0.id) }
            approvalsError = nil
        } catch {
            guard started == generation else { return }
            approvalsError = "Couldn't load approvals. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// Account switch (phone calm I14b, org and person): drops the leaving
    /// account's questions, approvals and names. Watchers keep polling and
    /// fill it again with the new session; a poll already in flight is
    /// dropped (`generation`).
    func reset() {
        generation += 1
        questions = []
        approvals = []
        questionsError = nil
        approvalsError = nil
        hasLoaded = false
        agentNames = [:]
        handled = []
        snoozedUntil = [:]
        resolvedApprovals = []
    }

    /// `decision`: "approve" | "reject".
    func resolve(_ approval: FactoryApprovalsClient.Approval, decision: String) async throws {
        try await factory.resolve(id: approval.id, decision: decision)
        resolvedApprovals.insert(approval.id)
        approvals.removeAll { $0.id == approval.id }
    }
}

/// The deck: "Decisions · 3 waiting for you · 1 of 3", the current question
/// answered in place, then the next slides in. Factory approvals list under
/// it. Mount it at the top of a tasks screen; it takes no room when empty.
struct DecisionsDeckView: View {
    /// Show the Factory approvals under the questions.
    var showsApprovals = true
    /// Draw the "Nothing is waiting" line instead of collapsing to nothing
    /// (for a screen of its own).
    var showsEmptyState = false

    @ObservedObject private var store = DecisionsStore.shared
    @ObservedObject private var asks = AgentAskStore.shared
    @State private var index = 0
    @State private var collapsed = false
    @State private var showsLater = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let items = store.visibleQuestions
        let approvals = showsApprovals ? store.approvals : []
        VStack(alignment: .leading, spacing: 14) {
            if !items.isEmpty {
                questionsDeck(items)
            } else if let error = store.questionsError, approvals.isEmpty {
                errorRow(error)
            }

            if !approvals.isEmpty {
                approvalsSection(approvals)
            } else if showsApprovals, let error = store.approvalsError, showsEmptyState {
                errorRow(error)
            }

            if showsEmptyState, items.isEmpty, approvals.isEmpty, store.questionsError == nil {
                if store.hasLoaded {
                    VStack(spacing: 8) {
                        Image(systemName: "checkmark.circle")
                            .font(.title2)
                        Text("Nothing is waiting for you.")
                            .font(.subheadline)
                        Text("When an agent asks a question or a Factory step needs approval, it shows here.")
                            .font(.footnote)
                            .multilineTextAlignment(.center)
                    }
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 40)
                } else {
                    ProgressView()
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                }
            }
        }
        .onAppear { store.startWatching() }
        .onDisappear { store.stopWatching() }
    }

    // MARK: - Questions

    private func questionsDeck(_ items: [AgentQuestionRequest]) -> some View {
        let at = min(index, max(0, items.count - 1))
        let current = items[at]
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "arrow.triangle.branch")
                    .font(.footnote.weight(.bold))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Decisions")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(items.count) waiting for you")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                Spacer(minLength: 0)
                if items.count > 1, !collapsed {
                    HStack(spacing: 0) {
                        Text("\(at + 1) of \(items.count)")
                            .font(.footnote.monospacedDigit())
                            .foregroundColor(Color("TextSecondary"))
                        pagerButton("chevron.left", label: "Previous decision", disabled: at == 0) { index = at - 1 }
                        pagerButton("chevron.right", label: "Next decision", disabled: at >= items.count - 1) { index = at + 1 }
                    }
                }
                Button {
                    withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.2)) { collapsed.toggle() }
                } label: {
                    Image(systemName: "chevron.down")
                        .font(.footnote.weight(.semibold))
                        .rotationEffect(.degrees(collapsed ? -90 : 0))
                        .frame(width: 36, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundColor(Color("TextSecondary"))
                .accessibilityLabel(collapsed ? "Expand decisions" : "Collapse decisions")
            }
            if !collapsed {
                questionBody(current, at: at)
            }
        }
    }

    @ViewBuilder
    private func questionBody(_ current: AgentQuestionRequest, at: Int) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            QuestionCardView(request: current, agentName: store.agentNames[current.sessionID]) {
                // Let "Sent" read before the next one slides in.
                Task {
                    try? await Task.sleep(nanoseconds: 700_000_000)
                    withAnimation(.easeInOut(duration: 0.25)) {
                        index = min(at, max(0, store.visibleQuestions.count - 1))
                    }
                }
            }
            .id(current.id)
            .transition(.asymmetric(insertion: .move(edge: .trailing).combined(with: .opacity), removal: .opacity))
            laterRow(current)
        }
    }

    /// Done / Later under the card. Later opens a short row of lengths (minutes).
    private func laterRow(_ current: AgentQuestionRequest) -> some View {
        VStack(alignment: .trailing, spacing: 8) {
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                Button("Later") {
                    withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.2)) { showsLater.toggle() }
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                .accessibilityLabel("Later: choose when this comes back")
                Button("Done") { dismissCurrent(current) { store.markDone(current.id) } }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                    .accessibilityLabel("Done: hide this decision from the deck")
            }
            if showsLater {
                HStack(spacing: 8) {
                    ForEach([(15, "15 min"), (60, "1 hour"), (180, "3 hours")], id: \.0) { minutes, label in
                        Button(label) { dismissCurrent(current) { store.snooze(current.id, minutes: minutes) } }
                            .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                    }
                }
                .transition(.opacity)
            }
        }
    }

    private func dismissCurrent(_ current: AgentQuestionRequest, _ apply: () -> Void) {
        Haptics.success()
        showsLater = false
        withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.25)) {
            apply()
            index = min(index, max(0, store.visibleQuestions.count - 1))
        }
    }

    private func pagerButton(_ symbol: String, label: String, disabled: Bool, action: @escaping () -> Void) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.2)) { action() }
        } label: {
            Image(systemName: symbol)
                .font(.footnote.weight(.semibold))
                .frame(width: 36, height: 36)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundColor(disabled ? Color("TextTertiary") : Color("TextSecondary"))
        .disabled(disabled)
        .accessibilityLabel(label)
    }

    // MARK: - Factory approvals

    private func approvalsSection(_ approvals: [FactoryApprovalsClient.Approval]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.shield")
                    .font(.footnote.weight(.bold))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Approvals")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(approvals.count) waiting")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            ForEach(approvals) { approval in
                FactoryApprovalCard(approval: approval)
            }
        }
    }

    private func errorRow(_ message: String) -> some View {
        HStack(spacing: 12) {
            Text(message)
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("Retry") { store.retry() }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
        }
    }
}

/// One Factory approval: what the step wants to do, its risk, Approve / Reject.
struct FactoryApprovalCard: View {
    let approval: FactoryApprovalsClient.Approval
    @ObservedObject private var store = DecisionsStore.shared
    @State private var busy: String? = nil
    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Text(approval.title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                if approval.isHighRisk {
                    Text("High risk")
                        .font(.caption2.weight(.semibold))
                        .foregroundColor(.red)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Color.red.opacity(0.12)))
                }
            }
            if !approval.summary.isEmpty {
                Text(approval.summary)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if approval.isHighRisk, let reason = approval.riskReason, !reason.isEmpty {
                Text(reason)
                    .font(.caption)
                    .foregroundColor(.orange)
            }
            Text("Node \(approval.nodeId)")
                .font(.caption2)
                .foregroundColor(Color("TextTertiary"))
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                decisionButton("Reject", decision: "reject", kind: .quiet)
                decisionButton("Approve", decision: "approve", kind: .primary)
            }
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .stroke(approval.isHighRisk ? Color.red.opacity(0.35) : Color("BorderSubtle"), lineWidth: 1)
        )
    }

    private func decisionButton(_ title: String, decision: String, kind: QuestionCardButtonStyle.Kind) -> some View {
        Button {
            busy = decision
            error = nil
            Task {
                do {
                    try await store.resolve(approval, decision: decision)
                    Haptics.success()
                } catch {
                    self.error = FriendlyErrorMessage.from(error.localizedDescription)
                    Haptics.error()
                }
                busy = nil
            }
        } label: {
            if busy == decision {
                ProgressView().controlSize(.small)
            } else {
                Text(title)
            }
        }
        .buttonStyle(QuestionCardButtonStyle(kind: kind))
        .disabled(busy != nil)
        .accessibilityLabel("\(title): \(approval.title)")
    }
}

/// A screen of its own for the drawer / a notification tap: every decision
/// and approval waiting for the person.
struct DecisionsScreen: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                DecisionsDeckView(showsApprovals: true, showsEmptyState: true)
                    .padding(16)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Decisions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
    }
}
