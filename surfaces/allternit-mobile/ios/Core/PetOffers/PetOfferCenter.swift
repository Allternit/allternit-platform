import Foundation

/// One failure an Allternit run reported (a chat stream error, a failed cowork
/// load). `text` stays on the device: it is used for the matcher and the
/// composer prefill, never logged and never put in a notification or URL.
struct PetOfferFailure: Sendable {
    let text: String
    /// The bot whose run failed; nil falls back to the main bot.
    let botId: String?
    /// The chat session the failure belongs to, for per-chat mute and the
    /// "is this chat on screen" check.
    let sessionId: String?
}

/// A matcher result: the rule that fired and the line it fired on.
struct PetOfferMatch: Equatable, Sendable {
    let kind: PetOfferKind
    let ruleId: String
    let line: String
}

/// The offer the banner shows (contract offer shape, iOS subset).
struct PetOffer: Identifiable, Equatable, Sendable {
    let id: String
    let kind: PetOfferKind
    let ruleId: String
    let line: String
    let botId: String?
    let sessionId: String?
    let createdAt: Date
}

/// Spots a failed run and offers help in an in-app banner. Acts only on a tap.
/// Gate order: switch, screen, mute, cooldown, same line.
@MainActor
final class PetOfferCenter: ObservableObject {
    static let shared = PetOfferCenter()

    /// The offer on screen, nil when none.
    @Published private(set) var current: PetOffer?

    /// The chat session on screen right now (ShellSessionsStore.setActive).
    /// A failure in that chat is already in front of the user: no offer.
    var visibleSessionId: String?

    private let defaults: UserDefaults
    private var lastOfferAt: Date?
    private var lastLine: String?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    // MARK: Switch + mute

    var isEnabled: Bool {
        get { defaults.object(forKey: PetOfferContract.SettingsKey.enabled) as? Bool ?? PetOfferContract.SettingsDefault.enabled }
        set {
            defaults.set(newValue, forKey: PetOfferContract.SettingsKey.enabled)
            if !newValue { current = nil }
            objectWillChange.send()
        }
    }

    private var muted: Set<String> {
        Set(defaults.stringArray(forKey: PetOfferContract.SettingsKey.muted) ?? PetOfferContract.SettingsDefault.muted)
    }

    func isBotMuted(_ botId: String) -> Bool { muted.contains("bot:\(botId)") }

    func muteBot(_ botId: String) {
        defaults.set(Array(muted.union(["bot:\(botId)"])).sorted(), forKey: PetOfferContract.SettingsKey.muted)
        if current?.botId == botId { current = nil }
    }

    func muteChat(_ sessionId: String) {
        defaults.set(Array(muted.union(["chat:\(sessionId)"])).sorted(), forKey: PetOfferContract.SettingsKey.muted)
        if current?.sessionId == sessionId { current = nil }
    }

    /// Settings list: every mute key, "bot:<id>" or "chat:<id>", sorted.
    var mutedKeys: [String] { muted.sorted() }

    /// Settings: undo one mute (a key from `mutedKeys`).
    func unmute(_ key: String) {
        defaults.set(muted.subtracting([key]).sorted(), forKey: PetOfferContract.SettingsKey.muted)
        objectWillChange.send()
    }

    // MARK: Report + gate

    /// Offers help for a failed run. Returns the offer, or nil when the gate said no.
    @discardableResult
    func report(failure: PetOfferFailure, now: Date = Date()) -> PetOffer? {
        guard isEnabled else { return nil }
        if let sid = failure.sessionId, sid == visibleSessionId { return nil }
        let line = Self.eventLine(from: failure.text)
        guard !line.isEmpty else { return nil }
        let muteSet = muted
        if let bot = failure.botId, muteSet.contains("bot:\(bot)") { return nil }
        if let sid = failure.sessionId, muteSet.contains("chat:\(sid)") { return nil }
        let cooldown = Double(PetOfferContract.SettingsDefault.cooldownMs) / 1000
        if let last = lastOfferAt, now.timeIntervalSince(last) < cooldown { return nil }
        if line == lastLine { return nil }

        let offer = PetOffer(
            id: UUID().uuidString, kind: .runFailed, ruleId: "event", line: line,
            botId: failure.botId, sessionId: failure.sessionId, createdAt: now
        )
        lastOfferAt = now
        lastLine = line
        current = offer
        return offer
    }

    /// "Not now", the 8 s timeout, or after "Fix it".
    func dismiss(_ id: String? = nil) {
        if id == nil || current?.id == id { current = nil }
    }

    // MARK: Copy

    static func copy(for kind: PetOfferKind, app: String? = nil) -> String {
        (PetOfferContract.copyByKind[kind] ?? "")
            .replacingOccurrences(of: PetOfferContract.appPlaceholder, with: app ?? PetOfferContract.appFallback)
    }

    /// What lands in the composer. The owner reads it and sends it.
    static func prefill(for offer: PetOffer) -> String {
        "This failed: \(offer.line). Find the cause and fix it."
    }

    // MARK: Matcher

    /// First non-empty line of an event's text, cut to the contract limit.
    static func eventLine(from text: String) -> String {
        for raw in normalize(text).split(separator: "\n", omittingEmptySubsequences: true) {
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { return cut(trimmed) }
        }
        return ""
    }

    /// Runs the contract rules over `text`. `dialog` is true only when the text
    /// came from a dialog; rules with scope `dialog` skip otherwise.
    static func match(text: String, dialog: Bool = false) -> PetOfferMatch? {
        var body = normalize(text)
        if body.count > PetOfferContract.SettingsDefault.maxTextChars {
            body = String(body.suffix(PetOfferContract.SettingsDefault.maxTextChars))
        }
        let ns = body as NSString
        let whole = NSRange(location: 0, length: ns.length)
        for (rule, regex) in compiledRules {
            if rule.scope == .dialog && !dialog { continue }
            guard let hit = regex.firstMatch(in: body, options: [], range: whole) else { continue }
            let start = hit.range.location
            let before = ns.range(of: "\n", options: .backwards, range: NSRange(location: 0, length: start))
            let lineStart = before.location == NSNotFound ? 0 : before.upperBound
            let after = ns.range(of: "\n", options: [], range: NSRange(location: start, length: ns.length - start))
            let lineEnd = after.location == NSNotFound ? ns.length : after.location
            let line = ns.substring(with: NSRange(location: lineStart, length: lineEnd - lineStart))
            return PetOfferMatch(kind: rule.kind, ruleId: rule.id, line: cut(line.trimmingCharacters(in: .whitespacesAndNewlines)))
        }
        return nil
    }

    private static func normalize(_ text: String) -> String {
        text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
    }

    private static func cut(_ line: String) -> String {
        String(line.prefix(PetOfferContract.lineMaxChars))
    }

    private static let compiledRules: [(PetOfferRule, NSRegularExpression)] = PetOfferContract.rules.compactMap { rule in
        var options: NSRegularExpression.Options = []
        if rule.caseInsensitive { options.insert(.caseInsensitive) }
        if rule.multiline { options.insert(.anchorsMatchLines) }
        guard let regex = try? NSRegularExpression(pattern: rule.pattern, options: options) else { return nil }
        return (rule, regex)
    }
}
