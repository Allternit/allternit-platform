import Foundation

// "Switch account" without Clerk multi-session (phone calm I14b, option B;
// web `src/lib/accounts/remembered-accounts.ts`, ai PR #1162). Pure parts:
// the list of accounts used on this device and the order of a person switch.
// No Clerk, no UI.
//
// The production Clerk instance runs one session at a time (multi-session is
// a paid add-on, not turned on). So this device remembers the accounts that
// signed in here, and switching to one is: the person-change cleanup
// (`AccountSwitchCleanup.run(reason: .personChanged)`), Clerk sign-out, then
// the sign-in screen with that account's email filled in. "Add another
// account" is the same with an empty sign-in.
//
// Security: only what Clerk already shows on screen is stored (user id, name,
// email, avatar URL, the cached plan label, when it was last used), in
// UserDefaults. No token, no session id. Signing in again still needs the
// password, code, passkey or social sign-in.

/// One account that signed in on this device.
struct RememberedAccount: Codable, Equatable, Identifiable, Sendable {
    let userId: String
    let name: String
    let email: String
    let imageUrl: String?
    /// Plan label last seen while this account was active (D14), or nil.
    var planLabel: String?
    /// When this account last signed in or was confirmed on this device.
    var lastUsedAt: Date

    var id: String { userId }

    /// The avatar letter when there is no photo.
    var initial: String {
        String((name.isEmpty ? email : name).prefix(1)).uppercased()
    }

    /// "email · plan", or the email alone.
    var detail: String {
        [email, planLabel].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
    }
}

/// The accounts remembered on this device. Same key, cap and rules as the web.
///
/// The list must outlive a person change, so `defaultsKey` is NOT cleared by
/// `AccountSwitchCleanup` or by sign-out: the list is what "Switch account"
/// shows right after a person change, and it holds no token or workspace
/// data. "Remove from this device" is the only way a row leaves (besides the
/// cap).
enum RememberedAccounts {
    /// Web `REMEMBERED_ACCOUNTS_KEY`.
    static let defaultsKey = "allternit-remembered-accounts-v1"
    /// Web `REMEMBERED_ACCOUNTS_MAX`: the oldest rows drop off past this.
    static let maxCount = 8
    /// Web `SWITCH_ACCOUNT_FAILED_TEXT`: a refused sign-out, one calm line.
    static let switchFailedText = "Couldn't sign out. Try again."

    /// True for a string shaped like an email address (not a full RFC check).
    static func isLikelyEmail(_ value: String?) -> Bool {
        guard let value, value.count <= 254 else { return false }
        let pattern = #"^[^\s@<>()\[\]\\,;:"]+@[^\s@<>()\[\]\\,;:"]+\.[^\s@<>()\[\]\\,;:"]+$"#
        return value.range(of: pattern, options: .regularExpression) != nil
    }

    /// Accounts remembered on this device, newest first, one row per user id.
    static func list(defaults: UserDefaults = .standard) -> [RememberedAccount] {
        guard let data = defaults.data(forKey: defaultsKey),
              let rows = try? JSONDecoder().decode([RememberedAccount].self, from: data) else { return [] }
        var seen = Set<String>()
        let valid = rows.filter { row in
            guard !row.userId.isEmpty, isLikelyEmail(row.email), !seen.contains(row.userId) else { return false }
            seen.insert(row.userId)
            return true
        }
        return valid.sorted { $0.lastUsedAt > $1.lastUsedAt }
    }

    /// The remembered accounts other than the one signed in now.
    static func others(excluding userId: String?, defaults: UserDefaults = .standard) -> [RememberedAccount] {
        list(defaults: defaults).filter { $0.userId != userId }
    }

    /// Saves (or refreshes) the signed-in account: one row per user id, moved
    /// to the top, at most `maxCount` rows. A missing plan label keeps the one
    /// already saved. An account with no email is not saved (sign-in could
    /// not be filled in for it). Web `rememberAccount`.
    static func remember(userId: String, name: String?, email: String?, imageUrl: String?,
                         planLabel: String? = nil,
                         defaults: UserDefaults = .standard, now: Date = Date()) {
        guard !userId.isEmpty, let email, isLikelyEmail(email) else { return }
        let rows = list(defaults: defaults)
        let previous = rows.first { $0.userId == userId }
        let trimmedName = name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let next = RememberedAccount(
            userId: userId,
            name: trimmedName.isEmpty ? String(email.split(separator: "@").first ?? Substring(email)) : trimmedName,
            email: email,
            imageUrl: (imageUrl?.isEmpty == false) ? imageUrl : nil,
            planLabel: (planLabel?.isEmpty == false) ? planLabel : previous?.planLabel,
            lastUsedAt: now
        )
        write([next] + rows.filter { $0.userId != userId }, defaults: defaults)
    }

    /// Updates the saved plan label of an account already on the list. Web
    /// `setRememberedAccountPlan`.
    static func setPlan(userId: String, planLabel: String?, defaults: UserDefaults = .standard) {
        guard let planLabel, !planLabel.isEmpty else { return }
        var rows = list(defaults: defaults)
        guard let index = rows.firstIndex(where: { $0.userId == userId }),
              rows[index].planLabel != planLabel else { return }
        rows[index].planLabel = planLabel
        write(rows, defaults: defaults)
    }

    /// "Remove from this device". Web `forgetRememberedAccount`.
    static func forget(userId: String, defaults: UserDefaults = .standard) {
        let rows = list(defaults: defaults)
        let kept = rows.filter { $0.userId != userId }
        guard kept.count != rows.count else { return }
        write(kept, defaults: defaults)
    }

    private static func write(_ rows: [RememberedAccount], defaults: UserDefaults) {
        guard let data = try? JSONEncoder().encode(Array(rows.prefix(maxCount))) else { return }
        defaults.set(data, forKey: defaultsKey)
    }
}

// MARK: - Person switch order

/// The steps of one person switch, passed in so the order can be tested
/// without Clerk. `AccountSwitch.switchPerson` fills them with the real calls.
struct PersonSwitchSteps {
    /// `AccountSwitchCleanup.run(reason: .personChanged)`: everything of the
    /// leaving person, while their session still works.
    var cleanup: @MainActor () async -> Void
    /// `AuthManager.signOut()` (Clerk sign-out). Throws when refused.
    var signOut: @MainActor () async throws -> Void
    /// Local resets once more after sign-out (no network), so a store that
    /// refilled while sign-out was in flight is empty again.
    var afterSignOut: @MainActor () -> Void
    /// Sign-out refused: Clerk keeps the current person, so reload their data.
    var rebuild: @MainActor () async -> Void
    /// The sign-in screen, with this email filled in (nil = empty sign-in).
    var showSignIn: @MainActor (_ email: String?) -> Void
}

enum PersonSwitchOrder {
    /// Cleanup first, then sign out, then sign-in. If sign-out is refused the
    /// error is thrown, the current person's data is rebuilt and sign-in is
    /// not shown; the caller shows one calm line. Web `signOutToSignIn`.
    @MainActor
    static func run(email: String?, steps: PersonSwitchSteps) async throws {
        let prefill = RememberedAccounts.isLikelyEmail(email) ? email : nil
        await steps.cleanup()
        do {
            try await steps.signOut()
        } catch {
            await steps.rebuild()
            throw error
        }
        steps.afterSignOut()
        steps.showSignIn(prefill)
    }
}
