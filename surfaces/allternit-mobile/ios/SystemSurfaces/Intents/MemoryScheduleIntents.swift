import AppIntents
import Foundation

// Memory and Schedule groups: Remember this, Create a routine.

enum RoutineRepeatChoice: String, AppEnum {
    case daily, weekdays, everyFewHours

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Repeat"
    static let caseDisplayRepresentations: [RoutineRepeatChoice: DisplayRepresentation] = [
        .daily: "Every day at a time",
        .weekdays: "Weekdays at a time",
        .everyFewHours: "Every few hours",
    ]
}

/// Maps the simple Shortcuts choices to the 5-field cron the app's routine
/// editor (`RoutineRule` in `Features/Connect/RoutineScheduleEditor.swift`)
/// writes and reads back: `M H * * *`, `M H * * 1-5` and `0 0-23/N * * *`.
/// Hour steps are limited to what that editor can show (1 to 4 hours).
enum RoutineSchedule {
    static let allowedHourSteps = [1, 2, 3, 4]

    static func cron(kind: RoutineRepeatChoice, hour: Int?, minute: Int?, everyHours: Int?) throws -> String {
        switch kind {
        case .daily, .weekdays:
            guard let hour, let minute, (0...23).contains(hour), (0...59).contains(minute) else {
                throw IntentFailure.failed("Pick a time of day for this routine.")
            }
            return "\(minute) \(hour) * * \(kind == .daily ? "*" : "1-5")"
        case .everyFewHours:
            guard let n = everyHours, allowedHourSteps.contains(n) else {
                throw IntentFailure.failed("Routines can repeat every 1, 2, 3 or 4 hours. Pick one of those, or set a custom schedule in Allternit.")
            }
            return "0 0-23/\(n) * * *"
        }
    }

    static func words(kind: RoutineRepeatChoice, hour: Int?, minute: Int?, everyHours: Int?) -> String {
        func clock() -> String {
            let h = hour ?? 0, m = minute ?? 0
            return "\(h % 12 == 0 ? 12 : h % 12):\(String(format: "%02d", m)) \(h < 12 ? "AM" : "PM")"
        }
        switch kind {
        case .daily: return "every day at \(clock())"
        case .weekdays: return "weekdays at \(clock())"
        case .everyFewHours: return "every \(everyHours ?? 1) hour\(everyHours == 1 ? "" : "s")"
        }
    }
}

struct RememberThisIntent: AppIntent {
    static let title: LocalizedStringResource = "Remember this"
    static let description = IntentDescription(
        "Saves a note to a bot's memory so it can use it later.",
        categoryName: "Memory")

    @Parameter(title: "Bot")
    var bot: BotEntity

    @Parameter(title: "Note")
    var text: String

    static var parameterSummary: some ParameterSummary { Summary("Remember \(\.$text) for \(\.$bot)") }

    func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        let note = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !note.isEmpty else { throw IntentFailure.failed("There is nothing to remember.") }
        let botId = bot.id
        let saved = try await IntentGate.run { svc in try await svc.remember(botId: botId, text: note) }
        let msg = "Saved to \(saved.botName)'s memory."
        return .result(value: msg, dialog: IntentDialog(stringLiteral: msg))
    }
}

struct CreateRoutineIntent: AppIntent {
    static let title: LocalizedStringResource = "Create a routine"
    static let description = IntentDescription(
        "Creates a routine that runs your instructions on a schedule: every day or on weekdays at a time, or every 1 to 4 hours.",
        categoryName: "Schedule")

    @Parameter(title: "Name")
    var name: String

    @Parameter(title: "Instructions")
    var instructions: String

    @Parameter(title: "Repeat", default: .daily)
    var repeatKind: RoutineRepeatChoice

    @Parameter(title: "Time")
    var time: Date?

    @Parameter(title: "Hours between runs", default: 4)
    var everyHours: Int

    static var parameterSummary: some ParameterSummary {
        Summary("Create routine \(\.$name): \(\.$instructions), \(\.$repeatKind) at \(\.$time) or every \(\.$everyHours) hours")
    }

    func perform() async throws -> some IntentResult & ReturnsValue<RoutineEntity> & ProvidesDialog {
        let title = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let steps = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, !steps.isEmpty else { throw IntentFailure.failed("A routine needs a name and instructions.") }
        let comps = time.map { Calendar.current.dateComponents([.hour, .minute], from: $0) }
        let cron = try RoutineSchedule.cron(kind: repeatKind, hour: comps?.hour, minute: comps?.minute, everyHours: everyHours)
        let when = RoutineSchedule.words(kind: repeatKind, hour: comps?.hour, minute: comps?.minute, everyHours: everyHours)
        let info = try await IntentGate.run { svc in
            try await svc.createRoutine(name: title, instructions: steps, schedule: cron)
        }
        return .result(value: RoutineEntity(info),
                       dialog: IntentDialog(stringLiteral: "Created \(info.name), \(when)."))
    }
}
