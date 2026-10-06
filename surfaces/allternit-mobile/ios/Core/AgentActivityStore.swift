import SwiftUI

/// Agent Activity state: the fetched thread list (`GET factory/mail/threads`)
/// plus a ledger-tail-derived, read-only heuristic for review/guard/
/// reservation visibility per thread — the same substring-match-on-
/// `event_type` approach the web phase already proved out
/// (docs/AGENT_ACTIVITY_WEB_PHASE_1_NOTES.md, "What reservation/guard data
/// actually looked like"). On failure the store keeps whatever it last had
/// and exposes `loadError` so views render an error state instead of
/// spinning forever (CronJobStore's convention).
@MainActor
final class AgentActivityStore: ObservableObject {
    static let shared = AgentActivityStore()

    @Published private(set) var threads: [AgentActivityThreadSummary] = []
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String? = nil
    /// Ledger-derived signal, keyed by `threadId`. Absent entries mean
    /// "nothing found" — not "unknown" — so callers default to
    /// `AgentActivityThreadState()` via `state(for:)`.
    @Published private(set) var threadStates: [String: AgentActivityThreadState] = [:]
    /// The same windowed (last 4 relevant) ledger events `threadStates` was
    /// derived from, kept around so `AgentActivityDetailView` can render its
    /// read-only guard/reservation cards from real event data instead of a
    /// second network round-trip.
    @Published private(set) var relevantLedgerEvents: [String: [LedgerEvent]] = [:]

    /// Threads whose detail view has been opened at least once this app
    /// session — the "unread" signal. There is no server-side ack-per-device
    /// concept in this backend contract for iOS to read back (the web phase
    /// had a real `POST /mail/ack` per message; that endpoint isn't part of
    /// this phase's confirmed surface), so this is deliberately local and
    /// in-memory only — it resets on relaunch rather than inventing a fake
    /// ack call.
    @Published private(set) var viewedThreadIds: Set<String> = []

    private let client: AgentActivityClient
    private var fetchTask: Task<Void, Never>? = nil

    /// How many of the most recent ledger events to scan for
    /// review/guard/reservation signal across ALL threads in one batch call.
    private let ledgerTailBatchSize = 200
    /// Web's own constraint, kept exactly: only a thread's last 4 *relevant*
    /// ledger events (after filtering to that thread) feed the heuristic
    /// (AGENT_ACTIVITY_WEB_PHASE_1_NOTES.md's `useMonitorData` window).
    private let relevantEventWindow = 4

    init(client: AgentActivityClient = AgentActivityClient()) {
        self.client = client
    }

    func isUnread(_ threadId: String) -> Bool {
        !viewedThreadIds.contains(threadId)
    }

    /// Marks a thread as seen — called when its detail view appears
    /// ("I opened the thread" is the natural local signal for "I've read
    /// this," mirroring the web phase's own reasoning for its real ack).
    func markViewed(_ threadId: String) {
        viewedThreadIds.insert(threadId)
    }

    func state(for threadId: String) -> AgentActivityThreadState {
        threadStates[threadId] ?? AgentActivityThreadState()
    }

    /// Raw guard/reservation-relevant ledger events for one thread (most
    /// recent last), for `AgentActivityDetailView`'s read-only cards.
    func relevantEvents(for threadId: String) -> [LedgerEvent] {
        relevantLedgerEvents[threadId] ?? []
    }

    // MARK: - Thread detail (pass-through, AutomationTaskDetailView's
    // `jobStore.listRuns` convention — views never touch clients directly)

    func getThreadMessages(threadId: String) async throws -> [AgentActivityMessage] {
        try await client.getThreadMessages(threadId: threadId)
    }

    func send(threadId: String, body: String) async throws {
        try await client.send(threadId: threadId, body: body)
    }

    /// Decides a pending review, then resyncs the ledger-derived heuristic so
    /// the review tag/card clears once the new `decide` event is visible.
    func decide(threadId: String, approve: Bool) async throws {
        try await client.decide(threadId: threadId, approve: approve)
        await refresh()
    }

    // MARK: - Fetch

