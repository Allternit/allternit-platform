import Foundation
import Security

/// A computer the File Provider shows as one location (one
/// `NSFileProviderDomain`). `id` is cloud-api's runtime device id, the
/// `computer_id` of every Files route (SPEC section 2, "Computer ids").
public struct FilesSharedComputer: Codable, Sendable, Hashable, Identifiable {
    public var id: ComputerId
    public var name: String
    public var runtimeType: String?

    public init(id: ComputerId, name: String, runtimeType: String? = nil) {
        self.id = id
        self.name = name
        self.runtimeType = runtimeType
    }
}

/// A wake prompt the extension could not show itself (setting `ask`): the
/// File Provider UI sheet (step 20) or the app reads it and asks the user.
public struct FilesPendingWake: Codable, Sendable, Hashable {
    public var computerId: ComputerId
    public var itemId: ItemId?
    public var reason: WakeReason
    public var hint: WakeHint?
    public var at: Date

    public init(computerId: ComputerId, itemId: ItemId?, reason: WakeReason, hint: WakeHint?, at: Date = Date()) {
        self.computerId = computerId
        self.itemId = itemId
        self.reason = reason
        self.hint = hint
        self.at = at
    }
}

/// A protected folder the extension could not open (`protected_root_locked`).
public struct FilesPendingUnlock: Codable, Sendable, Hashable {
    public var computerId: ComputerId
    public var rootId: RootId?
    public var rootName: String?
    public var at: Date

    public init(computerId: ComputerId, rootId: RootId?, rootName: String?, at: Date = Date()) {
        self.computerId = computerId
        self.rootId = rootId
        self.rootName = rootName
        self.at = at
    }
}

/// State the app shares with the File Provider extension (and later the
/// UI extension). Config goes in the App Group's defaults; secrets (the
/// Files token, wake tickets) go in the shared keychain group, readable
/// after first unlock, this device only — the same split as the widget and
/// notification extensions use (`SharedAccess`, `AppGroupStore`).
public final class FilesSharedStore: @unchecked Sendable {
    public let appGroup: String
    public let keychainGroup: String?
    private let defaults: UserDefaults?

    private enum Key {
        static let cloudBaseURL = "files.cloudBaseURL"
        static let deviceId = "files.deviceId"
        static let deviceName = "files.deviceName"
        static let computers = "files.computers.v1"
        static let tokenRejected = "files.tokenRejected"
        static let pins = "files.pins.v1"
        static let pendingWake = "files.pendingWake.v1"
        static let pendingUnlock = "files.pendingUnlock.v1"
    }

    private static let keychainService = "com.allternit.mobile.files"
    private static let tokenAccount = "files-token"
    private static let tokenKeyIdAccount = "files-token-key-id"

    /// `keychainGroup` is the full access group with the team prefix
    /// (Info.plist `AllternitSharedKeychainGroup`).
    public init(appGroup: String, keychainGroup: String?) {
        self.appGroup = appGroup
        self.keychainGroup = keychainGroup
        self.defaults = UserDefaults(suiteName: appGroup)
    }

    /// Reads `AllternitAppGroup` and `AllternitSharedKeychainGroup` from the
    /// running target's Info.plist (both set in project.yml / the plists).
    public static func fromMainBundle(defaultAppGroup: String = "group.com.allternit.mobile") -> FilesSharedStore {
        let info = Bundle.main.infoDictionary ?? [:]
        let group = (info["AllternitAppGroup"] as? String).flatMap { $0.isEmpty || $0.hasPrefix("$(") ? nil : $0 } ?? defaultAppGroup
        let keychain = (info["AllternitSharedKeychainGroup"] as? String).flatMap { $0.isEmpty || $0.hasPrefix("$(") ? nil : $0 }
        return FilesSharedStore(appGroup: group, keychainGroup: keychain)
    }

