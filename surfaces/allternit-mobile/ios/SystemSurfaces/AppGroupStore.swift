import Foundation

/// Latest status snapshot and small flags, kept in the App Group so the
/// widget, notification and share extensions read what the app last saw.
/// JSON in `UserDefaults(suiteName:)`. No app-only imports.
enum AppGroupStore {
    private enum Key {
        static let snapshot = "surfaces.snapshot.v1"
        static let signedIn = "surfaces.signedIn"
        static let hasComputer = "surfaces.hasComputer"
        static let frontFace = "surfaces.widget.frontFace"
        static let flippedPrefix = "surfaces.widget.flippedAt."
        static let cloudBaseURL = "surfaces.cloudBaseURL"
    }

    /// Used when the app has not written a base URL yet.
    static let defaultCloudBaseURL = "https://api.allternit.com"

    private static var defaults: UserDefaults? { AppGroup.defaults }

    // MARK: Snapshot

    /// The stored snapshot, with `asOf` set to when it was stored.
    static var snapshot: StatusSnapshot? {
        guard let data = defaults?.data(forKey: Key.snapshot) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try? decoder.decode(StatusSnapshot.self, from: data)
    }

    /// Stores `snapshot`, stamping `asOf` and dropping `focus.question` when
    /// `focus.private` is true.
    static func saveSnapshot(_ snapshot: StatusSnapshot, now: Date = Date()) {
        var stored = snapshot.redactedForStorage()
        stored.asOf = now
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        guard let data = try? encoder.encode(stored) else { return }
        defaults?.set(data, forKey: Key.snapshot)
    }

    static func clearSnapshot() { defaults?.removeObject(forKey: Key.snapshot) }

    // MARK: Flags

    static var signedIn: Bool {
        get { defaults?.bool(forKey: Key.signedIn) ?? false }
        set { defaults?.set(newValue, forKey: Key.signedIn) }
    }

    static var hasComputer: Bool {
        get { defaults?.bool(forKey: Key.hasComputer) ?? false }
        set { defaults?.set(newValue, forKey: Key.hasComputer) }
    }

    // MARK: Widget face state

    /// The configured front face ("status" or "actions").
    static var frontFace: String {
        get { defaults?.string(forKey: Key.frontFace) ?? "status" }
        set { defaults?.set(newValue, forKey: Key.frontFace) }
    }

    /// When the widget in `slot` (family plus front face) was last flipped.
    static func flippedAt(slot: String) -> Date? {
        let t = defaults?.double(forKey: Key.flippedPrefix + slot) ?? 0
        return t > 0 ? Date(timeIntervalSince1970: t) : nil
    }

    static func setFlippedAt(_ date: Date?, slot: String) {
        let key = Key.flippedPrefix + slot
        if let date { defaults?.set(date.timeIntervalSince1970, forKey: key) }
        else { defaults?.removeObject(forKey: key) }
    }

    // MARK: Cloud base URL

    /// The app writes `AppConfig.cloudAPIBaseURL` here at launch; extensions
    /// read it (they cannot import app code).
    static var cloudBaseURL: URL {
        get {
            let s = defaults?.string(forKey: Key.cloudBaseURL) ?? defaultCloudBaseURL
            return URL(string: s) ?? URL(string: defaultCloudBaseURL)!
        }
        set { defaults?.set(newValue.absoluteString, forKey: Key.cloudBaseURL) }
    }

    // MARK: Sign-out

    /// Removes everything this store wrote (the base URL stays: it is config).
    static func clearAll() {
        guard let defaults else { return }
        for key in defaults.dictionaryRepresentation().keys where key.hasPrefix("surfaces.") && key != Key.cloudBaseURL {
            defaults.removeObject(forKey: key)
        }
    }
}
