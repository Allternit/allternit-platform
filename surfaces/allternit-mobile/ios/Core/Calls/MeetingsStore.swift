import ActivityKit
import AuthenticationServices
import Foundation
import SwiftUI
import UIKit
import UserNotifications

/// The Meetings section's state (lane 2.11): the `meetings` account flag,
/// the agenda (live call on top, upcoming, past with recaps), the user's
/// meeting settings and calendar connections. It also keeps the meeting
/// Live Activity and the local state notifications in step with what it
/// sees, while the app runs.
///
/// Free plans get `402 meetings_requires_paid_plan` on every route except
/// `GET /me/meeting-settings` and the calendar OAuth routes (CONTRACT §1),
/// so a 402 on the agenda puts the whole section in its plan state.
@MainActor
final class MeetingsStore: ObservableObject {
    static let shared = MeetingsStore()

    enum Phase: Equatable {
        case idle, loading, loaded
        case failed(String)
        case signedOut
        case paidPlan
        case forbidden(String)
    }

    /// The `meetings` flag from `GET /me/flags` (cached so the entry shows
    /// at once on the next launch; a failed read keeps the cached value).
    @Published private(set) var isEnabled: Bool
    @Published private(set) var phase: Phase = .idle
    /// Meetings Gizzi is in now (launching … paused), newest first.
    @Published private(set) var live: [Meeting] = []
    @Published private(set) var upcoming: [Meeting] = []
    @Published private(set) var past: [Meeting] = []
    /// Latest session per meeting id, for live timers and recap labels.
    @Published private(set) var sessions: [String: MeetingSession] = [:]
    @Published private(set) var settings: UserMeetingSettings?
    @Published private(set) var connections: [CalendarConnection] = []
    @Published private(set) var connectionsLoaded = false
    @Published private(set) var usage: MeetingUsage?
    /// nil until read; Zoom calls need it (`409 zoom_not_connected`).
    @Published private(set) var zoomConnected: Bool?
    /// After the first calendar connects, the auto-join rule picker shows inline.
    @Published var pickRule = false
    @Published var actionError: String?
    @Published private(set) var pendingSwitches: Set<String> = []

    var activeMeeting: Meeting? { live.first }

    private static let flagKey = "allternit.meetings.flag"
    private static let seenKey = "allternit.meetings.seenStates"
    private var seenStates: [String: String]
    private var awaitingRecap: [String: Date] = [:]
    private var watchTask: Task<Void, Never>?
    private var loadingTask: Task<Void, Never>?

    private let client = MeetingsClient.shared

    private init() {
        isEnabled = UserDefaults.standard.bool(forKey: Self.flagKey)
        seenStates = (UserDefaults.standard.dictionary(forKey: Self.seenKey) as? [String: String]) ?? [:]
        #if DEBUG
        if SampleWorkspace.isEnabled || launchArgumentEnabled("meetings") { isEnabled = true }
        #endif
    }

    // MARK: - Flag

    func refreshFlag() async {
        #if DEBUG
        if SampleWorkspace.isEnabled || launchArgumentEnabled("meetings") { isEnabled = true; return }
        #endif
        guard AuthManager.shared.isSignedIn else {
            setFlag(false)
            return
        }
        do {
            setFlag(try await AccountFlagsClient.shared.string("meetings") != nil)
        } catch {
            // Unreachable: keep what we had.
        }
    }

    private func setFlag(_ on: Bool) {
        isEnabled = on
        UserDefaults.standard.set(on, forKey: Self.flagKey)
        if !on { MeetingLiveActivityManager.shared.endAll() }
    }

    // MARK: - Agenda

    private var lastLoadedAt: Date = .distantPast

    /// The drawer's light refresh: flag, then the agenda at most once a minute
    /// (keeps the live dot and the Live Activity honest without a poll).
    func refreshIfStale() async {
        await refreshFlag()
        guard isEnabled, Date().timeIntervalSince(lastLoadedAt) > 60 else { return }
        await load(quiet: true)
    }

    /// Loads the agenda, settings and calendars. `quiet` keeps the current
    /// rows on screen (pull to refresh, background polls).
    func load(quiet: Bool = false) async {
        if let loadingTask { return await loadingTask.value }
        let task = Task { await performLoad(quiet: quiet) }
        loadingTask = task
        await task.value
        loadingTask = nil
    }

