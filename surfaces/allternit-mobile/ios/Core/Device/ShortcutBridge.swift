import Foundation
import UIKit

/// Runs one of the person's own shortcuts for a bot (`device.shortcuts.run`,
/// contract §3.1 `phone.shortcuts.run` and §9.1), through Apple's public
/// `shortcuts://x-callback-url/run-shortcut` URL.
///
/// Each run gets a random request id and waits in `pending` until one of two
/// answers comes back, or the timeout:
/// - `ReturnShortcutResultIntent`, an App Intent the shortcut calls as its
///   last step (it runs without opening the app). Allternit's own shortcuts
///   get the id in their JSON input as `request_id`.
/// - the x-success / x-error / x-cancel callback
///   (`allternit://shortcut-result?request=<id>&status=…`), which Shortcuts
///   opens when the shortcut ends. Only a pending id is accepted, so a link
///   from anywhere else can't fake a result.
///
/// Whatever comes first wins; the other is ignored. Approval is not decided
/// here: sends are approved on the person's computer before the request
/// reaches the phone (phone-tools.ts `phoneUseApproval`), the same path as
/// calendar writes. This type only runs what the paired Desktop sent.
@MainActor
final class ShortcutBridge {
    static let shared = ShortcutBridge()

    enum ReturnedVia: String, Sendable {
        case appIntent = "app_intent"
        case xCallback = "x_callback"
    }

    enum Outcome: Equatable, Sendable {
        case output(String?, ReturnedVia)
        /// x-error, with Shortcuts' `errorMessage`.
        case failed(String)
        case cancelled
        case timedOut
        /// `shortcuts://` would not open (Shortcuts deleted, or restricted).
        case couldNotOpen
    }

    private struct Pending {
        let name: String
        let continuation: CheckedContinuation<Outcome, Never>
        var timer: Task<Void, Never>?
    }

    nonisolated static let callbackScheme = "allternit"
    nonisolated static let callbackHost = "shortcut-result"
    /// Contract default for `timeout_s`.
    nonisolated static let defaultTimeout = 30
    /// The runtime fails a request at its own timeout (30 s today); the phone
    /// answers this much earlier so the bot gets the plain sentence instead.
    nonisolated static let runtimeMargin: TimeInterval = 2
    /// Shortcuts takes the input in the URL; keep it well under URL limits.
    nonisolated static let maxInputBytes = 32 * 1024
    /// One run at a time is normal; a few may overlap. More is a stuck bot.
    nonisolated static let maxPending = 4
    /// Allternit-authored shortcuts ("Allternit Actions", "Allternit Slack" …)
    /// read `request_id` from their JSON input.
    nonisolated static let ownPrefix = "Allternit "

    private let open: @MainActor (URL) async -> Bool
    private let defaults: UserDefaults
    private var pending: [String: Pending] = [:]

    private static let knownKey = "phoneUse.shortcuts.known"

    init(defaults: UserDefaults = .standard,
         open: @escaping @MainActor (URL) async -> Bool = { url in await UIApplication.shared.open(url) }) {
        self.defaults = defaults
        self.open = open
    }

    var pendingCount: Int { pending.count }
    var pendingIds: [String] { Array(pending.keys) }

    /// Shortcut names that returned a result on this phone at least once.
    /// Apple gives apps no list of installed shortcuts, so this is "known",
    /// not "installed": a name is dropped again when Shortcuts says it's missing.
    var knownShortcuts: [String] { (defaults.stringArray(forKey: Self.knownKey) ?? []).sorted() }

    // MARK: - Run

