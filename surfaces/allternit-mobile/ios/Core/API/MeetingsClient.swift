import Foundation

// Allternit Meetings wire types and client (lane 2.11).
// Source of truth: docs/meetings/CONTRACT.md (frozen) and the web's
// allternit-ai src/lib/meetings/types.ts. Every route lives on cloud-api
// (`{cloud}/api/v1/...`), not on the user's runtime, so requests go straight
// to `AppConfig.cloudAPIBaseURL` like `AccountFlagsClient`.
//
// Decoding is lenient on purpose: fields a list row can live without are
// optional, and unknown enum strings stay strings, so one odd row never
// empties the agenda.

// MARK: - Models

struct Meeting: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var platform: String
    var platformLabel: String?
    var title: String
    var joinUrl: String
    var urlKey: String?
    var startsAt: String?
    var endsAt: String?
    var source: String?
    var attendeeCount: Int?
    var externalAttendeeCount: Int?
    var isOneOnOne: Bool?
    /// Per-meeting override of the auto-join rule; nil follows the rule.
    var autoJoin: Bool?
    var replyMode: String?
    /// `scheduled|off|launching|joining|waiting_admit|live|paused|ended|not_admitted`.
    var gizzi: String
    var peopleLabel: String?
    var latestSessionId: String?
    var pageUrl: String?
    /// When the latest session went live (drives the "Live · 12 min" chip).
    var liveAt: String?
    /// `GET /meetings/:id` only: the latest session in brief.
    var latestSession: LatestSession?

    struct LatestSession: Codable, Hashable, Sendable {
        let id: String
        var state: String
        var endReason: String?
        var liveAt: String?
        var endedAt: String?
        var summary: String?
    }

    var startDate: Date? { MeetingDates.parse(startsAt) }
    var liveDate: Date? { MeetingDates.parse(liveAt ?? latestSession?.liveAt) }
    /// The session to open: the brief one from `GET /meetings/:id`, else the id.
    var sessionId: String? { latestSession?.id ?? latestSessionId }
    var endDate: Date? { MeetingDates.parse(endsAt) }
    var status: GizziStatus { GizziStatus(rawValue: gizzi) ?? .off }

    var displayTitle: String {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? (platformLabel ?? "Call") : trimmed
    }

    var platformName: String {
        if let platformLabel, !platformLabel.isEmpty { return platformLabel }
        switch platform {
        case "meet": return "Google Meet"
        case "zoom": return "Zoom"
        case "teams": return "Microsoft Teams"
        default: return "Call"
        }
    }

    /// Gizzi will be (or is) in this call: the switch's value.
    var gizziOn: Bool {
        switch status {
        case .off, .notAdmitted: return false
        case .ended: return autoJoin ?? false
        default: return true
        }
    }
}

/// The per-meeting state the UI shows (CONTRACT §3 `gizzi`).
enum GizziStatus: String, Sendable {
    case scheduled, off, launching, joining
    case waitingAdmit = "waiting_admit"
    case live, paused, ended
    case notAdmitted = "not_admitted"

    /// A bot is (or is about to be) in the call.
    var isActive: Bool {
        switch self {
        case .launching, .joining, .waitingAdmit, .live, .paused: return true
        default: return false
        }
    }
}

struct MeetingActionItem: Codable, Hashable, Sendable {
    var text: String
    var owner: String?
    var due: String?
}

struct MeetingSession: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var meetingId: String
    var state: String
    var gizzi: String
    var endReason: String?
    var platform: String?
    var speakReplies: Bool
    var listening: Bool
    var botIdentity: String?
    var scheduledJoinAt: String?
    var joinedAt: String?
    var liveAt: String?
    var endedAt: String?
    var summary: String?
    var decisions: [String]?
    var actionItems: [MeetingActionItem]?

    var status: GizziStatus { GizziStatus(rawValue: gizzi) ?? .off }
    var liveDate: Date? { MeetingDates.parse(liveAt) }
    var endedDate: Date? { MeetingDates.parse(endedAt) }
}

struct MeetingParticipant: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var displayName: String
    var email: String?
    var `class`: String?
    var joinedAt: String?
    var leftAt: String?
}

struct TranscriptSegment: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var seq: Int
    var speaker: String?
    var confidence: Double?
    var source: String?
    var startMs: Int
    var endMs: Int
    var text: String
}