    private func performLoad(quiet: Bool) async {
        guard isEnabled else { return }
        lastLoadedAt = Date()
        guard AuthManager.shared.isSignedIn else {
            phase = .signedOut
            clearRows()
            return
        }
        if !quiet || phase != .loaded { phase = .loading }

        async let settingsLoad: Void = loadSettings()
        async let calendarsLoad: Void = loadConnections()
        do {
            async let nowRows = client.list(range: "now")
            async let upcomingRows = client.list(range: "upcoming")
            async let pastRows = client.list(range: "past", limit: 30)
            let (now, next, done) = try await (nowRows, upcomingRows, pastRows)
            apply(now: now, upcoming: next, past: done)
            phase = .loaded
            await loadSessions()
        } catch let error as MeetingsError {
            switch error {
            case .signedOut: phase = .signedOut; clearRows()
            case .paidPlanRequired: phase = .paidPlan; clearRows()
            case .forbidden(let message): phase = .forbidden(message); clearRows()
            default:
                // Keep rows already shown on a failed background refresh.
                if !(quiet && phase == .loaded) { phase = .failed(error.localizedDescription) }
            }
        } catch {
            if !(quiet && phase == .loaded) { phase = .failed(FriendlyErrorMessage.from(error.localizedDescription)) }
        }
        _ = await (settingsLoad, calendarsLoad)
        syncSystemSurfaces()
        ensureWatching()
    }

    private func clearRows() {
        live = []; upcoming = []; past = []; sessions = [:]
        MeetingLiveActivityManager.shared.endAll()
    }

    private func apply(now: [Meeting], upcoming next: [Meeting], past done: [Meeting]) {
        var seen = Set<String>()
        func unique(_ rows: [Meeting]) -> [Meeting] { rows.filter { seen.insert($0.id).inserted } }
        let all = unique(now + next)
        live = all.filter { $0.status.isActive }
        upcoming = all.filter { !$0.status.isActive && $0.status != .ended && $0.status != .notAdmitted }
            .sorted { ($0.startDate ?? .distantFuture) < ($1.startDate ?? .distantFuture) }
        // Calls that just ended still show today: they belong with the past.
        let endedToday = all.filter { $0.status == .ended || $0.status == .notAdmitted }
        past = unique(endedToday + done)
            .sorted { ($0.startDate ?? .distantPast) > ($1.startDate ?? .distantPast) }
    }

    /// Sessions for live calls (timer, listening) and the newest past calls
    /// (Recap ready vs. the end reason). The list rows carry no summary.
    private func loadSessions() async {
        let wanted = (live + past.prefix(12)).compactMap { m in m.sessionId.map { (m.id, $0) } }
        await withTaskGroup(of: (String, MeetingSession?).self) { group in
            for (meetingId, sid) in wanted where shouldRefetch(meetingId) {
                group.addTask { [client] in (meetingId, try? await client.session(sid).session) }
            }
            for await (meetingId, session) in group {
                if let session { sessions[meetingId] = session }
            }
        }
    }

    private func shouldRefetch(_ meetingId: String) -> Bool {
        guard let known = sessions[meetingId] else { return true }
        // An ended session only changes once more: when its summary lands.
        return known.status.isActive || (known.status == .ended && known.summary == nil)
    }

    func session(for meeting: Meeting) -> MeetingSession? { sessions[meeting.id] }

    /// The live page pushes what it hears back here (SSE state, controls).
    func didUpdate(session: MeetingSession, meeting: Meeting?) {
        sessions[session.meetingId] = session
        func patch(_ rows: inout [Meeting]) {
            if let i = rows.firstIndex(where: { $0.id == session.meetingId }) { rows[i].gizzi = session.gizzi }
        }
        patch(&live); patch(&upcoming); patch(&past)
        if session.status.isActive, !live.contains(where: { $0.id == session.meetingId }), let meeting {
            var m = meeting
            m.gizzi = session.gizzi
            live.insert(m, at: 0)
            upcoming.removeAll { $0.id == m.id }
        } else if !session.status.isActive, let i = live.firstIndex(where: { $0.id == session.meetingId }) {
            let m = live.remove(at: i)
            past.insert(m, at: 0)
        }
        syncSystemSurfaces()
        ensureWatching()
    }

    // MARK: - Actions