    /// Fetches the thread list once per launch unless forced; concurrent
    /// callers share the in-flight request (CronJobStore's
    /// `fetchJobsIfNeeded` pattern).
    func fetchIfNeeded(force: Bool = false) {
        guard force || threads.isEmpty, fetchTask == nil else { return }
        isLoading = true
        loadError = nil
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer {
                self.isLoading = false
                self.fetchTask = nil
            }
            await self.load()
        }
    }

    /// Unconditional refresh (pull-to-refresh, post-mutation resync).
    func refresh() async {
        loadError = nil
        await load()
    }

    private func load() async {
        do {
            threads = try await client.listThreads()
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
            return
        } catch {
            loadError = error.localizedDescription
            return
        }

        // The ledger tail is read-only, best-effort decoration on top of the
        // real thread list — a failure here shouldn't block the thread list
        // itself from showing (unlike the thread fetch above).
        if let ledgerEvents = try? await client.tailLedger(count: ledgerTailBatchSize) {
            let (states, windows) = Self.deriveThreadStates(
                threadIds: threads.map(\.threadId),
                ledgerEvents: ledgerEvents,
                window: relevantEventWindow
            )
            threadStates = states
            relevantLedgerEvents = windows
        }
    }

    // MARK: - Heuristic derivation

    /// Substring match on `event_type` against /guard/i, /reserve/i,
    /// review-request, and decision — there is no documented `event_type`
    /// vocabulary, so this is read-only best-effort surfacing, not a
    /// guaranteed status field (identical caveat to the web phase's own
    /// heuristic). "Pending review" mirrors the web phase's derivation: a
    /// review is open if the most recent review-request-matching event in the
    /// window postdates the most recent decision-matching event (or there is
    /// no decision event at all).
    ///
    /// The backend emits `ReviewDecision` for a resolution; it contains the
    /// substring "review" but is a decision, so it must be excluded from
    /// review-request matching and included in decision matching.
    private static func isDecisionEvent(_ type: String) -> Bool {
        type.range(of: "decide", options: .caseInsensitive) != nil || type == "ReviewDecision"
    }

    private static func isReviewRequestEvent(_ type: String) -> Bool {
        type.range(of: "review", options: .caseInsensitive) != nil && !isDecisionEvent(type)
    }

    private static func deriveThreadStates(
        threadIds: [String],
        ledgerEvents: [LedgerEvent],
        window: Int
    ) -> (states: [String: AgentActivityThreadState], windows: [String: [LedgerEvent]]) {
        var statesByThread: [String: AgentActivityThreadState] = [:]
        var windowsByThread: [String: [LedgerEvent]] = [:]
        for threadId in threadIds {
            let relevant = ledgerEvents.filter { $0.relatedThreadId == threadId }
            let windowed = Array(relevant.suffix(window))
            guard !windowed.isEmpty else { continue }
            windowsByThread[threadId] = windowed

            var state = AgentActivityThreadState()
            var lastReviewIndex: Int? = nil
            var lastDecideIndex: Int? = nil
            for (index, event) in windowed.enumerated() {
                let type = event.eventType
                if type.range(of: "guard", options: .caseInsensitive) != nil {
                    state.hasGuardActivity = true
                }
                if type.range(of: "reserve", options: .caseInsensitive) != nil {
                    state.hasReservationActivity = true
                }
                if isReviewRequestEvent(type) {
                    lastReviewIndex = index
                }
                if isDecisionEvent(type) {
                    lastDecideIndex = index
                }
            }
            if let lastReviewIndex {
                state.hasPendingReview = lastDecideIndex.map { $0 < lastReviewIndex } ?? true
            }
            statesByThread[threadId] = state
        }
        return (statesByThread, windowsByThread)
    }
}

/// Read-only, ledger-tail-derived signal for one thread — best-effort
/// surfacing, not a guaranteed status field (see `AgentActivityStore`'s
/// derivation doc comment and `LedgerEvent`'s doc comment for why).
struct AgentActivityThreadState: Sendable, Equatable {
    var hasPendingReview = false
    var hasGuardActivity = false
    var hasReservationActivity = false
}
