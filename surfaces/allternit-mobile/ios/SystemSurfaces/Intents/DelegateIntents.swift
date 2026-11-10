import AppIntents
import Foundation
import UniformTypeIdentifiers

// Delegate group of the action suite (plan step I4b): Hand off a task to the
// team, Run a routine, Start a code task on my computer, Send to a bot.
// SystemSurfaces code: system frameworks and SystemSurfaces types only.

// MARK: - File rules (testable; the app's `sendFile` applies them too)

enum SendFileRules {
    /// Uploads go base64 in one JSON body, so files are capped at 10 MB.
    static let maxBytes = 10 * 1024 * 1024

    struct TooLarge: Error, Equatable, LocalizedError, CustomLocalizedStringResourceConvertible {
        let bytes: Int
        var message: String {
            "That file is \(SendFileRules.sizeText(bytes)). Allternit can send files up to 10 MB."
        }
        var errorDescription: String? { message }
        var localizedStringResource: LocalizedStringResource { LocalizedStringResource(stringLiteral: message) }
    }

    static func check(byteCount: Int) throws {
        if byteCount > maxBytes { throw TooLarge(bytes: byteCount) }
    }

    /// The size without loading the file when it is on disk.
    static func byteCount(of file: IntentFile) -> Int {
        if let url = file.fileURL, let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize {
            return size
        }
        return file.data.count
    }

    static func sizeText(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    /// The bot gets the file as a Markdown link, the same shape the share
    /// extension sends, with the note above it.
    static func message(filename: String, url: String, note: String?) -> String {
        let name = filename.replacingOccurrences(of: "[", with: "(").replacingOccurrences(of: "]", with: ")")
        let link = "[\(name)](\(url))"
        guard let note = note?.trimmingCharacters(in: .whitespacesAndNewlines), !note.isEmpty else { return link }
        return "\(note)\n\n\(link)"
    }

    static func mediaType(for filename: String) -> String {
        UTType(filenameExtension: (filename as NSString).pathExtension)?.preferredMIMEType
            ?? "application/octet-stream"
    }

    /// Text and link joined into one message; nil when both are empty.
    static func compose(text: String?, link: URL?) -> String? {
        let parts = [text?.trimmingCharacters(in: .whitespacesAndNewlines), link?.absoluteString]
            .compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: "\n\n")
    }
}

// MARK: - Hand off a task to the team

struct HandOffTaskIntent: AppIntent {
    static let title: LocalizedStringResource = "Hand Off a Task to the Team"
    static let description = IntentDescription(
        "Creates a project for your bot team and gives the task to its coordinator.",
        resultValueName: "Project")

    @Parameter(title: "Task")
    var task: String
    @Parameter(title: "Project name", description: "Leave empty to name it after the task.")
    var projectName: String?

    static var parameterSummary: some ParameterSummary {
        Summary("Hand off \(\.$task) to the team") {
            \.$projectName
        }
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<ProjectEntity> & ProvidesDialog {
        let text = task.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw $task.needsValueError("What should the team do?") }
        let name = projectName
        let project = try await IntentGate.run { svc in try await svc.handOff(text: text, title: name) }
        return .result(value: ProjectEntity(project),
                       dialog: IntentSpeech.dialog("Handed off to the team as \"\(project.title)\"."))
    }
}

// MARK: - Run a routine

struct RunRoutineIntent: AppIntent {
    static let title: LocalizedStringResource = "Run a Routine"
    static let description = IntentDescription("Starts one of your routines now.", resultValueName: "Routine")

    @Parameter(title: "Routine")
    var routine: RoutineEntity

    static var parameterSummary: some ParameterSummary {
        Summary("Run \(\.$routine)")
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<RoutineEntity> & ProvidesDialog {
        let id = routine.id
        let r = try await IntentGate.run { svc in try await svc.runRoutine(id: id) }
        let text = r.started ? "Started \(r.routine.name)." : "\(r.routine.name) did not start."
        return .result(value: RoutineEntity(r.routine), dialog: IntentSpeech.dialog(text))
    }
}

// MARK: - Start a code task on my computer

struct StartCodeTaskIntent: AppIntent {
    static let title: LocalizedStringResource = "Start a Code Task on My Computer"
    static let description = IntentDescription(
        "Starts a Code session on your active computer and sends it the task.",
        resultValueName: "Conversation")

    @Parameter(title: "Task")
    var task: String
    @Parameter(title: "Project")
    var project: ProjectEntity?

    static var parameterSummary: some ParameterSummary {
        Summary("Start code task \(\.$task)") {
            \.$project
        }
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<ConversationEntity> & ProvidesDialog {
        let text = task.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw $task.needsValueError("What should the code task do?") }
        let projectId = project?.id
        let convo = try await IntentGate.run { svc in try await svc.startCodeTask(text: text, projectId: projectId) }
        return .result(value: ConversationEntity(convo),
                       dialog: IntentSpeech.dialog("Started a code task on your computer: \(convo.title)."))
    }
}

// MARK: - Send to a bot

struct SendToBotIntent: AppIntent {
    static let title: LocalizedStringResource = "Send to a Bot"
    static let description = IntentDescription(
        "Sends text, a link, or a file or image (up to 10 MB) to one of your bots.",
        resultValueName: "Conversation")

    @Parameter(title: "Bot")
    var bot: BotEntity
    @Parameter(title: "Text")
    var text: String?
    @Parameter(title: "Link")
    var link: URL?
    @Parameter(title: "File or image")
    var file: IntentFile?

    static var parameterSummary: some ParameterSummary {
        Summary("Send to \(\.$bot)") {
            \.$text
            \.$link
            \.$file
        }
    }

    init() {}

    func perform() async throws -> some IntentResult & ReturnsValue<ConversationEntity> & ProvidesDialog {
        let botId = bot.id
        let botName = bot.name
        let message = SendFileRules.compose(text: text, link: link)
        let convo: ConversationInfo
        if let file {
            // Check the size before loading the file into memory.
            try SendFileRules.check(byteCount: SendFileRules.byteCount(of: file))
            let data = file.data
            try SendFileRules.check(byteCount: data.count)
            let filename = file.filename.isEmpty ? "file" : file.filename
            convo = try await IntentGate.run { svc in
                try await svc.sendFile(botId: botId, data: data, filename: filename, note: message)
            }
        } else {
            guard let message else { throw $text.needsValueError("What do you want to send?") }
            convo = try await IntentGate.run { svc in try await svc.send(botId: botId, text: message) }
        }
        return .result(value: ConversationEntity(convo), dialog: IntentSpeech.dialog("Sent to \(botName)."))
    }
}