    /// The per-meeting Gizzi switch (`PATCH auto_join`), optimistic.
    func setGizzi(_ meeting: Meeting, on: Bool) async {
        guard !pendingSwitches.contains(meeting.id) else { return }
        pendingSwitches.insert(meeting.id)
        defer { pendingSwitches.remove(meeting.id) }
        let before = upcoming
        if let i = upcoming.firstIndex(where: { $0.id == meeting.id }) {
            upcoming[i].autoJoin = on
            upcoming[i].gizzi = on ? GizziStatus.scheduled.rawValue : GizziStatus.off.rawValue
        }
        do {
            let updated = try await client.setAutoJoin(meetingId: meeting.id, on: on)
            if let i = upcoming.firstIndex(where: { $0.id == updated.id }) { upcoming[i] = updated }
        } catch {
            upcoming = before
            handle(error)
        }
    }

    /// A pasted Meet / Zoom / Teams link: saved, then Gizzi is sent now
    /// unless `joinNow` is false (the call is later).
    @discardableResult
    func addLink(_ link: String, joinNow: Bool) async throws -> (Meeting, MeetingSession?) {
        let meeting = try await client.create(url: link.trimmingCharacters(in: .whitespacesAndNewlines))
        var session: MeetingSession? = nil
        if joinNow {
            let started = try await client.startSession(meetingId: meeting.id, when: "now",
                                                        speakReplies: settings?.defaultReplyMode == "voice")
            session = started
            didUpdate(session: started, meeting: meeting)
        }
        await load(quiet: true)
        return (meeting, session)
    }

    /// "Send Gizzi now" on an upcoming row.
    func sendNow(_ meeting: Meeting) async -> MeetingSession? {
        do {
            let session = try await client.startSession(meetingId: meeting.id, when: "now",
                                                        speakReplies: settings?.defaultReplyMode == "voice")
            didUpdate(session: session, meeting: meeting)
            return session
        } catch {
            handle(error)
            return nil
        }
    }

    func handle(_ error: Error) {
        switch error as? MeetingsError {
        case .paidPlanRequired?: phase = .paidPlan
        case .signedOut?: phase = .signedOut
        default: actionError = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    // MARK: - Settings and calendars

    func loadSettings() async {
        guard AuthManager.shared.isSignedIn else { return }
        if let value = try? await client.settings() { settings = value }
        if let value = try? await client.usage() { usage = value }
    }

    func updateSettings(autoJoin: String? = nil, recapEmail: Bool? = nil, defaultReplyMode: String? = nil) async {
        let before = settings
        if var s = settings {
            if let autoJoin { s.autoJoin = autoJoin }
            if let recapEmail { s.recapEmail = recapEmail }
            if let defaultReplyMode { s.defaultReplyMode = defaultReplyMode }
            settings = s
        }
        do {
            settings = try await client.updateSettings(autoJoin: autoJoin, recapEmail: recapEmail, defaultReplyMode: defaultReplyMode)
        } catch {
            settings = before
            handle(error)
        }
    }

    func loadConnections() async {
        guard AuthManager.shared.isSignedIn else { return }
        do {
            connections = try await client.connections()
        } catch {
            // Keep the last list; the connect card still works.
        }
        if let zoom = try? await client.zoomConnection() { zoomConnected = zoom.connected && zoom.status != "needs_reauth" }
        connectionsLoaded = true
    }

    /// "Connect Zoom": the same consent-sheet flow as Google Calendar.
    func connectZoom() async {
        do {
            let consent = try await client.zoomConsentURL(returnTo: "allternit://meetings/zoom-connected")
            let callback = await MeetingsWebAuth.shared.open(consent, callbackScheme: "allternit")
            await loadConnections()
            let failed = callback.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }?
                .first { $0.name == "zoom" }?.value == "error"
            if failed { actionError = "Zoom didn't connect. Try again." }
        } catch {
            handle(error)
        }
    }

    func disconnectZoom() async {
        do {
            try await client.disconnectZoom()
            zoomConnected = false
        } catch {
            handle(error)
        }
    }

    /// Opens Google's consent page in the system web sign-in sheet. The
    /// callback 302s to `return_to`; with the app scheme the sheet closes by
    /// itself, otherwise closing it is the return. Either way the calendar
    /// list is reread and, for a first calendar, the rule picker opens.
    func connectGoogle() async {
        let hadAny = !connections.isEmpty
        do {
            let consent = try await client.googleConsentURL(returnTo: "allternit://meetings/calendar-connected")
            let callback = await MeetingsWebAuth.shared.open(consent, callbackScheme: "allternit")
            await loadConnections()
            let failed = callback.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems }?
                .first { $0.name == "calendar" }?.value == "error"
            if failed {
                actionError = "Google Calendar didn't connect. Try again."
            } else if !hadAny, !connections.isEmpty {
                pickRule = true
            }
        } catch {
            handle(error)
        }
    }

