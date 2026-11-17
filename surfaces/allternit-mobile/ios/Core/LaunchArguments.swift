import Foundation

/// Normalized access to process launch arguments.
///
/// Maestro serializes YAML arguments differently depending on the runner
/// version / platform: `key`, `-key`, `key=true`, and `-key=true` are all
/// observed in the wild. Use `launchArgumentEnabled(_:)` instead of raw
/// `CommandLine.arguments.contains` so audit deep-links stay reliable.
func launchArgumentEnabled(_ key: String) -> Bool {
    CommandLine.arguments.contains { arg in
        // Maestro serializes YAML launch arguments differently depending on
        // key format and runner state: `key`, `-key`, `--key`, `key=true`,
        // `-key=true`, and `--key=true` are all observed in the wild.
        var normalized = arg
        while normalized.hasPrefix("-") {
            normalized = String(normalized.dropFirst())
        }
        return normalized == key || normalized == "\(key)=true"
    }
}

/// The value of a `key=value` launch argument (`-key=value`, `--key=value`),
/// or of `-key value` passed as two arguments. nil when absent.
func launchArgumentValue(_ key: String) -> String? {
    let args = CommandLine.arguments
    for (index, arg) in args.enumerated() {
        var normalized = arg
        while normalized.hasPrefix("-") {
            normalized = String(normalized.dropFirst())
        }
        if normalized.hasPrefix("\(key)=") {
            return String(normalized.dropFirst(key.count + 1))
        }
        if normalized == key, index + 1 < args.count, !args[index + 1].hasPrefix("-") {
            return args[index + 1]
        }
    }
    return nil
}

/// Launch argument keys read through `launchArgumentEnabled(_:)` outside
/// AllternitApp.
enum LaunchArgument {
    /// `-sample-workspace` (DEBUG only): run signed in on the web preview's
    /// sample workspace (Harbor Bakery), answered locally by
    /// `SampleURLProtocol`, for side-by-side and App Store screenshots.
    /// Combine with `-bots` / `-code` / `-open-aci` etc. to pick the screen.
    static let sampleWorkspace = "sample-workspace"

    /// `-calm-drawer-open` (DEBUG only): launch with the drawer open (page
    /// pushed right, scaled, rounded) for screenshots.
    static let calmDrawerOpen = "calm-drawer-open"

    /// `-calm-fan` (DEBUG only): open the ☰ mode fan (Home · Bots · Code ·
    /// ACI) on launch, with the Code card under a pretend finger (grown).
    /// `-open-mode-dial` does the same (older name).
    static let calmFan = "calm-fan"

    /// `-calm-shelf` (DEBUG only): open the bots shelf on launch with the
    /// second bot magnified and its "Name · 2 of n" label. Combine with
    /// `-sample-workspace` for sample bots.
    static let calmShelf = "calm-shelf"

    /// `-calm-recents-select` (DEBUG only): open the session overview in select
    /// mode with the first two rows ticked (phone calm step 25).
    static let calmRecentsSelect = "calm-recents-select"

    /// `-calm-overview-menu` (DEBUG only): open the session overview with its
    /// ⋯ "Recents options" popover showing.
    static let calmOverviewMenu = "calm-overview-menu"

    /// `-calm-search` (DEBUG only): open the Search chats and projects screen.
    static let calmSearch = "calm-search"

    /// `-calm-jump-button` (DEBUG only): show the chat's scroll-to-bottom button.
    static let calmJumpButton = "calm-jump-button"

    /// `-calm-gallery` (DEBUG only): show `CalmGalleryView`.
    static let calmGallery = "calm-gallery"

    /// `-calm-chip=<none|noComputer|cantReach|waking|gate>` (DEBUG only):
    /// force the no-computer chip above the composer into one state for
    /// screenshots (`ComputerChipStore`).
    static let calmChip = "calm-chip"

    /// `-open-plus-sheet` (DEBUG only): open the composer's + sheet
    /// ("Add to chat") on launch. Combine with `-chat`.
    static let openPlusSheet = "open-plus-sheet"

    /// `-open-plus-permission` (DEBUG only): open the + sheet on its
    /// Permission page (Auto / Ask / Read-only).
    static let openPlusPermission = "open-plus-permission"

    /// `-open-plus-computer` (DEBUG only): open the + sheet on its Computer
    /// page (the computer list, the fix action).
    static let openPlusComputer = "open-plus-computer"

    /// `-open-chat-menu` (DEBUG only): open the chat's ⋯ menu (Run on my
    /// phone, Capture to brain, Share link, Pin, Archive).
    static let openChatMenu = "open-chat-menu"
}
