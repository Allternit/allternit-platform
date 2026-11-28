import Foundation

// Pure rules for the composer's "+" sheet ("Add to chat") and the chat ⋯
// menu (phone calm pass step 22, contract C2, brief items 4 and 5). Web
// twins: allternit-ai `computerRowValue` (ComposerPlusSheet.tsx) and the
// chat ⋯ rows in `screens/ChatsScreen.tsx` (CHAT_MENU_ITEMS + Pin / Archive).

/// The pages inside the + sheet. Root is the grouped list; the others push
/// in from the right with a back button.
enum ComposerPlusPage: String, CaseIterable, Sendable {
    case root, permission, computer, folder, project

    var title: String {
        switch self {
        case .root: return "Add to chat"
        case .permission: return "Permission"
        case .computer: return "Computer"
        case .folder: return "Folder"
        case .project: return "Project"
        }
    }

    /// The page the sheet opens on. A DEBUG launch argument can start it on
    /// Permission or Computer (`-open-plus-permission`, `-open-plus-computer`);
    /// Permission wins when both are passed. Permission only exists where the
    /// chat shows it, so without it the sheet falls back to the root.
    static func initial(openPermission: Bool, openComputer: Bool, permissionAvailable: Bool = true) -> ComposerPlusPage {
        if openPermission, permissionAvailable { return .permission }
        if openComputer { return .computer }
        return .root
    }
}

/// The short value on the "Computer ›" row (web `computerRowValue`). With a
/// computer answering it names that computer when there is one.
func computerRowValue(_ state: ComputerChipState, activeName: String?) -> String {
    switch state.kind {
    case .none:
        if let activeName, !activeName.isEmpty { return activeName }
        return "Connected"
    case .connect: return "No computer"
    case .fix: return "Can't reach"
    case .waking: return "Waking…"
    case .starting: return "Starting…"
    case .gate, .suspended, .runway: return state.actionLabel ?? "Check"
    }
}

/// One computer's status line on the Computer page. Words only, no codes.
func computerStatusLabel(status: String, wakeable: Bool) -> String {
    switch status {
    case "online": return "Online"
    case "busy": return "Online · busy"
    default: return wakeable ? "Asleep · wakes when you send" : "Offline"
    }
}

/// The rows of the chat ⋯ menu, top first. "Run on my phone" and "Capture to
/// brain" moved here from the + sheet; Share link moved here from the header
/// so ⋯ fits beside New chat. Pin / Unpin and Archive act on the open chat,
/// so they show only while a saved chat is open (web ChatsScreen).
enum ChatMenuItem: String, CaseIterable, Sendable {
    case runOnPhone, captureToBrain, shareLink, pin, unpin, saveTestCase, archive
}

struct ChatMenuInput: Equatable, Sendable {
    /// The chat has been saved (it has a session id).
    var hasSession = false
    /// It has messages to share.
    var hasMessages = false
    var pinned = false
    /// Run on my phone applies (not an artifact session).
    var allowsPhoneRun = true
    /// The account has a second brain to capture into.
    var hasBrain = false
}

func chatMenuItems(_ input: ChatMenuInput) -> [ChatMenuItem] {
    var items: [ChatMenuItem] = []
    if input.allowsPhoneRun { items.append(.runOnPhone) }
    if input.hasBrain { items.append(.captureToBrain) }
    if input.hasSession, input.hasMessages { items.append(.shareLink) }
    if input.hasSession {
        items.append(input.pinned ? .unpin : .pin)
        // Save as test case: needs a saved chat with something in it.
        if input.hasMessages { items.append(.saveTestCase) }
        items.append(.archive)
    }
    return items
}
