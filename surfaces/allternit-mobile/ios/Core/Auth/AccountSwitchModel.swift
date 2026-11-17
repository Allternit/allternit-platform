import Foundation

// Pure parts of the account switch (phone calm step 31, plan contract C7;
// web `src/lib/accounts/switch-account.ts`, PR #1160). No Clerk, no UI:
// key naming, the one-time Keychain move, the cached plan labels (D14) and
// the switch order. `AccountSwitch` / `AccountSwitchCleanup` run them.

/// Why the account switch cleanup runs.
enum AccountSwitchReason: String, Equatable, Sendable {
    /// Same person, another of their organizations (or personal).
    case organizationChanged
    /// Another person (Clerk multi-session; also a Clerk session that came
    /// back as someone else without a sign-out).
    case personChanged

    /// Per-person data (the runtime device credential) is unloaded only when
    /// the person changes.
    var clearsPersonData: Bool { self == .personChanged }
}

// MARK: - Secret storage

/// The Keychain calls the credential code needs, so the per-user naming and
/// the one-time move can be tested with an in-memory store.
protocol SecretStore {
    func load(_ account: String) -> String?
    func save(_ value: String, for account: String)
    func delete(_ account: String)
}

/// The app's Keychain (`KeychainHelper`, service com.allternit.mobile).
struct KeychainSecretStore: SecretStore {
    func load(_ account: String) -> String? { KeychainHelper.load(account) }
    func save(_ value: String, for account: String) { KeychainHelper.save(value, for: account) }
    func delete(_ account: String) { KeychainHelper.delete(account) }
}

// MARK: - Runtime device credential keys

/// Keychain naming for the runtime device credential (`RuntimePairing`): one
/// item per Clerk user id. Before step 31 there was one unkeyed item.
enum RuntimeCredentialKeys {
    /// The unkeyed item every build before step 31 wrote.
    static let legacy = "allternit-ios-runtime-session"

    /// UserDefaults: the Clerk user id whose credential was loaded last, so
    /// a cold launch finds it before Clerk restores its session. An id, not
    /// a secret.
    static let activeUserDefaultsKey = "allternit-ios-runtime-session-user"

    /// `allternit-ios-runtime-session.<clerk user id>`.
    static func key(forUserId userId: String) -> String {
        "\(legacy).\(userId)"
    }

    /// Whether the unkeyed item may move to `userId`'s key. It may unless it
    /// names a different Clerk user (`user_…`). Sign-out always deleted the
    /// unkeyed item, so it belongs to whoever is signed in now; an item from
    /// a cloud build with a non-Clerk user id is taken as theirs too.
    static func legacyBelongs(to userId: String, json: String) -> Bool {
        guard let data = json.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let owner = object["userId"] as? String,
              owner.hasPrefix("user_") else { return true }
        return owner == userId
    }

    /// The stored credential JSON for `userId`. When that person has no
    /// item yet and the unkeyed item is theirs, it is copied to their key
    /// first and removed only after the copy reads back the same, so a
    /// working pairing is never dropped.
    static func loadMigrating(userId: String, store: SecretStore) -> String? {
        let userKey = key(forUserId: userId)
        if let stored = store.load(userKey) { return stored }
        guard let legacyJSON = store.load(legacy),
              legacyBelongs(to: userId, json: legacyJSON) else { return nil }
        store.save(legacyJSON, for: userKey)
        guard store.load(userKey) == legacyJSON else {
            // The copy failed: keep using the unkeyed item, try again next time.
            return legacyJSON
        }
        store.delete(legacy)
        return legacyJSON
    }
}

// MARK: - Cached plan labels (D14)

/// Plan label per account row, written while that account is active and read
/// for the rows that are not. No token for an inactive account is ever asked
/// for. Same key and row key as the web (`ACCOUNT_PLAN_LABELS_KEY`,
/// `accountPlanKey`).
enum AccountPlanLabels {
    static let defaultsKey = "allternit-account-plan-labels-v1"

