import Foundation

/// Why the File Provider extension handed an error to the File Provider UI
/// sheet (plan step 20). The extension answers `.notAuthenticated` with this
/// kind in the error's `userInfo` (and leaves a pending prompt in
/// `FilesSharedStore`, in case the system drops custom `userInfo` keys on
/// the way); Files then shows a button that opens the sheet.
public enum FilesPromptKind: String, Codable, Sendable {
    /// The computer is asleep and its wake setting is `ask`: the wake sheet.
    case wake
    /// A protected root (Memory Drive) needs a fresh Face ID unlock.
    case unlock
    /// No Files token, or cloud-api refused it: open the app to reconnect.
    case signIn

    /// `userInfo` key on the `NSFileProviderError` the extension returns.
    public static let userInfoKey = "AllternitFilesPrompt"
}
