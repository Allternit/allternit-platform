import Foundation

/// Share-extension outbox (plan step I7). The extension never sends: it
/// writes one `Item` per share here, and the app drains the folder on launch
/// and foreground (`Core/ShareOutboxDrainer.swift`).
///
/// Layout in the App Group container: `Outbox/<id>.json` per item (so the
/// extension and the app never rewrite a shared index) and
/// `Outbox/files/<id>/<name>` for copied files. Consent and the bot-provider
/// directory live in the group's defaults. No app-only imports.
enum OutboxStore {
    /// Largest single file the share sheet accepts. Files go out as base64 in
    /// a JSON body (`AgentChatClient.upload`), so this stays small.
    static let maxFileBytes: Int64 = 10 * 1024 * 1024
    /// At most this many files in one share.
    static let maxFilesPerItem = 10

    struct FileRef: Codable, Sendable, Equatable {
        var name: String
        var mediaType: String
        var size: Int64
        /// Path under `Outbox/files/`.
        var relativePath: String
        /// Set once uploaded, so a retry after a failed send skips the upload.
        var uploadedURL: String?
    }

    struct Item: Codable, Sendable, Equatable, Identifiable {
        var id: String
        var createdAt: Date
        var botId: String
        var botName: String
        /// Shared text, or the shared link (inline).
        var text: String?
        var url: String?
        /// What the person typed in the share sheet.
        var note: String?
        var files: [FileRef] = []
        var attempts: Int = 0
    }

    private enum Key {
        static let consentPrefix = "surfaces.share.consent."
        static let providers = "surfaces.share.botProviders"
    }

    // MARK: Folders

    private static var root: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier)?
            .appendingPathComponent("Outbox", isDirectory: true)
    }

    static var filesRoot: URL? { root?.appendingPathComponent("files", isDirectory: true) }

    /// A fresh folder for one item's files.
    static func filesFolder(for id: String) -> URL? {
        guard let dir = filesRoot?.appendingPathComponent(id, isDirectory: true) else { return nil }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static func fileURL(_ ref: FileRef) -> URL? {
        filesRoot?.appendingPathComponent(ref.relativePath)
    }

    // MARK: Items

    /// Writes (or replaces) an item. False when the container is missing.
    @discardableResult
    static func save(_ item: Item) -> Bool {
        guard let root else { return false }
        do {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            let encoder = JSONEncoder()
            encoder.dateEncodingStrategy = .iso8601
            try encoder.encode(item).write(to: root.appendingPathComponent("\(item.id).json"), options: .atomic)
            return true
        } catch { return false }
    }

    /// Items waiting to go, oldest first.
    static func pending() -> [Item] {
        guard let root,
              let names = try? FileManager.default.contentsOfDirectory(atPath: root.path) else { return [] }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return names.filter { $0.hasSuffix(".json") }.compactMap { name in
            guard let data = try? Data(contentsOf: root.appendingPathComponent(name)) else { return nil }
            return try? decoder.decode(Item.self, from: data)
        }.sorted { $0.createdAt < $1.createdAt }
    }

    /// Removes an item and its copied files (after a confirmed send).
    static func remove(_ item: Item) {
        guard let root else { return }
        try? FileManager.default.removeItem(at: root.appendingPathComponent("\(item.id).json"))
        try? FileManager.default.removeItem(at: root.appendingPathComponent("files/\(item.id)", isDirectory: true))
    }

    /// Sign-out: drop every item and file.
    static func clearAll() {
        guard let root else { return }
        try? FileManager.default.removeItem(at: root)
        AppGroup.defaults?.removeObject(forKey: Key.providers)
        guard let defaults = AppGroup.defaults else { return }
        for key in defaults.dictionaryRepresentation().keys where key.hasPrefix(Key.consentPrefix) {
            defaults.removeObject(forKey: key)
        }
    }

    // MARK: Consent (per bot: a different bot may run on a different provider)

    static func hasConsent(botId: String) -> Bool {
        AppGroup.defaults?.bool(forKey: Key.consentPrefix + botId) ?? false
    }

    static func recordConsent(botId: String) {
        AppGroup.defaults?.set(true, forKey: Key.consentPrefix + botId)
    }

    // MARK: Bot model providers

    /// `botId` to model provider name. The status snapshot's `bots` rows carry
    /// no provider, so the app writes it here when it drains.
    static var botProviders: [String: String] {
        get { (AppGroup.defaults?.dictionary(forKey: Key.providers) as? [String: String]) ?? [:] }
        set { AppGroup.defaults?.set(newValue, forKey: Key.providers) }
    }
}
