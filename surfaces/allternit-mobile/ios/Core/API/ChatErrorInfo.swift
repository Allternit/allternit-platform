import Foundation

/// A typed chat or generation failure (design pick: errors B): a plain reason,
/// the machine code when the server sent one, the usage-limit reset time, and
/// which way out to offer. Port of the web `src/lib/errors/chat-error.ts`
/// (ai #1229) over the chat error frame `{type:"error", error, code?, reset_at?}`
/// (platform #2807). The inline error card and the generation failure card
/// both draw from it.
struct ChatErrorInfo: Equatable, Sendable {
    enum Kind: String, Sendable { case usageLimit = "usage_limit", signedOut = "signed_out", network, server }
    enum Action: String, Sendable { case connect, pickModel = "pick_model", retry }
    /// Which surface the failure belongs to.
    enum Lane: Sendable { case picture, motion, chat }

    var kind: Kind
    /// One or two plain sentences: what happened, in the server's words when it said.
    var reason: String
    var code: String? = nil
    /// Usage limit: when it resets.
    var resetAt: Date? = nil
    var isChatGPT: Bool = false
    var action: Action? = nil

    static let usageLimitCodes: Set<String> = [
        "subscription_sharing_usage_limit_exceeded", "subscription_sharing_usage_unavailable",
        "usage_limit_reached", "usage_limit_exceeded", "usage_limit", "plan_rate", "vendor_limited",
    ]
    static let signedOutCodes: Set<String> = [
        "signed_out", "login_expired", "not_signed_in", "signed_out_allternit",
        "subscription_sharing_invalid_user", "chatpass_v2_scope_not_authorized",
        "chatpass_v2_invalid_authorization_context",
    ]
    static let networkCodes: Set<String> = ["network", "stream_ended"]

    /// The server's own sentences when the code itself was lost on the way.
    private static let usageLimitText = try! NSRegularExpression(pattern: #"\b(?:usage limit|limit reached|limit is used up)\b"#, options: .caseInsensitive)
    private static let signedOutText = try! NSRegularExpression(pattern: #"\b(?:gizzi connect(?: chatgpt)?|sign-in expired|not signed in|did not accept your sign-in|sign in again)\b"#, options: .caseInsensitive)
    private static let networkText = try! NSRegularExpression(pattern: #"^(?:failed to fetch|fetch failed|networkerror|network error|load failed)\b|\bcould not reach\b|\bcouldn't reach\b"#, options: .caseInsensitive)

    private static func matches(_ regex: NSRegularExpression, _ text: String) -> Bool {
        regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)) != nil
    }

    /// A reset time from epoch seconds or milliseconds (the server sends either).
    static func resetDate(epoch: Double?) -> Date? {
        guard let epoch, epoch > 0 else { return nil }
        return Date(timeIntervalSince1970: epoch < 1e12 ? epoch : epoch / 1000)
    }

    /// The typed error for anything a chat turn or a generation can fail with.
    /// Unknown errors keep the server's message; only an error with no words
    /// at all falls back to a sentence.
    static func make(message: String?, code: String? = nil, status: Int? = nil, resetAt: Date? = nil, lane: Lane = .chat) -> ChatErrorInfo {
        let text = (message ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let lowered = code?.lowercased()
        let chatgpt = (lowered.map { $0.hasPrefix("subscription_sharing") || $0.hasPrefix("chatpass") } ?? false)
            || text.range(of: "chatgpt", options: .caseInsensitive) != nil
        let base = { (kind: Kind, reason: String, action: Action) in
            ChatErrorInfo(kind: kind, reason: reason, code: code, resetAt: nil, isChatGPT: chatgpt, action: action)
        }
        if usageLimitCodes.contains(lowered ?? "") || status == 429 || matches(usageLimitText, text) {
            var info = base(.usageLimit, text.isEmpty ? (chatgpt ? "ChatGPT will not take more right now." : "The usage limit is reached for now.") : text, .pickModel)
            info.resetAt = resetAt
            return info
        }
        if signedOutCodes.contains(lowered ?? "") || status == 401 || matches(signedOutText, text) {
            var info = base(.signedOut, signedOutReason(text, chatgpt: chatgpt, lane: lane), .connect)
            // Browser sign-in is separate from OAuth Connections.
            if lane == .picture && chatgpt { info.action = nil }
            return info
        }
        if networkCodes.contains(lowered ?? "") || matches(networkText, text) {
            let bare = text.range(of: #"^(?:failed to fetch|fetch failed|load failed)$"#, options: [.regularExpression, .caseInsensitive]) != nil
            return base(.network, text.isEmpty || bare ? "Couldn't reach Allternit. Check your connection and try again." : text, .retry)
        }
        return base(.server, text.isEmpty ? "Something went wrong. Try again." : text, .retry)
    }

    /// The words for a sign-in error: the CLI command means nothing in the app.
    private static func signedOutReason(_ text: String, chatgpt: Bool, lane: Lane) -> String {
        if !text.isEmpty, text.range(of: "gizzi connect", options: .caseInsensitive) == nil { return text }
        if !chatgpt { return "Sign in again to keep going." }
        return lane == .chat ? "Sign in to ChatGPT on this phone to keep going." : "Sign in to ChatGPT in this device’s ACI image browser, then confirm a new image request. OAuth chat sign-in does not sign in that browser."
    }

    /// "3:40 PM" today, "Tue 3:40 PM" on another day; nil when unknown or past.
    static func formatReset(_ resetAt: Date?, now: Date = Date(), calendar: Calendar = .current, locale: Locale = .current) -> String? {
        guard let resetAt, resetAt > now else { return nil }
        let time = DateFormatter()
        time.locale = locale
        time.calendar = calendar
        time.timeZone = calendar.timeZone
        time.dateStyle = .none
        time.timeStyle = .short
        if calendar.isDate(resetAt, inSameDayAs: now) { return time.string(from: resetAt) }
        let day = DateFormatter()
        day.locale = locale
        day.calendar = calendar
        day.timeZone = calendar.timeZone
        day.setLocalizedDateFormatFromTemplate("EEE")
        return "\(day.string(from: resetAt)) \(time.string(from: resetAt))"
    }

    /// The card title for an error on a lane ("ChatGPT usage limit - resets at 3:40 PM").
    func title(lane: Lane, now: Date = Date()) -> String {
        switch kind {
        case .usageLimit:
            let head = isChatGPT ? "ChatGPT usage limit" : "Usage limit reached"
            return Self.formatReset(resetAt, now: now).map { "\(head) — resets at \($0)" } ?? head
        case .signedOut:
            return isChatGPT ? "ChatGPT is not signed in" : "Sign-in needed"
        default:
            switch lane {
            case .picture: return "Picture not made"
            case .motion: return "Clip not made"
            case .chat: return kind == .network ? "Couldn't reach Allternit" : "Reply not finished"
            }
        }
    }

    /// The button label for the way out (nil: none).
    var actionLabel: String? {
        switch action {
        case .connect: return isChatGPT ? "Connect ChatGPT" : "Open Connections"
        case .pickModel: return "Pick another model"
        case .retry: return "Try again"
        case nil: return nil
        }
    }
}

/// A `{type:"error", error, code?, reset_at?}` chat stream frame.
struct StreamErrorFrame: Equatable, Sendable {
    var message: String
    var code: String?
    var resetAt: Date?

    var info: ChatErrorInfo { ChatErrorInfo.make(message: message, code: code, resetAt: resetAt) }
}
