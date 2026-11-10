import AppIntents
import Foundation
import UniformTypeIdentifiers

// Status group: Bot status, Get last routine result, Find a conversation,
// Get latest artifact from a bot. Bot status and the routine result read the
// cloud summary, so they answer with the computer asleep.

/// Pure text and filter helpers (unit-tested).
enum StatusFormat {
    static func matches(status: String, filter: BotStatusFilter) -> Bool {
        let s = status.lowercased()
        func has(_ keys: [String]) -> Bool { keys.contains { s.contains($0) } }
        switch filter {
        case .all: return true
        case .working: return has(["work", "run"])
        case .needsMe: return has(["wait", "attention", "need", "block"])
        }
    }

    static func age(of date: Date?, now: Date = Date()) -> String {
        guard let date else { return "time unknown" }
        let s = max(0, Int(now.timeIntervalSince(date)))
        if s < 90 { return "just now" }
        if s < 3600 { return "\(s / 60) minutes ago" }
        if s < 86_400 { let h = s / 3600; return "\(h) hour\(h == 1 ? "" : "s") ago" }
        let d = s / 86_400
        return "\(d) day\(d == 1 ? "" : "s") ago"
    }

    static func row(_ bot: BotInfo, asOf: Date?, now: Date = Date()) -> String {
        let status = bot.status.replacingOccurrences(of: "_", with: " ")
        var line = "\(bot.name): \(status.isEmpty ? "no status" : status)"
        if !bot.step.isEmpty { line += ", \(bot.step)" }
        return line + " (as of \(age(of: asOf, now: now)))"
    }
}

enum BotStatusChoice: String, AppEnum {
    case all, working, needsMe

    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Bot filter"
    static let caseDisplayRepresentations: [BotStatusChoice: DisplayRepresentation] = [
        .all: "All bots",
        .working: "Working",
        .needsMe: "Needs me",
    ]

    var filter: BotStatusFilter {
        switch self {
        case .all: return .all
        case .working: return .working
        case .needsMe: return .needsMe
        }
    }
}

struct BotStatusIntent: AppIntent {
    static let title: LocalizedStringResource = "Bot status"
    static let description = IntentDescription(
        "Lists your bots with what each is doing and how old that information is. Works while your computer is asleep.",
        categoryName: "Status")

    @Parameter(title: "Show", default: .all)
    var filter: BotStatusChoice

    static var parameterSummary: some ParameterSummary { Summary("Show \(\.$filter) bots") }

    func perform() async throws -> some IntentResult & ReturnsValue<[String]> & ProvidesDialog {
        try IntentGate.check(needsComputer: false)
        let wanted = filter.filter
        let list: BotStatusList
        if IntentServiceRegistry.current != nil {
            list = try await IntentGate.run(needsComputer: false) { svc in try await svc.botStatus(filter: wanted) }
        } else if let snap = AppGroupStore.snapshot {
            // Extension process: answer from the shared snapshot.
            let rows = snap.bots.filter { StatusFormat.matches(status: $0.status, filter: wanted) }
            list = BotStatusList(bots: rows.map { BotInfo(id: $0.botId, name: $0.name, status: $0.status, step: $0.step) },
                                 asOf: snap.asOf)
        } else {
            throw IntentFailure.appRequired
        }
        let lines = list.bots.map { StatusFormat.row($0, asOf: list.asOf) }
        let dialog: String
        if lines.isEmpty {
            dialog = wanted == .all ? "You have no bots yet." : "No bots match that. Data is from \(StatusFormat.age(of: list.asOf))."
        } else {
            dialog = "\(lines.count) bot\(lines.count == 1 ? "" : "s"). Data is from \(StatusFormat.age(of: list.asOf)). " + lines.joined(separator: ". ")
        }
        return .result(value: lines, dialog: IntentDialog(stringLiteral: dialog))
    }
}

struct LastRoutineResultIntent: AppIntent {
    static let title: LocalizedStringResource = "Get last routine result"
    static let description = IntentDescription(
        "Tells you how the most recent routine run ended. Works while your computer is asleep.",
        categoryName: "Status")

    @Parameter(title: "Routine")
    var routine: RoutineEntity?

    static var parameterSummary: some ParameterSummary { Summary("Get the last result of \(\.$routine)") }

    func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        let id = routine?.id
        let r = try await IntentGate.run(needsComputer: false) { svc in try await svc.lastRoutineResult(routineId: id) }
        let text = "\(r.title): \(r.status), \(StatusFormat.age(of: r.at))."
        return .result(value: text, dialog: IntentDialog(stringLiteral: text))
    }
}

struct FindConversationIntent: AppIntent {
    static let title: LocalizedStringResource = "Find a conversation"
    static let description = IntentDescription(
        "Finds your conversations whose title contains the words you give.",
        categoryName: "Status")

    @Parameter(title: "Title contains")
    var text: String

    static var parameterSummary: some ParameterSummary { Summary("Find conversations titled \(\.$text)") }

    func perform() async throws -> some IntentResult & ReturnsValue<[ConversationEntity]> & ProvidesDialog {
        let needle = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { throw IntentFailure.failed("Tell me which words to look for.") }
        let found = try await IntentGate.run(needsComputer: false) { svc in
            try await svc.listConversations(matching: needle)
        }
        let dialog = found.isEmpty
            ? "No conversation title contains \"\(needle)\"."
            : "Found \(found.count): " + found.prefix(5).map(\.title).joined(separator: ", ") + (found.count > 5 ? ", and more." : ".")
        return .result(value: found.map(ConversationEntity.init), dialog: IntentDialog(stringLiteral: dialog))
    }
}

struct LatestArtifactIntent: AppIntent {
    static let title: LocalizedStringResource = "Get latest artifact from a bot"
    static let description = IntentDescription(
        "Returns the newest artifact a bot wrote, as a file. Text-based artifacts only (documents, pages, code); PDFs and images are not downloaded yet.",
        categoryName: "Status")

    @Parameter(title: "Bot")
    var bot: BotEntity

    static var parameterSummary: some ParameterSummary { Summary("Get the latest artifact from \(\.$bot)") }

    func perform() async throws -> some IntentResult & ReturnsValue<IntentFile> & ProvidesDialog {
        let botId = bot.id
        let name = bot.name
        let info = try await IntentGate.run(needsComputer: false) { svc in try await svc.latestArtifact(botId: botId) }
        guard let url = info.fileURL else { throw IntentFailure.notFound("that artifact's file") }
        let file = IntentFile(fileURL: url, filename: url.lastPathComponent)
        return .result(value: file, dialog: IntentDialog(stringLiteral: "Here is \(info.title) from \(name)."))
    }
}
