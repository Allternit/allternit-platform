import Foundation
import UIKit

/// Incoming and in-progress Allternit calls — the native port of the web's
/// `InappCallLayer` + `src/lib/gateway/inapp-calls.ts`.
///
/// - While the app is open and signed in it polls `GET /api/v1/phone/incoming`
///   every 4 s (each poll is also the presence heartbeat that lets the cloud
///   ring this phone instead of filing an instant missed call). Signed out or
///   a server without the routes backs off to once a minute.
/// - A ring push tap / `allternit_call` link asks for rings at once
///   (`showRing(room:)`); a ring that already ended falls back to its thread.
/// - Answer: mic permission → `POST …/answer` → LiveKit room join → live.
///   Decline: `POST …/decline`. Hang up leaves the room (an answered call has
///   no server hang-up route; the other side sees us leave the room).
/// - A ring that disappears without us acting is looked up in the thread
///   log: `call.missed` / `call.cancelled` → "Missed call"; answered on
///   another device → it just goes away.
@MainActor
final class InappCallStore: ObservableObject {
    static let shared = InappCallStore()

    typealias IncomingCall = InappCallsClient.IncomingCall

    enum Phase: Equatable {
        case connecting
        case live
        /// Call over; `note` says why.
        case ended
        /// A ring nobody answered here.
        case missed
        /// The mic is off for Allternit in iOS Settings.
        case micDenied
        /// This build can't carry call audio (no LiveKit SDK linked).
        case unavailable
    }

    struct ActiveCall: Equatable {
        let room: String
        let threadId: String
        let botId: String
        let userId: String
        let label: String
        var phase: Phase
        var muted = false
        var speaker = false
        var startedAt: Date?
        var note: String?
    }

    /// Rings waiting for me that I haven't answered or declined.
    @Published private(set) var incoming: [IncomingCall] = []
    @Published private(set) var active: ActiveCall?

    /// The ring on screen (newest first, like the web's `incoming[0]`).
    var ringing: IncomingCall? { active == nil ? incoming.first : nil }

    private let api = InappCallsClient.shared
    private let ringtone = CallRingtone()
    private var session: CallAudioSession?
    private var pollTask: Task<Void, Never>?
    private var isForeground = true
    /// Rings acted on here (declined, answered, mic denied) so the next poll
    /// doesn't bring them back while the server catches up.
    private var handledRooms: Set<String> = []

    static let pollInterval: UInt64 = 4_000_000_000
    static let backoffInterval: UInt64 = 60_000_000_000

    private init() {}

    // MARK: - Lifecycle