    /// The App Group container (nil when the target lacks the entitlement).
    public var containerURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
    }

    /// Per-computer scratch space for envelopes, upload journals and partial
    /// downloads: `<group>/Files/<computer>/`, protected `completeUnlessOpen`.
    public func scratchDirectory(for computerId: ComputerId) -> URL {
        let base = containerURL ?? FileManager.default.temporaryDirectory
        let safe = computerId.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" }
        let url = base.appendingPathComponent("Files", isDirectory: true).appendingPathComponent(String(safe), isDirectory: true)
        if !FileManager.default.fileExists(atPath: url.path) {
            try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            FilesProtection.protectDirectory(url)
        }
        return url
    }

    // MARK: Config (App Group defaults)

    /// cloud-api origin without `/api/v1` (for example https://api.allternit.com).
    public var cloudBaseURL: URL {
        get { defaults?.string(forKey: Key.cloudBaseURL).flatMap(URL.init(string:)) ?? URL(string: "https://api.allternit.com")! }
        set { defaults?.set(newValue.absoluteString, forKey: Key.cloudBaseURL) }
    }

    /// `X-Allternit-Device-Id` / `device_id` for pins and wake consent.
    public var deviceId: String? {
        get { defaults?.string(forKey: Key.deviceId) }
        set { defaults?.set(newValue, forKey: Key.deviceId) }
    }

    /// Names conflict copies (`Lease (conflict iPhone 2026-11-15 1042).pdf`).
    public var deviceName: String {
        get { defaults?.string(forKey: Key.deviceName) ?? "iPhone" }
        set { defaults?.set(newValue, forKey: Key.deviceName) }
    }

    /// The computers that have a File Provider domain.
    public var computers: [FilesSharedComputer] {
        get {
            guard let data = defaults?.data(forKey: Key.computers) else { return [] }
            return (try? JSONDecoder().decode([FilesSharedComputer].self, from: data)) ?? []
        }
        set { defaults?.set(try? JSONEncoder().encode(newValue), forKey: Key.computers) }
    }

    public func computer(_ id: ComputerId) -> FilesSharedComputer? {
        computers.first { $0.id == id }
    }

    /// Set by the extension when cloud-api refused the token (401); the app
    /// mints a new one on its next sync.
    public var tokenRejected: Bool {
        get { defaults?.bool(forKey: Key.tokenRejected) ?? false }
        set { defaults?.set(newValue, forKey: Key.tokenRejected) }
    }

    // MARK: Offline pins (this device)

    /// Items kept offline on this device, as `<computer>/<item>`.
    public var pins: Set<String> {
        get { Set(defaults?.stringArray(forKey: Key.pins) ?? []) }
        set { defaults?.set(Array(newValue).sorted(), forKey: Key.pins) }
    }

    public func isPinned(computerId: ComputerId, itemId: ItemId) -> Bool {
        pins.contains("\(computerId)/\(itemId)")
    }

    public func setPinned(_ pinned: Bool, computerId: ComputerId, itemId: ItemId) {
        var all = pins
        if pinned { all.insert("\(computerId)/\(itemId)") } else { all.remove("\(computerId)/\(itemId)") }
        pins = all
    }

    // MARK: Pending wake prompts

    public var pendingWakes: [FilesPendingWake] {
        get {
            guard let data = defaults?.data(forKey: Key.pendingWake) else { return [] }
            return (try? JSONDecoder().decode([FilesPendingWake].self, from: data)) ?? []
        }
        set { defaults?.set(try? JSONEncoder().encode(newValue), forKey: Key.pendingWake) }
    }

    public func addPendingWake(_ wake: FilesPendingWake) {
        var all = pendingWakes.filter { $0.computerId != wake.computerId && Date().timeIntervalSince($0.at) < 600 }
        all.append(wake)
        pendingWakes = all
    }

    public func clearPendingWake(computerId: ComputerId) {
        pendingWakes = pendingWakes.filter { $0.computerId != computerId }
    }

    // MARK: Pending unlock prompts (protected roots)

    /// Protected folders the extension could not open because they need a
    /// fresh device unlock. The File Provider UI sheet reads it and asks for
    /// Face ID.
    public var pendingUnlocks: [FilesPendingUnlock] {
        get {
            guard let data = defaults?.data(forKey: Key.pendingUnlock) else { return [] }
            return (try? JSONDecoder().decode([FilesPendingUnlock].self, from: data)) ?? []
        }
        set { defaults?.set(try? JSONEncoder().encode(newValue), forKey: Key.pendingUnlock) }
    }

    public func addPendingUnlock(_ unlock: FilesPendingUnlock) {
        var all = pendingUnlocks.filter { $0.computerId != unlock.computerId && Date().timeIntervalSince($0.at) < 600 }
        all.append(unlock)
        pendingUnlocks = all
    }

    /// The latest unlock prompt for `computerId`, if it is under 10 minutes old.
    public func pendingUnlock(computerId: ComputerId, now: Date = Date()) -> FilesPendingUnlock? {
        pendingUnlocks.last { $0.computerId == computerId && now.timeIntervalSince($0.at) < 600 }
    }

    public func clearPendingUnlock(computerId: ComputerId) {
        pendingUnlocks = pendingUnlocks.filter { $0.computerId != computerId }
    }

    /// The latest wake prompt for `computerId`, if it is under 10 minutes old.
    public func pendingWake(computerId: ComputerId, now: Date = Date()) -> FilesPendingWake? {
        pendingWakes.last { $0.computerId == computerId && now.timeIntervalSince($0.at) < 600 }
    }

    // MARK: Device unlock (protected roots, SPEC section 12)

    /// How long one Face ID unlock opens protected roots on a computer.
    public static let unlockWindow: TimeInterval = 300

    /// Records a fresh device unlock (Face ID, Touch ID or the passcode) for
    /// `computerId`. Requests to that computer then carry
    /// `X-Allternit-Unlock` for `unlockWindow` seconds.
    public func markUnlocked(computerId: ComputerId, at date: Date = Date()) {
        writeSecret(String(Int(date.timeIntervalSince1970)), account: "unlock.\(computerId)")
        clearPendingUnlock(computerId: computerId)
    }

    /// The `X-Allternit-Unlock` value while an unlock is fresh, else nil:
    /// `bio1.<device id>.<unix seconds of the unlock>`, the same proof the
    /// Android app sends.
    public func unlockProof(computerId: ComputerId, now: Date = Date()) -> String? {
        guard let stored = readSecret("unlock.\(computerId)"), let at = TimeInterval(stored) else { return nil }
        guard now.timeIntervalSince1970 - at < Self.unlockWindow, now.timeIntervalSince1970 >= at - 60 else {
            deleteSecret("unlock.\(computerId)")
            return nil
        }
        return "bio1.\(deviceId ?? "dev_unknown").\(Int(at))"
    }

    public func clearUnlock(computerId: ComputerId) {
        deleteSecret("unlock.\(computerId)")
    }

    // MARK: Secrets (shared keychain group)

    /// The Files token: a cloud-api API key with the `compute` scope that
    /// the app mints for this phone (`POST /api/v1/api-keys`). The extension
    /// sends it as the Bearer on endpoints, wake consent and the relay.
    public func token() -> String? { readSecret(Self.tokenAccount) }

    /// The API key id behind `token()`, so sign-out can revoke it.
    public func tokenKeyId() -> String? { readSecret(Self.tokenKeyIdAccount) }

    @discardableResult
    public func setToken(_ token: String, keyId: String?) -> Bool {
        let ok = writeSecret(token, account: Self.tokenAccount)
        if let keyId { writeSecret(keyId, account: Self.tokenKeyIdAccount) } else { deleteSecret(Self.tokenKeyIdAccount) }
        if ok { tokenRejected = false }
        return ok
    }

    public func clearToken() {
        deleteSecret(Self.tokenAccount)
        deleteSecret(Self.tokenKeyIdAccount)
    }

    /// A wake ticket still valid for at least 5 s, or nil.
    public func wakeTicket(computerId: ComputerId, now: Date = Date()) -> String? {
        guard let stored = readSecret("wake.\(computerId)") else { return nil }
        let parts = stored.split(separator: " ", maxSplits: 1).map(String.init)
        guard parts.count == 2, let expiry = TimeInterval(parts[0]), expiry - 5 > now.timeIntervalSince1970 else {
            deleteSecret("wake.\(computerId)")
            return nil
        }
        return parts[1]
    }

    public func setWakeTicket(_ ticket: String, expiresAt: Date, computerId: ComputerId) {
        writeSecret("\(expiresAt.timeIntervalSince1970) \(ticket)", account: "wake.\(computerId)")
    }

    /// Sign-out: drops every account-bound value. Config (the cloud URL, the
    /// device id and name) stays.
    public func clearAccount() {
        clearToken()
        for computer in computers {
            deleteSecret("wake.\(computer.id)")
            deleteSecret("unlock.\(computer.id)")
        }
        computers = []
        pins = []
        pendingWakes = []
        pendingUnlocks = []
        tokenRejected = false
    }

    // MARK: Keychain

    private func query(_ account: String) -> [CFString: Any] {
        var q: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: Self.keychainService,
            kSecAttrAccount: account,
        ]
        if let keychainGroup { q[kSecAttrAccessGroup] = keychainGroup }
        return q
    }

    private func readSecret(_ account: String) -> String? {
        var q = query(account)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data, let value = String(data: data, encoding: .utf8), !value.isEmpty else { return nil }
        return value
    }

    @discardableResult
    private func writeSecret(_ value: String, account: String) -> Bool {
        let data = Data(value.utf8)
        let q = query(account)
        let status = SecItemUpdate(q as CFDictionary, [kSecValueData: data] as CFDictionary)
        if status == errSecSuccess { return true }
        guard status == errSecItemNotFound else { return false }
        var item = q
        item[kSecValueData] = data
        item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    private func deleteSecret(_ account: String) {
        SecItemDelete(query(account) as CFDictionary)
    }
}
