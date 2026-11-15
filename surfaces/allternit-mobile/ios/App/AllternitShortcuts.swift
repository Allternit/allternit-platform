import AppIntents

/// The 10 App Shortcuts (plan I4f). They show in Shortcuts, Siri and Spotlight
/// with no setup. Every phrase contains the app name, as the system requires.
/// Call `AllternitShortcuts.updateAppShortcutParameters()` when the bot list
/// changes so phrases with a bot name stay current.
struct AllternitShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: AskBotIntent(),
            phrases: [
                "Ask \(\.$bot) in \(.applicationName)",
                "Ask a bot in \(.applicationName)",
                "Ask my bot in \(.applicationName)",
            ],
            shortTitle: "Ask a bot",
            systemImageName: "bubble.left.fill")
        AppShortcut(
            intent: CallMainBotIntent(),
            phrases: [
                "Call my main bot in \(.applicationName)",
                "Call my bot with \(.applicationName)",
                "Start a call in \(.applicationName)",
            ],
            shortTitle: "Call main bot",
            systemImageName: "phone.fill")
        AppShortcut(
            intent: NewBotChatIntent(),
            phrases: [
                "New chat with \(\.$bot) in \(.applicationName)",
                "Start a chat in \(.applicationName)",
                "New \(.applicationName) chat",
            ],
            shortTitle: "New chat with a bot",
            systemImageName: "square.and.pencil")
        AppShortcut(
            intent: HandOffTaskIntent(),
            phrases: [
                "Hand off a task in \(.applicationName)",
                "Give the team a task in \(.applicationName)",
                "Delegate a task with \(.applicationName)",
            ],
            shortTitle: "Hand off a task",
            systemImageName: "arrowshape.turn.up.right.fill")
        AppShortcut(
            intent: RunRoutineIntent(),
            phrases: [
                "Run \(\.$routine) in \(.applicationName)",
                "Run a routine in \(.applicationName)",
                "Start a routine with \(.applicationName)",
            ],
            shortTitle: "Run a routine",
            systemImageName: "play.circle.fill")
        AppShortcut(
            intent: SendToBotIntent(),
            phrases: [
                "Send this to \(\.$bot) in \(.applicationName)",
                "Send to a bot in \(.applicationName)",
                "Share with a bot using \(.applicationName)",
            ],
            shortTitle: "Send to a bot",
            systemImageName: "paperplane.fill")
        AppShortcut(
            intent: ShowWhatNeedsMeIntent(),
            phrases: [
                "What needs me in \(.applicationName)",
                "Show what needs me in \(.applicationName)",
                "Anything waiting on me in \(.applicationName)",
            ],
            shortTitle: "What needs me",
            systemImageName: "tray.fill")
        AppShortcut(
            intent: BotStatusIntent(),
            phrases: [
                "Bot status in \(.applicationName)",
                "What are my bots doing in \(.applicationName)",
                "How are my bots in \(.applicationName)",
            ],
            shortTitle: "Bot status",
            systemImageName: "waveform.path.ecg")
        AppShortcut(
            intent: PauseAllBotsIntent(),
            phrases: [
                "Pause all bots in \(.applicationName)",
                "Pause my bots in \(.applicationName)",
                "Stop all bots with \(.applicationName)",
            ],
            shortTitle: "Pause all bots",
            systemImageName: "pause.circle.fill")
        AppShortcut(
            intent: RememberThisIntent(),
            phrases: [
                "Remember this in \(.applicationName)",
                "Tell \(\.$bot) to remember this in \(.applicationName)",
                "Save a note to my bot in \(.applicationName)",
            ],
            shortTitle: "Remember this",
            systemImageName: "brain.head.profile")
    }
}