    /// Runs `name` with `input` and returns the contract envelope.
    func run(name rawName: String, input: Any?, timeoutSeconds: Int = ShortcutBridge.defaultTimeout) async -> [String: Any] {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.count <= 200 else {
            return PhoneUseEnvelope.failure("bad_params", "name is required (up to 200 characters).")
        }
        guard (1...120).contains(timeoutSeconds) else {
            return PhoneUseEnvelope.failure("bad_params", "timeout_s must be 1-120.")
        }
        guard pending.count < Self.maxPending else {
            return PhoneUseEnvelope.failure("busy", "Other shortcuts are still running on the iPhone. Try again when they finish.")
        }
        let requestId = UUID().uuidString.lowercased()
        let text: String?
        switch Self.inputText(input, name: name, requestId: requestId) {
        case .none: text = nil
        case .text(let t): text = t
        case .tooLarge:
            return PhoneUseEnvelope.failure("bad_params", "input is too large for a shortcut (over 32 KB).")
        }
        guard let url = Self.runURL(name: name, text: text, requestId: requestId) else {
            return PhoneUseEnvelope.failure("bad_params", "That shortcut name can't be sent to Shortcuts.")
        }
        let wait = max(1, TimeInterval(timeoutSeconds) - Self.runtimeMargin)
        let outcome: Outcome = await withCheckedContinuation { continuation in
            self.pending[requestId] = Pending(name: name, continuation: continuation, timer: nil)
            self.pending[requestId]?.timer = Task { [weak self] in
                try? await Task.sleep(for: .seconds(wait))
                guard !Task.isCancelled else { return }
                self?.finish(requestId, .timedOut)
            }
            Task { [weak self] in
                guard let self else { return }
                if await !self.open(url) { self.finish(requestId, .couldNotOpen) }
            }
        }
        return Self.envelope(outcome, name: name, timeoutSeconds: timeoutSeconds)
    }

    // MARK: - Answers

    /// The x-callback URL, from the app's URL handler. True when the URL was
    /// a shortcut callback (handled or stale), so nothing else opens it.
    @discardableResult
    func handle(url: URL) -> Bool {
        guard let callback = Self.parseCallback(url) else { return false }
        if let id = callback.requestId { finish(id, callback.outcome) }
        return true
    }

    /// `ReturnShortcutResultIntent`. With no id, the answer goes to the only
    /// pending run; with several running it can't be told apart and is dropped.
    @discardableResult
    func deliver(result: String?, requestId: String?) -> Bool {
        if let id = requestId?.trimmingCharacters(in: .whitespacesAndNewlines), !id.isEmpty {
            return finish(id.lowercased(), .output(result, .appIntent))
        }
        guard pending.count == 1, let only = pending.keys.first else { return false }
        return finish(only, .output(result, .appIntent))
    }

    @discardableResult
    private func finish(_ id: String, _ outcome: Outcome) -> Bool {
        guard let entry = pending.removeValue(forKey: id) else { return false }
        entry.timer?.cancel()
        switch outcome {
        case .output: remember(entry.name, known: true)
        case .failed(let message) where Self.isNotInstalled(message): remember(entry.name, known: false)
        default: break
        }
        entry.continuation.resume(returning: outcome)
        return true
    }

    private func remember(_ name: String, known: Bool) {
        var names = Set(defaults.stringArray(forKey: Self.knownKey) ?? [])
        if known { names.insert(name) } else { names.remove(name) }
        defaults.set(Array(names).sorted(), forKey: Self.knownKey)
    }

    // MARK: - Pure parts (tested)

    enum InputText: Equatable {
        case none
        case text(String)
        case tooLarge
    }