    /// One row = one person and, optionally, one of their organizations.
    static func rowKey(userId: String, orgId: String?) -> String {
        "\(userId):\(orgId ?? "personal")"
    }

    static func all(defaults: UserDefaults = .standard) -> [String: String] {
        (defaults.dictionary(forKey: defaultsKey) as? [String: String]) ?? [:]
    }

    /// Saves the active account's plan label (nil or empty removes it).
    static func remember(userId: String, orgId: String?, planName: String?,
                         defaults: UserDefaults = .standard) {
        var labels = all(defaults: defaults)
        let key = rowKey(userId: userId, orgId: orgId)
        if let planName, !planName.isEmpty { labels[key] = planName } else { labels.removeValue(forKey: key) }
        defaults.set(labels, forKey: defaultsKey)
    }

    /// The label last seen while this account was active, or nil.
    static func read(userId: String, orgId: String?, defaults: UserDefaults = .standard) -> String? {
        all(defaults: defaults)[rowKey(userId: userId, orgId: orgId)]
    }

    /// Sign-out: one person's labels go.
    static func forget(userId: String, defaults: UserDefaults = .standard) {
        let labels = all(defaults: defaults).filter { !$0.key.hasPrefix("\(userId):") }
        defaults.set(labels, forKey: defaultsKey)
    }
}

// MARK: - Accounts card rows

/// One Accounts card row: the signed-in person (orgId nil) or one of their
/// organizations.
struct AccountRowModel: Identifiable, Equatable, Sendable {
    let orgId: String?
    let name: String
    let detail: String?
    let isActive: Bool

    var id: String { orgId ?? "personal" }
}

/// The card's rows. The active row shows the live plan; every other row its
/// cached label (D14), else "Personal" / "Organization".
func accountRows(personName: String,
                 personFallbackDetail: String?,
                 organizations: [(id: String, name: String)],
                 activeOrgId: String?,
                 livePlanName: String?,
                 cachedPlan: (String?) -> String?) -> [AccountRowModel] {
    func detail(orgId: String?, kind: String, fallback: String?) -> String {
        let plan = orgId == activeOrgId ? (livePlanName ?? cachedPlan(orgId)) : cachedPlan(orgId)
        if let plan, !plan.isEmpty { return "\(kind) · \(plan)" }
        if let fallback, !fallback.isEmpty { return "\(kind) · \(fallback)" }
        return kind
    }
    var rows = [AccountRowModel(orgId: nil, name: personName,
                                detail: detail(orgId: nil, kind: "Personal", fallback: personFallbackDetail),
                                isActive: activeOrgId == nil)]
    for org in organizations {
        rows.append(AccountRowModel(orgId: org.id, name: org.name,
                                    detail: detail(orgId: org.id, kind: "Organization", fallback: nil),
                                    isActive: activeOrgId == org.id))
    }
    return rows
}

// MARK: - Switch order

/// The steps of one switch, passed in so the order can be tested without
/// Clerk. `AccountSwitch.activate` fills them with the real calls.
struct AccountSwitchSteps {
    /// Clears the current account's local data (C7).
    var cleanup: @MainActor (AccountSwitchReason) async -> Void
    /// Clerk `setActive(sessionId:organizationId:)`.
    var setActive: @MainActor (_ sessionId: String, _ orgId: String?) async throws -> Void
    /// Reloads stores, push, widgets and Spotlight for whichever account is
    /// active afterwards.
    var rebuild: @MainActor () async -> Void
}

enum AccountSwitchOrder {
    /// C7 switch: cleanup first, then `setActive`, then rebuild. The rebuild
    /// runs even when Clerk refuses the switch, because the cleanup already
    /// dropped the cached data; Clerk keeps the current account then, and the
    /// error is passed on.
    @MainActor
    static func run(sessionId: String, orgId: String?, reason: AccountSwitchReason,
                    steps: AccountSwitchSteps) async throws {
        await steps.cleanup(reason)
        do {
            try await steps.setActive(sessionId, orgId)
        } catch {
            await steps.rebuild()
            throw error
        }
        await steps.rebuild()
    }
}