    func addCalendarLink(_ link: String) async throws {
        let hadAny = !connections.isEmpty
        let connection = try await client.addCalendarLink(link.trimmingCharacters(in: .whitespacesAndNewlines))
        connections.removeAll { $0.id == connection.id }
        connections.append(connection)
        if !hadAny { pickRule = true }
        await load(quiet: true)
    }

    func removeConnection(_ connection: CalendarConnection) async {
        let before = connections
        connections.removeAll { $0.id == connection.id }
        do { try await client.removeConnection(connection.id) } catch {
            connections = before
            handle(error)
        }
    }

    // MARK: - Watching (polls while a call is active or a recap is due)

    private func ensureWatching() {
        let needed = activeMeeting != nil || !awaitingRecap.isEmpty
        if !needed { watchTask?.cancel(); watchTask = nil; return }
        guard watchTask == nil else { return }
        watchTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                guard let self, !Task.isCancelled else { return }
                await self.load(quiet: true)
                if self.activeMeeting == nil && self.awaitingRecap.isEmpty { break }
            }
            self?.watchTask = nil
        }
    }

    // MARK: - Live Activity and notifications

    private func syncSystemSurfaces() {
        MeetingLiveActivityManager.shared.sync(live: live.map { ($0, sessions[$0.id]) })
        let now = Date()
        awaitingRecap = awaitingRecap.filter { now.timeIntervalSince($0.value) < 15 * 60 }
        for meeting in live + past.prefix(12) {
            let session = sessions[meeting.id]
            let key = Self.noticeKey(meeting: meeting, session: session)
            let previous = seenStates[meeting.id]
            guard previous != key else { continue }
            seenStates[meeting.id] = key
            if meeting.status == .ended, session?.summary == nil, previous != nil {
                awaitingRecap[meeting.id] = awaitingRecap[meeting.id] ?? now
            }
            if session?.summary != nil { awaitingRecap.removeValue(forKey: meeting.id) }
            // First sight of a meeting (fresh install, another device's call): no banner.
            guard previous != nil else { continue }
            MeetingNotifier.post(key: key, meeting: meeting)
        }
        // Forget meetings that dropped out of the agenda.
        let known = Set((live + upcoming + past).map(\.id))
        seenStates = seenStates.filter { known.contains($0.key) }
        UserDefaults.standard.set(seenStates, forKey: Self.seenKey)
    }

    /// What a notification would say about this meeting now.
    static func noticeKey(meeting: Meeting, session: MeetingSession?) -> String {
        switch meeting.status {
        case .launching, .joining: return "joining"
        case .waitingAdmit: return "waiting_admit"
        case .live, .paused: return "live"
        case .notAdmitted: return "not_admitted"
        case .ended: return session?.summary != nil ? "recap_ready" : "ended"
        case .scheduled, .off: return meeting.status.rawValue
        }
    }
}

// MARK: - Notifications

/// Local notifications for the four states the plan names, posted when the
/// app sees a change (foreground banners via AppDelegate.willPresent). Taps
/// carry `url: /meetings/<id>`, the same path a cloud push would use.
enum MeetingNotifier {
    static func post(key: String, meeting: Meeting) {
        let title: String
        let body: String
        switch key {
        case "joining":
            title = "Gizzi is joining"
            body = "\(meeting.displayTitle) · \(meeting.platformName)"
        case "waiting_admit":
            title = "Let Gizzi in"
            body = "Gizzi (Allternit) is waiting in the lobby of \(meeting.displayTitle)."
        case "not_admitted":
            title = "Gizzi wasn't let in"
            body = "Nobody admitted Gizzi to \(meeting.displayTitle)."
        case "recap_ready":
            title = "Recap ready"
            body = "\(meeting.displayTitle): summary, decisions and action items."
        default:
            return
        }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = key == "waiting_admit" ? .default : nil
        content.threadIdentifier = "meeting-\(meeting.id)"
        content.userInfo = ["url": "/meetings/\(meeting.id)", "type": "meeting.\(key)"]
        if key == "waiting_admit" { content.interruptionLevel = .timeSensitive }
        let request = UNNotificationRequest(identifier: "meeting-\(meeting.id)-\(key)", content: content, trigger: nil)
        Task {
            let center = UNUserNotificationCenter.current()
            let granted = await center.notificationSettings().authorizationStatus
            guard granted == .authorized || granted == .provisional || granted == .ephemeral else { return }
            try? await center.add(request)
        }
    }
}