struct MeetingArtifact: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var artifactId: String?
    var kind: String
    var title: String
    var url: String
    var version: Int?
    var updatedAt: String?
}

struct ToolApproval: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var tool: String
    var argsSummary: String
    var askedBy: String?
    var status: String
    var requestedAt: String?
}

struct UserMeetingSettings: Codable, Hashable, Sendable {
    /// `off|all|all_except_1on1`.
    var autoJoin: String
    var recapEmail: Bool
    /// `chat|voice`.
    var defaultReplyMode: String
}

struct CalendarConnection: Codable, Identifiable, Hashable, Sendable {
    let id: String
    /// `google|ics|microsoft`.
    var provider: String
    var accountEmail: String?
    var label: String?
    var status: String
    var lastSyncedAt: String?

    var providerName: String {
        switch provider {
        case "google": return "Google Calendar"
        case "ics": return "Calendar link"
        case "microsoft": return "Outlook"
        default: return "Calendar"
        }
    }

    var detail: String {
        if status == "needs_reauth" { return "Reconnect needed" }
        if let accountEmail, !accountEmail.isEmpty { return accountEmail }
        return label ?? ""
    }
}

struct MeetingUsage: Codable, Hashable, Sendable {
    var usedMinutes: Int
    var includedMinutes: Int
    var plan: String
}

struct MeetingSessionDetail: Codable, Sendable {
    var session: MeetingSession
    var meeting: Meeting
    var participants: [MeetingParticipant]?
    var pendingApprovals: [ToolApproval]?
}

/// The auto-join rule choices, in the order the UI offers them.
enum AutoJoinRule: String, CaseIterable, Identifiable, Sendable {
    case allExcept1on1 = "all_except_1on1"
    case all
    case off

    var id: String { rawValue }

    var title: String {
        switch self {
        case .allExcept1on1: return "All calls except 1:1s"
        case .all: return "Every call with a link"
        case .off: return "Only when I ask"
        }
    }

    var detail: String {
        switch self {
        case .allExcept1on1: return "Gizzi joins your calendar calls with three or more people."
        case .all: return "Gizzi joins every Meet, Zoom and Teams call on your calendar."
        case .off: return "Turn Gizzi on per call, or paste a link."
        }
    }
}

enum MeetingDates {
    nonisolated(unsafe) private static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    nonisolated(unsafe) private static let plain = ISO8601DateFormatter()

    static func parse(_ string: String?) -> Date? {
        guard let string, !string.isEmpty else { return nil }
        return fractional.date(from: string) ?? plain.date(from: string)
    }

    static func iso(_ date: Date) -> String { plain.string(from: date) }
}

// MARK: - SSE events (CONTRACT §5)

enum MeetingEvent: Sendable {
    case state(state: String, gizzi: String, endReason: String?)
    case participant(MeetingParticipant, change: String)
    case transcript([TranscriptSegment])
    case reply(channel: String, text: String)
    case approvalRequested(ToolApproval)
    case approvalResolved(ToolApproval)
    case artifact(MeetingArtifact)
    case ended(endReason: String)
    case summary(summary: String, decisions: [String], actionItems: [MeetingActionItem])

    static func decode(type: String, data: Data) -> MeetingEvent? {
        let decoder = MeetingsClient.decoder
        switch type {
        case "meeting.session.state":
            struct P: Decodable { let state: String; let gizzi: String; let endReason: String? }
            return (try? decoder.decode(P.self, from: data)).map { .state(state: $0.state, gizzi: $0.gizzi, endReason: $0.endReason) }
        case "meeting.participant.changed":
            struct P: Decodable { let participant: MeetingParticipant; let change: String }
            return (try? decoder.decode(P.self, from: data)).map { .participant($0.participant, change: $0.change) }
        case "meeting.transcript.delta":
            struct P: Decodable { let segments: [TranscriptSegment] }
            return (try? decoder.decode(P.self, from: data)).map { .transcript($0.segments) }
        case "meeting.reply":
            struct P: Decodable { let channel: String; let text: String }
            return (try? decoder.decode(P.self, from: data)).map { .reply(channel: $0.channel, text: $0.text) }
        case "meeting.approval.requested", "meeting.approval.resolved":
            struct P: Decodable { let approval: ToolApproval }
            guard let p = try? decoder.decode(P.self, from: data) else { return nil }
            return type.hasSuffix("requested") ? .approvalRequested(p.approval) : .approvalResolved(p.approval)
        case "meeting.artifact.created", "meeting.artifact.updated":
            struct P: Decodable { let artifact: MeetingArtifact }
            return (try? decoder.decode(P.self, from: data)).map { .artifact($0.artifact) }
        case "meeting.ended":
            struct P: Decodable { let endReason: String }
            return (try? decoder.decode(P.self, from: data)).map { .ended(endReason: $0.endReason) }
        case "meeting.summary":
            struct P: Decodable { let summary: String; let decisions: [String]?; let actionItems: [MeetingActionItem]? }
            return (try? decoder.decode(P.self, from: data)).map {
                .summary(summary: $0.summary, decisions: $0.decisions ?? [], actionItems: $0.actionItems ?? [])
            }
        default:
            return nil
        }
    }
}