    /// Strings go as they are; objects, arrays, numbers and booleans as JSON
    /// text (contract §3.1). Allternit's own shortcuts get `request_id` added
    /// to an object input so their last step can return to this exact run.
    nonisolated static func inputText(_ input: Any?, name: String, requestId: String) -> InputText {
        let text: String
        switch input {
        case nil, is NSNull:
            return .none
        case let s as String:
            text = s
        case let object as [String: Any]:
            var object = object
            if name.hasPrefix(ownPrefix) { object["request_id"] = requestId }
            guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return .tooLarge }
            text = String(decoding: data, as: UTF8.self)
        default:
            guard let value = input,
                  let data = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys]) else {
                return .tooLarge
            }
            text = String(decoding: data, as: UTF8.self)
        }
        return text.utf8.count > maxInputBytes ? .tooLarge : .text(text)
    }

    /// Percent-encodes everything but RFC 3986 unreserved characters: the
    /// x-success value is itself a URL with `?` and `&` that must survive.
    nonisolated static func encode(_ value: String) -> String {
        var allowed = CharacterSet.alphanumerics.intersection(CharacterSet(charactersIn: Unicode.Scalar(0)...Unicode.Scalar(127)))
        allowed.insert(charactersIn: "-._~")
        return value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value
    }

    nonisolated static func callbackURL(requestId: String, status: String) -> String {
        "\(callbackScheme)://\(callbackHost)?request=\(encode(requestId))&status=\(status)"
    }

    nonisolated static func runURL(name: String, text: String?, requestId: String) -> URL? {
        var items: [(String, String)] = [("name", name)]
        if let text {
            items.append(("input", "text"))
            items.append(("text", text))
        }
        items.append(("x-success", callbackURL(requestId: requestId, status: "success")))
        items.append(("x-error", callbackURL(requestId: requestId, status: "error")))
        items.append(("x-cancel", callbackURL(requestId: requestId, status: "cancel")))
        let query = items.map { "\($0.0)=\(encode($0.1))" }.joined(separator: "&")
        return URL(string: "shortcuts://x-callback-url/run-shortcut?\(query)")
    }

    struct Callback: Equatable {
        /// nil when the link had no id: swallowed, never matched to a run.
        let requestId: String?
        let outcome: Outcome
    }

    /// `allternit://shortcut-result?request=<id>&status=success|error|cancel`,
    /// plus `result` / `errorMessage` that Shortcuts appends.
    nonisolated static func parseCallback(_ url: URL) -> Callback? {
        guard url.scheme?.lowercased() == callbackScheme, url.host?.lowercased() == callbackHost else { return nil }
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        func value(_ name: String) -> String? { items.first { $0.name == name }?.value }
        let id = value("request").map { $0.lowercased() }.flatMap { $0.isEmpty ? nil : $0 }
        let outcome: Outcome
        switch value("status") {
        case "success": outcome = .output(value("result"), .xCallback)
        case "cancel": outcome = .cancelled
        default: outcome = .failed(value("errorMessage") ?? "")
        }
        return Callback(requestId: id, outcome: outcome)
    }

    /// Shortcuts' x-error text when the name doesn't exist. Not checked on a
    /// real iPhone yet (plan P6); English wording only.
    nonisolated static func isNotInstalled(_ message: String) -> Bool {
        let m = message.lowercased()
        return m.contains("not found") || m.contains("couldn’t be found") || m.contains("couldn't be found")
            || m.contains("could not find") || m.contains("couldn’t find") || m.contains("couldn't find")
            || m.contains("no shortcut")
    }

    /// A JSON object or array comes back as itself; anything else as text.
    nonisolated static func output(_ text: String?) -> Any {
        guard let text else { return NSNull() }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("{") || trimmed.hasPrefix("["),
           let object = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8)) {
            return object
        }
        return text
    }

    /// Contract §4 envelope for each outcome.
    nonisolated static func envelope(_ outcome: Outcome, name: String, timeoutSeconds: Int = defaultTimeout) -> [String: Any] {
        switch outcome {
        case .output(let text, let via):
            return PhoneUseEnvelope.ok(["output": output(text), "returned_via": via.rawValue])
        case .failed(let message) where isNotInstalled(message):
            return PhoneUseEnvelope.needsYou(.shortcutNotInstalled, name: name, error: "shortcut_not_installed")
        case .failed(let message):
            let why = message.isEmpty ? "The \"\(name)\" shortcut stopped with an error." : "The \"\(name)\" shortcut stopped: \(message)"
            return PhoneUseEnvelope.failure("shortcut_failed", why)
        case .cancelled:
            return PhoneUseEnvelope.failure("cancelled", "The \"\(name)\" shortcut was cancelled on the iPhone.")
        case .timedOut:
            // No answer at all: most often the shortcut isn't there or doesn't
            // end with Return Result to Allternit.
            return PhoneUseEnvelope.needsYou(.shortcutNotInstalled, name: name, error: "timeout", extra: [
                "timeout_s": timeoutSeconds,
                "hint": "If \"\(name)\" is installed, make its last step Return Result to Allternit.",
            ])
        case .couldNotOpen:
            return PhoneUseEnvelope.failure("shortcuts_unavailable", "The Shortcuts app is not available on this iPhone. Install it from the App Store.")
        }
    }
}