// MARK: - Live Activity

/// One local Live Activity per meeting Gizzi is in (the widget extension's
/// `MeetingLiveActivityWidget`). cloud-api sends no pushes for it yet, so it
/// is requested without a push type and updated while the app runs.
@MainActor
final class MeetingLiveActivityManager {
    static let shared = MeetingLiveActivityManager()
    private init() {}

    func sync(live: [(Meeting, MeetingSession?)]) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        let wanted = Dictionary(live.map { ($0.0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for activity in Activity<MeetingActivityAttributes>.activities where activity.activityState == .active || activity.activityState == .stale {
            let id = activity.id
            if let pair = wanted[activity.attributes.meetingId] {
                let state = Self.state(pair.0, pair.1)
                Task { await Self.live(id)?.update(ActivityContent(state: state, staleDate: nil)) }
            } else {
                // Left the call (ended / not let in): show the outcome briefly.
                let final = MeetingActivityAttributes.ContentState(gizzi: "ended", liveSince: nil, listening: false)
                Task { await Self.live(id)?.end(ActivityContent(state: final, staleDate: nil), dismissalPolicy: .after(.now.addingTimeInterval(5 * 60))) }
            }
        }
        let running = Set(Activity<MeetingActivityAttributes>.activities.map(\.attributes.meetingId))
        for (meeting, session) in live where !running.contains(meeting.id) {
            let attributes = MeetingActivityAttributes(
                meetingId: meeting.id,
                sessionId: session?.id ?? meeting.sessionId ?? "",
                title: meeting.displayTitle,
                platformLabel: meeting.platformName
            )
            _ = try? Activity.request(attributes: attributes, content: ActivityContent(state: Self.state(meeting, session), staleDate: nil))
        }
    }

    func endAll() {
        let ids = Activity<MeetingActivityAttributes>.activities.map(\.id)
        Task {
            for id in ids { await Self.live(id)?.end(nil, dismissalPolicy: .immediate) }
        }
    }

    private static func state(_ meeting: Meeting, _ session: MeetingSession?) -> MeetingActivityAttributes.ContentState {
        MeetingActivityAttributes.ContentState(
            gizzi: session?.gizzi ?? meeting.gizzi,
            liveSince: (session?.liveDate ?? meeting.liveDate)?.timeIntervalSince1970,
            listening: session?.listening ?? true
        )
    }

    private nonisolated static func live(_ id: String) -> Activity<MeetingActivityAttributes>? {
        Activity<MeetingActivityAttributes>.activities.first { $0.id == id }
    }
}

// MARK: - Web sign-in with a callback

/// ASWebAuthenticationSession for calendar consent. Returns the callback URL
/// when the server redirected to the app scheme, nil when the person closed
/// the sheet (the cloud may land on its own page instead).
@MainActor
final class MeetingsWebAuth: NSObject {
    static let shared = MeetingsWebAuth()
    private var session: ASWebAuthenticationSession?

    func open(_ url: URL, callbackScheme: String) async -> URL? {
        let result = await withCheckedContinuation { (done: CheckedContinuation<URL?, Never>) in
            let session = ASWebAuthenticationSession(url: url, callbackURLScheme: callbackScheme) { callback, _ in
                done.resume(returning: callback)
            }
            session.presentationContextProvider = self
            session.prefersEphemeralWebBrowserSession = false
            self.session = session
            if !session.start() { done.resume(returning: nil) }
        }
        session = nil
        return result
    }
}

extension MeetingsWebAuth: ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        return scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first ?? UIWindow()
    }
}