// MARK: - Errors

enum MeetingsError: Error, LocalizedError, Equatable, Sendable {
    /// 401: no session token (signed out or expired).
    case signedOut
    /// 402 `meetings_requires_paid_plan`.
    case paidPlanRequired
    /// 403 `meetings_disabled_by_org` / `forbidden`: the message names the setting.
    case forbidden(String)
    /// 400 `unsupported_link`.
    case unsupportedLink
    /// 429 `no_capacity`.
    case noCapacity
    /// 409 `zoom_not_connected`: Zoom calls need the user's Zoom account (OBF).
    case zoomNotConnected
    case server(status: Int, code: String?, message: String?)
    case offline(String)

    var errorDescription: String? {
        switch self {
        case .signedOut: return "Sign in to use Meetings."
        case .paidPlanRequired: return "Meetings is part of the Plus, Super and Ultra plans."
        case .forbidden(let message): return message
        case .unsupportedLink: return "That isn't a Google Meet, Zoom or Microsoft Teams link."
        case .noCapacity: return "Gizzi is in too many calls right now. Try again in a minute."
        case .zoomNotConnected: return "Connect your Zoom account first. Zoom only lets Gizzi in through the account of someone in the call."
        case .server(let status, _, let message): return message ?? "Meetings answered \(status). Try again."
        case .offline(let message): return message
        }
    }
}

// MARK: - Client

