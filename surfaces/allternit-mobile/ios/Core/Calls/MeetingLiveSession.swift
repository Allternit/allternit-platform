import Foundation
import SwiftUI

/// One meeting session on the live page: the session and meeting, the
/// transcript, artifacts and pending approvals, kept current by the SSE
/// stream (`GET /meeting-sessions/:sid/events`, CONTRACT §5). It reconnects
/// with `Last-Event-ID` and falls back to polling the REST reads when the
/// stream can't be held (sample mode, proxies that buffer SSE).
@MainActor
final class MeetingLiveSession: ObservableObject {
    enum Phase: Equatable {
        case loading, ready
        case failed(String)
        case paidPlan
        case signedOut
    }

    let sessionId: String
    @Published private(set) var phase: Phase = .loading
    @Published private(set) var session: MeetingSession?
    @Published private(set) var meeting: Meeting?
    @Published private(set) var participants: [MeetingParticipant] = []
    @Published private(set) var transcript: [TranscriptSegment] = []
    @Published private(set) var artifacts: [MeetingArtifact] = []
    @Published private(set) var approvals: [ToolApproval] = []
    /// Gizzi's last reply (chat or voice), shown under the mascot.
    @Published private(set) var lastReply: String?
    @Published private(set) var busyAction: String?
    @Published var actionError: String?

    private var lastEventId: String?
    private var streamTask: Task<Void, Never>?
    private let client = MeetingsClient.shared

    init(sessionId: String, meeting: Meeting? = nil) {
        self.sessionId = sessionId
        self.meeting = meeting
    }

    var status: GizziStatus { session?.status ?? meeting?.status ?? .joining }
    var isEnded: Bool { status == .ended || status == .notAdmitted }

    // MARK: - Lifecycle

    func start() {
        guard streamTask == nil else { return }
        streamTask = Task { [weak self] in
            await self?.loadAll()
            await self?.runStream()
        }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
    }

    func loadAll() async {
        do {
            let detail = try await client.session(sessionId)
            apply(detail)
            async let segs = client.transcript(sessionId)
            async let arts = client.artifacts(sessionId)
            let (t, a) = try await (segs, arts)
            merge(segments: t.segments)
            artifacts = a
            phase = .ready
        } catch let error as MeetingsError {
            switch error {
            case .paidPlanRequired: phase = .paidPlan
            case .signedOut: phase = .signedOut
            default: if session == nil { phase = .failed(error.localizedDescription) }
            }
        } catch {
            if session == nil { phase = .failed(FriendlyErrorMessage.from(error.localizedDescription)) }
        }
    }

    private func apply(_ detail: MeetingSessionDetail) {
        session = detail.session
        meeting = detail.meeting
        participants = detail.participants ?? participants
        approvals = (detail.pendingApprovals ?? []).filter { $0.status == "pending" }
        MeetingsStore.shared.didUpdate(session: detail.session, meeting: detail.meeting)
    }

    private func runStream() async {
        var failures = 0
        while !Task.isCancelled && !isEnded {
            if SampleWorkspace.isEnabled || failures >= 3 {
                // Poll instead: every 4 s while live.
                try? await Task.sleep(for: .seconds(4))
                if Task.isCancelled { return }
                await loadAll()
                continue
            }
            do {
                for try await frame in client.events(sessionId, lastEventId: lastEventId) {
                    failures = 0
                    if let id = frame.id { lastEventId = id }
                    if let event = MeetingEvent.decode(type: frame.type, data: frame.data) { handle(event) }
                    if isEnded { break }
                }
            } catch let error as MeetingsError where error == .paidPlanRequired || error == .signedOut {
                phase = error == .paidPlanRequired ? .paidPlan : .signedOut
                return
            } catch {
                failures += 1
            }
            if Task.isCancelled || isEnded { break }
            // Back off, then catch up on anything the gap missed.
            try? await Task.sleep(for: .seconds(min(15, 1 << failures)))
            await loadAll()
        }
        // After the end, keep checking briefly for the summary.
        for _ in 0..<12 where session?.summary == nil && !Task.isCancelled {
            try? await Task.sleep(for: .seconds(10))
            await loadAll()
        }
    }

    private func handle(_ event: MeetingEvent) {
        switch event {
        case .state(let state, let gizzi, let endReason):
            guard var s = session else { return }
            s.state = state
            s.gizzi = gizzi
            s.endReason = endReason
            s.listening = state != "paused"
            if state == "live", s.liveAt == nil { s.liveAt = MeetingDates.iso(Date()) }
            session = s
            MeetingsStore.shared.didUpdate(session: s, meeting: meeting)
        case .participant(let p, let change):
            participants.removeAll { $0.id == p.id }
            if change != "left" { participants.append(p) }
        case .transcript(let segments):
            merge(segments: segments)
        case .reply(_, let text):
            lastReply = text
        case .approvalRequested(let a):
            approvals.removeAll { $0.id == a.id }
            approvals.append(a)
        case .approvalResolved(let a):
            approvals.removeAll { $0.id == a.id }
        case .artifact(let a):
            if let i = artifacts.firstIndex(where: { $0.id == a.id }) { artifacts[i] = a } else { artifacts.append(a) }
        case .ended(let reason):
            guard var s = session else { return }
            s.state = "ended"
            s.endReason = reason
            s.gizzi = ["not_admitted", "guest_blocked"].contains(reason) ? "not_admitted" : "ended"
            s.endedAt = MeetingDates.iso(Date())
            session = s
            MeetingsStore.shared.didUpdate(session: s, meeting: meeting)
        case .summary(let summary, let decisions, let items):
            guard var s = session else { return }
            s.summary = summary
            s.decisions = decisions
            s.actionItems = items
            session = s
            MeetingsStore.shared.didUpdate(session: s, meeting: meeting)
        }
    }

    private func merge(segments: [TranscriptSegment]) {
        guard !segments.isEmpty else { return }
        var bySeq = Dictionary(transcript.map { ($0.seq, $0) }, uniquingKeysWith: { _, new in new })
        for s in segments { bySeq[s.seq] = s }
        transcript = bySeq.values.sorted { $0.seq < $1.seq }
    }

    // MARK: - Controls

    /// Stop listening ↔ resume.
    func toggleListening() async {
        let action = status == .paused ? "resume" : "stop_listening"
        await run(action) { try await self.client.control(self.sessionId, action: action) }
    }

    func setSpeakReplies(_ on: Bool) async {
        await run("speak") { try await self.client.setSpeakReplies(self.sessionId, on: on) }
    }

    func leave() async {
        await run("leave") { try await self.client.control(self.sessionId, action: "leave") }
    }

    private func run(_ name: String, _ body: @escaping () async throws -> MeetingSession) async {
        guard busyAction == nil else { return }
        busyAction = name
        defer { busyAction = nil }
        do {
            let updated = try await body()
            session = updated
            MeetingsStore.shared.didUpdate(session: updated, meeting: meeting)
        } catch {
            actionError = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    func decide(_ approval: ToolApproval, approve: Bool) async {
        do {
            _ = try await client.decide(sessionId, approvalId: approval.id, approve: approve)
            approvals.removeAll { $0.id == approval.id }
        } catch {
            actionError = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
