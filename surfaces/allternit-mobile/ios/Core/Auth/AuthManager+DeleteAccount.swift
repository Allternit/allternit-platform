import Foundation
import ClerkKit

/// Account deletion (App Store guideline 5.1.1(v)): the user can delete their
/// account from inside the app.
///
/// The app calls Clerk's own self-delete (`User.delete()`, Clerk iOS 1.1.0).
/// Clerk then sends `user.deleted` to cloud-api
/// (`routes/clerk_webhooks.rs handle_user_deleted`), which closes the
/// Allternit account on the server.
extension AuthManager {
    enum DeleteAccountError: LocalizedError {
        case notSignedIn
        /// The Clerk instance has "Allow users to delete their accounts"
        /// turned off (`user.deleteSelfEnabled == false`).
        case selfDeleteDisabled

        var errorDescription: String? {
            switch self {
            case .notSignedIn:
                return "You're not signed in."
            case .selfDeleteDisabled:
                return "Account deletion isn't turned on for this sign-in. Email us and we'll delete it for you."
            }
        }
    }

    /// Where a user goes when the app can't delete the account itself.
    static let accountDeletionSupportURL = URL(string: "mailto:privacy@allternit.com?subject=Delete%20my%20Allternit%20account")!

    /// Whether Clerk lets this user delete their own account. nil when there
    /// is no signed-in Clerk user.
    var canSelfDeleteAccount: Bool? {
        guard isClerkConfigured, let user = Clerk.shared.user else { return nil }
        return user.deleteSelfEnabled
    }

    /// Deletes the signed-in Clerk user and drops this device's runtime
    /// credential. The sign-in gate is NOT flipped here, so the caller can
    /// show the result first; call `finishAccountDeletion()` when the user
    /// taps Done.
    ///
    /// Throws when Clerk refuses (self-delete disabled, needs a recent
    /// sign-in, network). On a throw nothing local is cleared, so the user is
    /// still signed in and can retry.
    func deleteAccount() async throws {
        guard isClerkConfigured, let user = Clerk.shared.user else {
            throw DeleteAccountError.notSignedIn
        }
        guard user.deleteSelfEnabled else {
            throw DeleteAccountError.selfDeleteDisabled
        }
        try await user.delete()

        // The account is gone; everything below is local cleanup and must
        // not turn a successful delete into an error.
        await RuntimePairing.shared.revoke()
    }

    /// Clears the dead Clerk session and returns the app to the sign-in gate.
    func finishAccountDeletion() async {
        try? await Clerk.shared.auth.signOut()
        refreshAuthState()
    }
}