final class MeetingsClient: Sendable {
    static let shared = MeetingsClient()

    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }()

    private init() {}

    private func url(_ path: String, query: [URLQueryItem] = []) -> URL {
        let base = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/\(path)")
        guard !query.isEmpty, var comps = URLComponents(url: base, resolvingAgainstBaseURL: false) else { return base }
        comps.queryItems = query
        return comps.url ?? base
    }

    private func send(_ method: String, _ url: URL, body: Data? = nil) async throws -> Data {
        let api = APIClient.shared
        var request = try await api.authorizedRequest(url: url, method: method)
        request.httpBody = body
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await api.session.data(for: request)
        } catch {
            throw MeetingsError.offline(FriendlyErrorMessage.from(error.localizedDescription))
        }
        guard let http = response as? HTTPURLResponse else { throw MeetingsError.server(status: 0, code: nil, message: nil) }
        guard (200...299).contains(http.statusCode) else {
            throw Self.error(status: http.statusCode, data: data)
        }
        return data
    }

    static func error(status: Int, data: Data) -> MeetingsError {
        struct Body: Decodable { let error: String?; let message: String? }
        let body = try? JSONDecoder().decode(Body.self, from: data)
        switch (status, body?.error) {
        case (401, _): return .signedOut
        case (402, _), (_, "meetings_requires_paid_plan"): return .paidPlanRequired
        case (403, _): return .forbidden(body?.message ?? "Your organization has turned Meetings off.")
        case (_, "unsupported_link"): return .unsupportedLink
        case (_, "zoom_not_connected"): return .zoomNotConnected
        case (429, _): return .noCapacity
        default: return .server(status: status, code: body?.error, message: body?.message)
        }
    }

    private func get<T: Decodable>(_ type: T.Type, _ url: URL) async throws -> T {
        let data = try await send("GET", url)
        do { return try Self.decoder.decode(T.self, from: data) } catch {
            throw MeetingsError.server(status: 200, code: "decode", message: "Meetings sent something this app can't read yet. Update the app.")
        }
    }

    private func write<T: Decodable, B: Encodable>(_ method: String, _ url: URL, body: B, as type: T.Type) async throws -> T {
        let data = try await send(method, url, body: try Self.encoder.encode(body))
        do { return try Self.decoder.decode(T.self, from: data) } catch {
            throw MeetingsError.server(status: 200, code: "decode", message: "Meetings sent something this app can't read yet. Update the app.")
        }
    }

    // Meetings

    /// `GET /meetings?range=` — `now|today|week|upcoming|past`, agenda order.
    func list(range: String, limit: Int = 50) async throws -> [Meeting] {
        struct R: Decodable { let meetings: [Meeting] }
        return try await get(R.self, url("meetings", query: [
            URLQueryItem(name: "range", value: range), URLQueryItem(name: "limit", value: String(limit)),
        ])).meetings
    }

    /// `GET /meetings/:id` — one row with `gizzi` and the latest summary.
    func meeting(_ id: String) async throws -> Meeting {
        try await get(Meeting.self, url("meetings/\(id)"))
    }

    func create(url link: String, title: String? = nil) async throws -> Meeting {
        struct B: Encodable { let url: String; let title: String? }
        return try await write("POST", url("meetings"), body: B(url: link, title: title), as: Meeting.self)
    }

    /// `PATCH /meetings/:id {auto_join}` — the Gizzi switch.
    func setAutoJoin(meetingId: String, on: Bool) async throws -> Meeting {
        struct B: Encodable { let autoJoin: Bool }
        return try await write("PATCH", url("meetings/\(meetingId)"), body: B(autoJoin: on), as: Meeting.self)
    }

    /// `POST /meetings/:id/sessions {when}` — attaches when a bot is already there.
    func startSession(meetingId: String, when: String, speakReplies: Bool? = nil) async throws -> MeetingSession {
        struct B: Encodable { let when: String; let speakReplies: Bool? }
        return try await write("POST", url("meetings/\(meetingId)/sessions"), body: B(when: when, speakReplies: speakReplies), as: MeetingSession.self)
    }

    // Sessions

    func session(_ sid: String) async throws -> MeetingSessionDetail {
        try await get(MeetingSessionDetail.self, url("meeting-sessions/\(sid)"))
    }

    func transcript(_ sid: String, afterSeq: Int? = nil, limit: Int = 500) async throws -> (segments: [TranscriptSegment], next: Int?) {
        struct R: Decodable { let segments: [TranscriptSegment]; let nextAfterSeq: Int? }
        var query = [URLQueryItem(name: "limit", value: String(limit))]
        if let afterSeq { query.append(URLQueryItem(name: "after_seq", value: String(afterSeq))) }
        let r = try await get(R.self, url("meeting-sessions/\(sid)/transcript", query: query))
        return (r.segments, r.nextAfterSeq)
    }

    func artifacts(_ sid: String) async throws -> [MeetingArtifact] {
        struct R: Decodable { let artifacts: [MeetingArtifact] }
        return try await get(R.self, url("meeting-sessions/\(sid)/artifacts")).artifacts
    }

    /// `POST /meeting-sessions/:sid/control` — `stop_listening|resume|leave|speak`.
    func control(_ sid: String, action: String, text: String? = nil) async throws -> MeetingSession {
        struct B: Encodable { let action: String; let text: String? }
        return try await write("POST", url("meeting-sessions/\(sid)/control"), body: B(action: action, text: text), as: MeetingSession.self)
    }

    /// `PATCH /meeting-sessions/:sid {speak_replies}` — the Speak replies switch.
    func setSpeakReplies(_ sid: String, on: Bool) async throws -> MeetingSession {
        struct B: Encodable { let speakReplies: Bool }
        return try await write("PATCH", url("meeting-sessions/\(sid)"), body: B(speakReplies: on), as: MeetingSession.self)
    }

    func decide(_ sid: String, approvalId: String, approve: Bool) async throws -> ToolApproval {
        struct B: Encodable { let decision: String }
        return try await write("POST", url("meeting-sessions/\(sid)/approvals/\(approvalId)"),
                               body: B(decision: approve ? "approve" : "deny"), as: ToolApproval.self)
    }

    /// The live event stream. Yields `(id, type, data)` frames; ends when the
    /// server closes. Throws `MeetingsError` for a non-2xx answer.
    func events(_ sid: String, lastEventId: String?) -> AsyncThrowingStream<(id: String?, type: String, data: Data), Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let api = APIClient.shared
                    var request = try await api.authorizedRequest(url: self.url("meeting-sessions/\(sid)/events"))
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    request.timeoutInterval = 90
                    if let lastEventId { request.setValue(lastEventId, forHTTPHeaderField: "Last-Event-ID") }
                    let (bytes, response) = try await api.session.bytes(for: request)
                    guard let http = response as? HTTPURLResponse else { throw MeetingsError.server(status: 0, code: nil, message: nil) }
                    guard (200...299).contains(http.statusCode) else {
                        var body = Data()
                        for try await byte in bytes.prefix(2048) { body.append(byte) }
                        throw Self.error(status: http.statusCode, data: body)
                    }
                    var id: String? = nil, type = "message", data = ""
                    for try await line in bytes.lines {
                        if line.isEmpty {
                            if !data.isEmpty { continuation.yield((id, type, Data(data.utf8))) }
                            type = "message"; data = ""
                            continue
                        }
                        if line.hasPrefix(":") { continue }
                        let (field, value) = Self.field(line)
                        switch field {
                        case "id": id = value
                        case "event": type = value
                        case "data": data += data.isEmpty ? value : "\n" + value
                        default: break
                        }
                        // `bytes.lines` drops the blank separator line on some
                        // OS versions: a complete JSON payload ends the frame.
                        if field == "data", value.hasSuffix("}") {
                            continuation.yield((id, type, Data(data.utf8)))
                            type = "message"; data = ""
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func field(_ line: String) -> (String, String) {
        guard let colon = line.firstIndex(of: ":") else { return (line, "") }
        var value = String(line[line.index(after: colon)...])
        if value.hasPrefix(" ") { value.removeFirst() }
        return (String(line[..<colon]), value)
    }

    // Settings, usage

    func settings() async throws -> UserMeetingSettings {
        try await get(UserMeetingSettings.self, url("me/meeting-settings"))
    }

    func updateSettings(autoJoin: String? = nil, recapEmail: Bool? = nil, defaultReplyMode: String? = nil) async throws -> UserMeetingSettings {
        struct B: Encodable { let autoJoin: String?; let recapEmail: Bool?; let defaultReplyMode: String? }
        return try await write("PUT", url("me/meeting-settings"),
                               body: B(autoJoin: autoJoin, recapEmail: recapEmail, defaultReplyMode: defaultReplyMode),
                               as: UserMeetingSettings.self)
    }

    func usage() async throws -> MeetingUsage {
        try await get(MeetingUsage.self, url("me/meeting-usage"))
    }

    // Calendars

    func connections() async throws -> [CalendarConnection] {
        struct R: Decodable { let connections: [CalendarConnection] }
        return try await get(R.self, url("calendar/connections")).connections
    }

    /// `GET /calendar/oauth/google/start?return_to=` → the consent URL.
    func googleConsentURL(returnTo: String) async throws -> URL {
        struct R: Decodable { let url: String }
        let r = try await get(R.self, url("calendar/oauth/google/start", query: [URLQueryItem(name: "return_to", value: returnTo)]))
        guard let consent = URL(string: r.url) else { throw MeetingsError.server(status: 200, code: "bad_url", message: nil) }
        return consent
    }

    /// `POST /calendar/connections {provider:'ics', feed_url}` — validated by one fetch.
    func addCalendarLink(_ feedURL: String, label: String? = nil) async throws -> CalendarConnection {
        struct B: Encodable { let provider: String; let feedUrl: String; let label: String? }
        return try await write("POST", url("calendar/connections"), body: B(provider: "ics", feedUrl: feedURL, label: label), as: CalendarConnection.self)
    }

    // Zoom (docs/meetings/ZOOM.md §3, accepted into CONTRACT.md)

    struct ZoomConnection: Decodable, Sendable { let connected: Bool; let status: String? }

    func zoomConnection() async throws -> ZoomConnection {
        try await get(ZoomConnection.self, url("zoom/connection"))
    }

    func zoomConsentURL(returnTo: String) async throws -> URL {
        struct R: Decodable { let url: String }
        let r = try await get(R.self, url("zoom/oauth/start", query: [URLQueryItem(name: "return_to", value: returnTo)]))
        guard let consent = URL(string: r.url) else { throw MeetingsError.server(status: 200, code: "bad_url", message: nil) }
        return consent
    }

    func disconnectZoom() async throws {
        _ = try await send("DELETE", url("zoom/connection"))
    }

    func removeConnection(_ id: String) async throws {
        _ = try await send("DELETE", url("calendar/connections/\(id)"))
    }
}
