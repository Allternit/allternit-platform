import Foundation
import UIKit

// -----------------------------------------------------------------------------
// capture_screen_context on iOS: the app's own view, never other apps.
//
// The runtime publishes `screen.context.request` {id, sessionID, reason, time}
// (gizzi integrations/screen-context); the session sync stream carries it to
// the chat screen (AgentChatClient.sessionSyncEvents), and this responder
// answers `POST /screen-context/:requestId/reply` once with
// {kind: "app_view", app: "Allternit", title, text}. No image: iOS can't
// picture another app, and a picture of this one adds nothing the text lacks.
//
// What "the screen" is: the chat rows currently on screen (MessageRow reports
// them on appear/disappear). The responder answers only while the app is in
// the foreground and the asking session's chat is the one showing, so a
// backgrounded phone stays quiet and another surface (Desktop, web) can
// answer instead; with no answer the runtime times out after 10 s.
// -----------------------------------------------------------------------------

/// One screen context request off the session sync stream. Field names are
/// read leniently (`request_id`/`requestId`, `session_id`/`sessionId`) so the
/// sync frame and the bus event shape both decode.
struct ScreenContextRequest: Sendable {
    let requestId: String
    let sessionId: String
    /// Why the agent wants to look (the user approved it on the ask).
    let reason: String?
    /// When the runtime asked (ms).
    let time: Double?

    init?(payload: String) {
        guard let object = try? JSONSerialization.jsonObject(with: Data(payload.utf8)) as? [String: Any] else {
            return nil
        }
        func string(_ keys: [String]) -> String? {
            for key in keys {
                if let value = object[key] as? String, !value.isEmpty { return value }
            }
            return nil
        }
        guard let requestId = string(["request_id", "requestId", "requestID", "id"]),
              let sessionId = string(["session_id", "sessionId", "sessionID"]) else {
            return nil
        }
        self.requestId = requestId
        self.sessionId = sessionId
        self.reason = string(["reason"])
        self.time = (object["time"] as? NSNumber)?.doubleValue
    }

    /// The sync frame types that carry a request (the bus name, and the
    /// snake-case forms the sync bridges use for other requests).
    static let frameTypes: Set<String> = [
        "screen_context_requested", "screen_context_request", "screen.context.request"
    ]

    /// Cheap line filter before any JSON decoding.
    static func mayMatch(_ line: String) -> Bool {
        line.contains("screen_context_request") || line.contains("screen.context.request")
    }
}

/// The reply body (gizzi `ScreenContext.Capture`, kind `app_view`).
struct ScreenContextReply: Encodable, Sendable {
    var kind = "app_view"
    var app = "Allternit"
    let title: String
    let text: String
}

/// Holds what the chat screen shows and answers screen context requests
/// from it, once per request.
@MainActor
final class ScreenContextResponder {
    static let shared = ScreenContextResponder()

    /// gizzi `ScreenContext.TIMEOUT_MS`: an older request has already failed
    /// there, so a replayed event never answers again.
    static let timeout: TimeInterval = 10
    /// Per-row and total text caps (the runtime caps at 20,000).
    static let maxRowText = 2_000
    static let maxText = 8_000

    private struct Row {
        let sessionId: String
        let role: String
        var text: String
        let createdAt: Date
        var title: String
    }

    /// Visible chat rows, by message id.
    private var rows: [String: Row] = [:]
    /// Request ids already taken (several sync streams can carry one request).
    private var handled: Set<String> = []

    /// A chat row came on screen, or its text changed while showing.
    func show(messageId: String, sessionId: String?, role: String, text: String, createdAt: Date, title: String) {
        guard let sessionId, !sessionId.isEmpty, role == "user" || role == "assistant" else { return }
        rows[messageId] = Row(sessionId: sessionId, role: role, text: text, createdAt: createdAt, title: title)
    }

    /// A chat row left the screen.
    func hide(messageId: String) {
        rows[messageId] = nil
    }

    /// Answers one request from the current screen, or stays quiet when this
    /// phone isn't showing that session in the foreground.
    func answer(_ request: ScreenContextRequest, client: AgentChatClient) async {
        if let time = request.time, Date().timeIntervalSince1970 - time / 1000 > Self.timeout { return }
        guard UIApplication.shared.applicationState == .active,
              let reply = snapshot(sessionId: request.sessionId) else { return }
        guard handled.insert(request.requestId).inserted else { return }
        if handled.count > 500 { handled = [request.requestId] }
        for attempt in 1...2 {
            do {
                try await client.replyScreenContext(requestId: request.requestId, reply: reply)
                return
            } catch {
                if attempt == 2 { print("screen-context: reply failed: \(error.localizedDescription)") }
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
    }

    /// The app view for one session: screen name + the visible messages,
    /// newest kept when the text runs over the cap. Nil when that session's
    /// chat isn't on screen.
    func snapshot(sessionId: String) -> ScreenContextReply? {
        let visible = rows.values
            .filter { $0.sessionId == sessionId }
            .sorted { $0.createdAt < $1.createdAt }
        guard let title = visible.last?.title else { return nil }

        var blocks: [String] = []
        var budget = Self.maxText
        for row in visible.reversed() {
            var text = row.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { continue }
            if text.count > Self.maxRowText { text = String(text.prefix(Self.maxRowText)) + "…" }
            let block = "\(row.role == "user" ? "You" : "Assistant"): \(text)"
            if block.count > budget { break }
            budget -= block.count
            blocks.append(block)
        }
        let header = "Screen: \(title) (Allternit for iOS). The user is looking at this conversation. Visible messages, oldest first:"
        let body = blocks.isEmpty ? "(no messages on screen yet)" : blocks.reversed().joined(separator: "\n\n")
        return ScreenContextReply(title: title, text: "\(header)\n\n\(body)")
    }
}