    /// The workspace appeared (signed in): start listening for rings.
    func start() {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                let wait = await self?.tick() ?? InappCallStore.backoffInterval
                try? await Task.sleep(nanoseconds: wait)
            }
        }
    }

    /// Signed out / workspace gone: stop polling and drop any call.
    func stop() {
        pollTask?.cancel()
        pollTask = nil
        incoming = []
        handledRooms = []
        if active != nil { endLocally(note: nil) }
        active = nil
        syncRing()
    }

    /// Scene phase: poll only in front (a live call keeps going).
    func setForeground(_ foreground: Bool) {
        isForeground = foreground
        if foreground { Task { await refreshNow() } } else { ringtone.stop() }
    }

    private func tick() async -> UInt64 {
        guard isForeground, AuthManager.shared.isSignedIn else { return Self.pollInterval }
        do {
            apply(try await api.incoming())
            return Self.pollInterval
        } catch let error as InappCallsClient.CallError where error.isBackoff {
            return Self.backoffInterval
        } catch {
            return Self.pollInterval
        }
    }

    func refreshNow() async {
        guard AuthManager.shared.isSignedIn, let calls = try? await api.incoming() else { return }
        apply(calls)
    }

    private func apply(_ calls: [IncomingCall]) {
        let fresh = calls.filter { !handledRooms.contains($0.room) }
        let shown = ringing
        // Keep a ring only while nothing else is happening on this phone.
        incoming = (active == nil || active?.phase == .missed || active?.phase == .ended) ? fresh : []
        if let shown, !calls.contains(where: { $0.room == shown.room }), !handledRooms.contains(shown.room) {
            Task { await resolveVanished(shown) }
        }
        handledRooms.formIntersection(Set(calls.map(\.room)))
        syncRing()
    }

    /// A ring left the list without us acting: missed, or answered elsewhere?
    private func resolveVanished(_ call: IncomingCall) async {
        let events = (try? await api.events(threadId: call.threadId)) ?? []
        let last = events.last(where: { $0.room == call.room })?.kind
        guard last == nil || last == "call.missed" || last == "call.cancelled" else { return }
        guard active == nil else { return }
        active = ActiveCall(room: call.room, threadId: call.threadId, botId: call.botId,
                            userId: call.from.userId, label: call.from.label, phase: .missed)
        syncRing()
    }

    private func syncRing() {
        if ringing != nil, isForeground { ringtone.start() } else { ringtone.stop() }
    }

    // MARK: - Links + pushes

    /// A ring push / `allternit_call` link: true when that ring is still live
    /// (the incoming screen is now up), false when it already ended.
    func showRing(room: String) async -> Bool {
        if incoming.contains(where: { $0.room == room }) || active?.room == room { return true }
        guard let calls = try? await api.incoming() else { return false }
        handledRooms.remove(room)
        apply(calls)
        return calls.contains(where: { $0.room == room })
    }

    /// A push landed while the app is in front. A ring is shown by the call
    /// screen, so its banner is skipped; a missed call we already show too.
    func handlesForegroundPush(type: String?, room: String?) -> Bool {
        switch type {
        case "call":
            Task { await refreshNow() }
            return true
        case "missed_call":
            return room != nil && active?.room == room && active?.phase == .missed
        default:
            return false
        }
    }

    // MARK: - Actions

    func answer(_ call: IncomingCall) async {
        incoming.removeAll { $0.room == call.room }
        handledRooms.insert(call.room)
        var next = ActiveCall(room: call.room, threadId: call.threadId, botId: call.botId,
                              userId: call.from.userId, label: call.from.label, phase: .connecting)
        guard CallAudio.isAvailable else {
            // Don't tell the server we answered: the caller would hear silence.
            next.phase = .unavailable
            active = next
            syncRing()
            return
        }
        active = next
        syncRing()
        guard await CallAudioSessionConfig.microphoneAllowed() else {
            active?.phase = .micDenied
            return
        }
        do {
            let grant = try await api.answer(userId: call.from.userId, room: call.room)
            guard active?.room == call.room else {
                // Hung up while it was connecting: don't leave them ringing into silence.
                return
            }
            let result = await CallAudio.join(url: grant.url, token: grant.token) { [weak self] in
                self?.remoteEnded(room: call.room)
            }
            switch result {
            case .joined(let joined):
                guard active?.room == call.room, active?.phase == .connecting else { joined.leave(); return }
                session = joined
                active?.phase = .live
                active?.startedAt = Date()
            case .failed:
                fail("Couldn't connect audio. Check your connection and try again.")
            }
        } catch {
            fail((error as? LocalizedError)?.errorDescription ?? "Couldn't answer the call.")
        }
    }

    func decline(_ call: IncomingCall) {
        incoming.removeAll { $0.room == call.room }
        handledRooms.insert(call.room)
        syncRing()
        Task { try? await api.decline(userId: call.from.userId, room: call.room) }
    }

    /// Mic denied / no audio here: turn the ring down on the server too.
    func declineActive() {
        guard let call = active else { return }
        active = nil
        Task { try? await api.decline(userId: call.userId, room: call.room) }
    }

    func hangUp() {
        guard active != nil else { return }
        endLocally(note: nil)
        active = nil
        syncRing()
    }

    func setMuted(_ muted: Bool) {
        session?.setMuted(muted)
        active?.muted = muted
    }

    func setSpeaker(_ on: Bool) {
        if let session { session.setSpeaker(on) } else { CallAudioSessionConfig.setSpeaker(on) }
        active?.speaker = on
    }

    /// Close the ended / missed / denied screen.
    func dismiss() {
        guard let phase = active?.phase, phase != .connecting, phase != .live else { return }
        active = nil
        syncRing()
    }

    /// Ended or missed → the conversation the call belongs to.
    func openConversation() {
        guard let call = active else { return }
        active = nil
        syncRing()
        guard !call.botId.isEmpty, !call.threadId.isEmpty else { return }
        DeepLinkRouter.shared.enqueue(.thread(botId: call.botId, threadId: call.threadId))
    }

    func openSystemSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }

    // MARK: - Ending

    private func remoteEnded(room: String) {
        guard active?.room == room, active?.phase == .live || active?.phase == .connecting else { return }
        endLocally(note: "Call ended.")
    }

    private func fail(_ note: String) {
        endLocally(note: note)
    }

    private func endLocally(note: String?) {
        session?.leave()
        session = nil
        active?.phase = .ended
        active?.note = note
    }
}
