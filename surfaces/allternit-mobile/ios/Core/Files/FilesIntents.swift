import AppIntents
import Foundation
import UniformTypeIdentifiers
import AllternitFilesKit

// Shortcuts actions for the Files drive (plan step 21). They run in the app
// (this file is in the app target only: the extensions that compile
// SystemSurfaces don't link AllternitFilesKit). The 10 App Shortcuts slots
// are full (App/AllternitShortcuts.swift), so these show as actions in the
// Shortcuts app, not as Siri phrases.

/// A computer with Files, as Shortcuts shows it.
struct FilesComputerEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Computer"
    static let defaultQuery = FilesComputerQuery()

    var id: String
    var name: String

    var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(name)") }
}

struct FilesComputerQuery: EntityStringQuery {
    init() {}

    private func all() async -> [FilesComputerEntity] {
        await MainActor.run { FilesDrive.shared.store.computers.map { FilesComputerEntity(id: $0.id, name: $0.name) } }
    }

    func entities(for identifiers: [String]) async throws -> [FilesComputerEntity] {
        let computers = await all()
        return identifiers.compactMap { ident in
            computers.first { $0.id == ident }
                ?? computers.first { $0.name.caseInsensitiveCompare(ident) == .orderedSame }
        }
    }

    func entities(matching string: String) async throws -> [FilesComputerEntity] {
        let needle = string.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return await all().filter { needle.isEmpty || $0.name.lowercased().contains(needle) }
    }

    func suggestedEntities() async throws -> [FilesComputerEntity] { await all() }
}

enum FilesIntentError: Error, CustomLocalizedStringResourceConvertible {
    case filesOff
    case neverWakes(String)
    case notWoken(String)
    case failed(String)

    var localizedStringResource: LocalizedStringResource {
        switch self {
        case .filesOff: return "Files isn't set up on this iPhone. Open Allternit once while signed in."
        case .neverWakes(let name): return "\(name) is asleep and set to never wake for Files. Change it in Allternit Settings > Files."
        case .notWoken(let name): return "\(name) is asleep, so the file wasn't saved."
        case .failed(let message): return "\(message)"
        }
    }
}

/// Save File to Computer: puts a file in a computer's Inbox, and can ask
/// the agent to read it (approves the Inbox task the drop created).
struct SaveFileToComputerIntent: AppIntent {
    static let title: LocalizedStringResource = "Save File to Computer"
    static let description = IntentDescription(
        "Saves a file in one of your computers' Inbox. Files in Inbox wait for your OK before your agent reads them, unless you ask it to read this one.",
        categoryName: "Files"
    )

    @Parameter(title: "File", supportedTypeIdentifiers: ["public.item"])
    var file: IntentFile

    @Parameter(title: "Computer")
    var computer: FilesComputerEntity

    @Parameter(title: "Ask my agent to read it", default: false)
    var askAgent: Bool

    static var parameterSummary: some ParameterSummary {
        Summary("Save \(\.$file) to \(\.$computer)") {
            \.$askAgent
        }
    }

    init() {}

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let enabled = await MainActor.run { FilesDrive.shared.hasFiles(computer.id) }
        guard enabled else { throw FilesIntentError.filesOff }
        let client = await MainActor.run { FilesDrive.shared.client(for: computer.id) }
        let drop = FilesInboxDrop(client: client)

        let name = FilesComputerModel.safeName(file.filename.isEmpty ? "Shortcut file" : file.filename)
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FilesIntent-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let local = folder.appendingPathComponent(name)
        if let source = file.fileURL {
            let scoped = source.startAccessingSecurityScopedResource()
            defer { if scoped { source.stopAccessingSecurityScopedResource() } }
            try FileManager.default.copyItem(at: source, to: local)
        } else {
            try file.data.write(to: local)
        }

        do {
            let inbox = try await drop.inboxRoot()
            let result: FilesWriteResult
            do {
                result = try await drop.send(fileAt: local, name: name, to: inbox, wake: .whenAutomatic)
            } catch FilesError.wakeConsentRequired(let hint, _) {
                guard hint?.setting != .never else { throw FilesIntentError.neverWakes(computer.name) }
                // Setting "ask": waking bills credits, so the person confirms here.
                do {
                    try await requestConfirmation(
                        result: .result(dialog: "\(computer.name) is asleep. Wake it to save the file? It bills credits while it is awake."),
                        confirmationActionName: .go
                    )
                } catch {
                    throw FilesIntentError.notWoken(computer.name)
                }
                try await client.requestWake(reason: .upload, hint: hint)
                result = try await drop.send(fileAt: local, name: name, to: inbox, wake: .whenAutomatic)
            }
            guard askAgent else {
                return .result(dialog: "Saved \(result.item.name) to \(computer.name)'s Inbox.")
            }
            switch await drop.askAgent(itemId: result.item.id) {
            case .approved:
                return .result(dialog: "Saved \(result.item.name). Your agent is reading it and will save a summary next to it.")
            case .pending, .notAsked:
                return .result(dialog: "Saved \(result.item.name) to Inbox. Approve it in Allternit to let your agent read it.")
            }
        } catch let error as FilesIntentError {
            throw error
        } catch {
            throw FilesIntentError.failed(FilesComputerModel.message(error))
        }
    }
}

/// Open Files on Computer: the computer's Files screens in the app.
struct OpenComputerFilesIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Files on Computer"
    static let description = IntentDescription("Opens a computer's files in Allternit.", categoryName: "Files")
    static let openAppWhenRun: Bool = true

    @Parameter(title: "Computer")
    var computer: FilesComputerEntity

    init() {}

    @MainActor
    func perform() async throws -> some IntentResult {
        guard FilesDrive.shared.hasFiles(computer.id) else { throw FilesIntentError.filesOff }
        FilesDrive.shared.launch = .browse(computer.id)
        return .result()
    }
}

/// Scan Document to Computer: opens the document camera; the PDF lands in
/// the computer's Scans folder.
struct ScanToComputerIntent: AppIntent {
    static let title: LocalizedStringResource = "Scan Document to Computer"
    static let description = IntentDescription("Scans a document with the camera and saves it as a PDF in a computer's Scans folder.",
                                               categoryName: "Files")
    static let openAppWhenRun: Bool = true

    @Parameter(title: "Computer")
    var computer: FilesComputerEntity

    init() {}

    @MainActor
    func perform() async throws -> some IntentResult {
        guard FilesDrive.shared.hasFiles(computer.id) else { throw FilesIntentError.filesOff }
        FilesDrive.shared.launch = .scan(computer.id)
        return .result()
    }
}
