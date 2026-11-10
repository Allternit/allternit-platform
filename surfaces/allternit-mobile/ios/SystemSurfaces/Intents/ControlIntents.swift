import AppIntents
import Foundation

// Control group: pause or resume all bots, stop a run, quiet hours, switch
// computer, set a bot's brain.

private func setPausedAndSay(_ paused: Bool) async throws -> (PausedState, IntentDialog) {
    let state = try await IntentGate.run { svc in try await svc.setPaused(paused) }
    let text = state.paused
        ? "All bots are paused. Routines and loops will not start, and bots will not message on their own. Your own messages still run."
        : "Bots are running again."
    return (state, IntentDialog(stringLiteral: text))
}

struct SetBotsPausedIntent: AppIntent {
    static let title: LocalizedStringResource = "Pause or resume all bots"
    static let description = IntentDescription(
        "Pauses or resumes every bot at once. While paused, routines and loops do not start and bots send nothing on their own. Your own messages still run.",
        categoryName: "Control")

    @Parameter(title: "Paused", default: true)
    var paused: Bool

    static var parameterSummary: some ParameterSummary { Summary("Set all bots paused to \(\.$paused)") }

    func perform() async throws -> some IntentResult & ReturnsValue<Bool> & ProvidesDialog {
        let (state, dialog) = try await setPausedAndSay(paused)
        return .result(value: state.paused, dialog: dialog)
    }
}

struct PauseAllBotsIntent: AppIntent {
    static let title: LocalizedStringResource = "Pause all bots"
    static let description = IntentDescription(
        "Stops routines, loops and unprompted messages from every bot until you resume.",
        categoryName: "Control")

    static var parameterSummary: some ParameterSummary { Summary("Pause all bots") }

    func perform() async throws -> some IntentResult & ReturnsValue<Bool> & ProvidesDialog {
        let (state, dialog) = try await setPausedAndSay(true)
        return .result(value: state.paused, dialog: dialog)
    }
}

struct ResumeAllBotsIntent: AppIntent {
    static let title: LocalizedStringResource = "Resume all bots"
    static let description = IntentDescription(
        "Lets routines, loops and bots run again after a pause.",
        categoryName: "Control")

    static var parameterSummary: some ParameterSummary { Summary("Resume all bots") }

    func perform() async throws -> some IntentResult & ReturnsValue<Bool> & ProvidesDialog {
        let (state, dialog) = try await setPausedAndSay(false)
        return .result(value: state.paused, dialog: dialog)
    }
}

struct StopRunIntent: AppIntent {
    static let title: LocalizedStringResource = "Stop a run"
    static let description = IntentDescription(
        "Stops the bot's current run in a conversation.",
        categoryName: "Control")

    @Parameter(title: "Conversation")
    var conversation: ConversationEntity

    static var parameterSummary: some ParameterSummary { Summary("Stop the run in \(\.$conversation)") }

    func perform() async throws -> some IntentResult & ReturnsValue<Bool> & ProvidesDialog {
        let info = conversation.info
        _ = try await IntentGate.run { svc in try await svc.stopRun(conversation: info) }
        return .result(value: true, dialog: IntentDialog(stringLiteral: "Stopped the run in \(info.title)."))
    }
}

struct SetQuietHoursIntent: AppIntent {
    static let title: LocalizedStringResource = "Turn quiet hours on or off"
    static let description = IntentDescription(
        "Turns your saved quiet hours on or off for the whole account. Works while your computer is asleep.",
        categoryName: "Control")

    @Parameter(title: "Quiet hours on", default: true)
    var enabled: Bool

    static var parameterSummary: some ParameterSummary { Summary("Turn quiet hours \(\.$enabled)") }

    func perform() async throws -> some IntentResult & ReturnsValue<Bool> & ProvidesDialog {
        let on = enabled
        let state = try await IntentGate.run(needsComputer: false) { svc in try await svc.setQuietHours(enabled: on) }
        return .result(value: state.enabled,
                       dialog: IntentDialog(stringLiteral: state.enabled ? "Quiet hours are on." : "Quiet hours are off."))
    }
}

struct SwitchComputerIntent: AppIntent {
    static let title: LocalizedStringResource = "Switch active computer"
    static let description = IntentDescription(
        "Chooses which of your computers Allternit talks to.",
        categoryName: "Control")

    @Parameter(title: "Computer")
    var computer: ComputerEntity

    static var parameterSummary: some ParameterSummary { Summary("Switch to \(\.$computer)") }

    func perform() async throws -> some IntentResult & ReturnsValue<ComputerEntity> & ProvidesDialog {
        let id = computer.id
        let info = try await IntentGate.run(needsComputer: false) { svc in try await svc.switchComputer(id: id) }
        return .result(value: ComputerEntity(info),
                       dialog: IntentDialog(stringLiteral: "Now using \(info.name)\(info.isOnline ? "." : ", which is offline right now.")"))
    }
}

struct SetBotBrainIntent: AppIntent {
    static let title: LocalizedStringResource = "Set a bot's brain"
    static let description = IntentDescription(
        "Changes which model a bot thinks with.",
        categoryName: "Control")

    @Parameter(title: "Bot")
    var bot: BotEntity

    @Parameter(title: "Model")
    var model: ModelEntity

    static var parameterSummary: some ParameterSummary { Summary("Set \(\.$bot) to think with \(\.$model)") }

    func perform() async throws -> some IntentResult & ReturnsValue<BotEntity> & ProvidesDialog {
        let botId = bot.id, modelId = model.id
        let result = try await IntentGate.run { svc in try await svc.setBrain(botId: botId, modelId: modelId) }
        return .result(value: BotEntity(result.bot),
                       dialog: IntentDialog(stringLiteral: "\(result.bot.name) now thinks with \(result.model.name)."))
    }
}
