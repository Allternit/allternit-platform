import Foundation

/// The iPhone side of the `settings` tool (Primitives Wave D, D2;
/// contracts/settings/README.md). Scopes: `account` keys are applied by the
/// server (this applier writes them only for Confirm and Undo), `device`
/// keys by the app that sent the turn, `runtime` keys (gizzi.*) by Gizzi Code
/// and never here. Device keys come back `apply_on_device`:
/// the client that sent the turn applies them once per tool-call id. This
/// applier keeps them where this app already does (AppearanceStore,
/// NotificationPrefs), records each call id it applied with the value it
/// replaced (for Undo), and writes the account keys this app already has a
/// client for under the user's own sign-in (Confirm and Undo).
/// A key with no store here falls back to "Open Settings".
@MainActor
enum AgentSettingsApplier {
    /// UserDefaults: `[{id: "<toolCallId>|<key>", prev: <value>?}]`, newest last.
    private static let appliedKey = "allternit-agent-settings-applied"
    private static let appliedLimit = 200

    // MARK: Device keys

    /// Keys this iPhone keeps itself.
    static func canApplyOnDevice(_ key: String) -> Bool {
        switch key {
        case "appearance.theme", "appearance.chat_font", "appearance.reduce_motion",
             "notifications.response", "notifications.approvals", "notifications.sound":
            return true
        default:
            return false
        }
    }

    /// The value this iPhone has now, nil for keys it doesn't keep.
    static func deviceValue(_ key: String) -> AgentSettingValue? {
        let store = AppearanceStore.shared
        switch key {
        case "appearance.theme": return .string(store.theme.rawValue)
        case "appearance.chat_font": return .string(store.chatFont.rawValue)
        case "appearance.reduce_motion": return .bool(store.reduceMotion)
        default:
            return notificationDefaultsKey(key).map { .bool(NotificationPrefs.isOn($0)) }
        }
    }

    /// Sets a device key. False when the key or value doesn't fit.
    @discardableResult
    static func setDevice(_ key: String, _ value: AgentSettingValue) -> Bool {
        let store = AppearanceStore.shared
        switch key {
        case "appearance.theme":
            guard let mode = value.stringValue.flatMap(AppearanceStore.Mode.init(rawValue:)) else { return false }
            store.theme = mode
        case "appearance.chat_font":
            guard let font = value.stringValue.flatMap(AppearanceStore.ChatFont.init(rawValue:)) else { return false }
            store.chatFont = font
        case "appearance.reduce_motion":
            guard let on = value.boolValue else { return false }
            store.reduceMotion = on
        default:
            guard let defaultsKey = notificationDefaultsKey(key), let on = value.boolValue else { return false }
            UserDefaults.standard.set(on, forKey: defaultsKey)
        }
        return true
    }

    /// contracts/settings/definitions.json `store.ios` for the notification keys.
    private static func notificationDefaultsKey(_ key: String) -> String? {
        switch key {
        case "notifications.response": return NotificationPrefs.responseKey
        case "notifications.approvals": return NotificationPrefs.approvalKey
        case "notifications.sound": return NotificationPrefs.soundKey
        default: return nil
        }
    }

    // MARK: Once per tool-call id

    enum Outcome: Equatable {
        /// Applied now, or by an earlier render of the same call.
        case applied(previous: AgentSettingValue?)
        /// This iPhone has no store for the key, or the value doesn't fit.
        case unsupported
    }

    /// Whether this call's change was applied here already.
    static func wasApplied(toolCallId: String, key: String) -> Bool {
        record(toolCallId: toolCallId, key: key) != nil
    }

    /// The value the call replaced, when this iPhone applied it.
    static func previous(toolCallId: String, key: String) -> AgentSettingValue? {
        record(toolCallId: toolCallId, key: key)?.previous
    }

    /// Applies an `apply_on_device` change once for its tool-call id; a
    /// second render (scroll, reload) finds the record and applies nothing.
    static func applyOnce(toolCallId: String, change: AgentSettingsResult.Change) -> Outcome {
        if let done = record(toolCallId: toolCallId, key: change.key) {
            return .applied(previous: done.previous)
        }
        guard canApplyOnDevice(change.key), let value = change.value else { return .unsupported }
        let before = deviceValue(change.key)
        guard setDevice(change.key, value) else { return .unsupported }
        remember(toolCallId: toolCallId, key: change.key, previous: before)
        return .applied(previous: before)
    }

    private struct Record { let previous: AgentSettingValue? }

    private static func entries() -> [[String: Any]] {
        UserDefaults.standard.array(forKey: appliedKey) as? [[String: Any]] ?? []
    }

    private static func record(toolCallId: String, key: String) -> Record? {
        let id = "\(toolCallId)|\(key)"
        guard let entry = entries().last(where: { $0["id"] as? String == id }) else { return nil }
        return Record(previous: AgentSettingValue(json: entry["prev"]))
    }

    private static func remember(toolCallId: String, key: String, previous: AgentSettingValue?) {
        var list = entries()
        var entry: [String: Any] = ["id": "\(toolCallId)|\(key)"]
        if let previous { entry["prev"] = plist(previous) }
        list.append(entry)
        if list.count > appliedLimit { list.removeFirst(list.count - appliedLimit) }
        UserDefaults.standard.set(list, forKey: appliedKey)
    }

    private static func plist(_ value: AgentSettingValue) -> Any {
        switch value {
        case .bool(let b): return b
        case .string(let s): return s
        case .number(let d): return d
        case .list(let l): return l
        }
    }

    // MARK: Account keys (the user's own sign-in)

    /// Account keys this app writes through an existing client
    /// (`PUT /api/v1/agent-preferences`, PreferencesClient).
    static func canWriteAccount(_ key: String) -> Bool {
        key == "chat.answer_style" || key == "instructions.custom"
    }

    enum WriteError: LocalizedError {
        case unsupported
        var errorDescription: String? { "This setting can't be changed from here. Open Settings to change it." }
    }

    /// Writes an account key with the user's session: Confirm on a
    /// `needs_confirmation` change, Undo on an `applied` one.
    static func writeAccount(_ key: String, _ value: AgentSettingValue) async throws {
        let client = PreferencesClient()
        let current = try await client.get()
        switch key {
        case "chat.answer_style":
            guard let style = value.stringValue.flatMap(ResponseStyle.init(rawValue:)) else { throw WriteError.unsupported }
            try await client.put(responseStyle: style.rawValue, customInstructions: current.customInstructions)
        case "instructions.custom":
            guard let text = value.stringValue else { throw WriteError.unsupported }
            try await client.put(responseStyle: current.responseStyle, customInstructions: text)
        default:
            throw WriteError.unsupported
        }
        // Settings shows the new value without waiting for the next launch.
        PreferencesStore.shared.fetchIfNeeded(force: true)
    }

    // MARK: Settings

    /// Opens the app's Settings. An Allternit web link (`/settings/<section>`)
    /// goes through the same router as a Universal Link; a bare path
    /// (`settings/appearance`) opens Settings.
    static func openSettings(link: String?) {
        if let link, let url = URL(string: link), url.scheme != nil, DeepLinkRouter.shared.open(url: url) {
            return
        }
        let section = link.flatMap { path -> String? in
            let parts = path.split(separator: "/").map(String.init)
            guard let at = parts.firstIndex(of: "settings"), at + 1 < parts.count else { return nil }
            return parts[at + 1]
        }
        DeepLinkRouter.shared.enqueue(.settings(section))
    }
}
