import SwiftUI
import UIKit
import UniformTypeIdentifiers
import AllternitFilesKit

/// Share sheet → a computer's Inbox (Files drive, plan step 21).
///
/// The app set Files up (`FilesDriveBridge`): the computers and the Files
/// token are in the App Group and the shared keychain, so this extension
/// uploads by itself over the cloud relay (relay only, like the File
/// Provider: no mesh in an extension). Inbox files are untrusted: the drop
/// creates a task that waits for the owner. "Ask my agent to read it"
/// approves that one task right after the upload.
@MainActor
final class FilesShareModel: ObservableObject {
    enum Phase: Equatable {
        case picking
        case sending(done: UInt64, total: UInt64)
        /// The computer is asleep and its setting is "ask".
        case wake
        case done(String)
        case failed(String)
    }

    @Published var phase: Phase = .picking
    @Published var selectedComputerId: ComputerId?
    @Published var askAgent = false
    let computers: [FilesSharedComputer]

    private let store: FilesSharedStore
    private let providers: [NSItemProvider]
    private var pendingHint: WakeHint?
    private var staged: [URL] = []

    init(providers: [NSItemProvider]) {
        self.providers = providers
        store = FilesSharedStore.fromMainBundle(defaultAppGroup: AppGroup.identifier)
        computers = store.token() == nil ? [] : store.computers
        selectedComputerId = computers.first?.id
    }

    var isAvailable: Bool { !computers.isEmpty }
    var selectedName: String { computers.first { $0.id == selectedComputerId }?.name ?? "the computer" }

    func send(finish: @escaping () -> Void) {
        guard let computerId = selectedComputerId else { return }
        let client = store.makeClient(computerId: computerId)
        Task {
            do {
                if staged.isEmpty { staged = try await Self.stage(providers) }
                guard !staged.isEmpty else { throw FilesError.invalidResponse("There was nothing to share.") }
                try await upload(staged, client: client)
                finish()
            } catch FilesError.wakeConsentRequired(let hint, _) {
                if hint?.setting == .never {
                    phase = .failed("\(selectedName) is asleep and set to never wake for Files. Nothing was sent.")
                } else {
                    pendingHint = hint
                    phase = .wake
                }
            } catch {
                phase = .failed(Self.describe(error))
            }
        }
    }

    /// The user agreed to wake the computer (it bills credits while awake).
    func wakeAndSend(finish: @escaping () -> Void) {
        guard let computerId = selectedComputerId else { return }
        let client = store.makeClient(computerId: computerId)
        let hint = pendingHint
        phase = .sending(done: 0, total: 0)
        Task {
            do {
                try await client.requestWake(reason: .upload, hint: hint)
                send(finish: finish)
            } catch {
                phase = .failed(Self.describe(error))
            }
        }
    }

    private func upload(_ urls: [URL], client: FilesClient) async throws {
        let drop = FilesInboxDrop(client: client)
        let inbox = try await drop.inboxRoot()
        let total = urls.reduce(UInt64(0)) { $0 + Self.size($1) }
        var sent: UInt64 = 0
        phase = .sending(done: 0, total: total)
        var items: [ItemId] = []
        for url in urls {
            let base = sent
            let result = try await drop.send(fileAt: url, name: url.lastPathComponent, to: inbox, wake: .whenAutomatic) { [weak self] done, _ in
                Task { @MainActor in self?.phase = .sending(done: base + done, total: total) }
            }
            sent += Self.size(url)
            items.append(result.item.id)
        }
        if askAgent {
            for item in items { _ = await drop.askAgent(itemId: item) }
        }
        for url in urls { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
        staged = []
    }

    // MARK: Staging

    /// Copies every shared item to a temporary file. Text and web links go
    /// in one "Shared text.txt".
    nonisolated static func stage(_ providers: [NSItemProvider]) async throws -> [URL] {
        var urls: [URL] = []
        var texts: [String] = []
        for provider in providers {
            if !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
               provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
               let link = await loadURL(provider), !link.isFileURL {
                texts.append(link.absoluteString)
            } else if let type = fileType(of: provider), let url = await copy(provider, type: type) {
                urls.append(url)
            } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
                      let text = await loadText(provider), !text.isEmpty {
                texts.append(text)
            }
        }
        if !texts.isEmpty {
            let folder = try newFolder()
            let url = folder.appendingPathComponent("Shared text.txt")
            try texts.joined(separator: "\n\n").write(to: url, atomically: true, encoding: .utf8)
            urls.append(url)
        }
        return urls
    }

    private nonisolated static func newFolder() throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FilesShare-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    private nonisolated static func fileType(of provider: NSItemProvider) -> UTType? {
        if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) { return .image }
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) { return .data }
        for id in provider.registeredTypeIdentifiers {
            guard let t = UTType(id) else { continue }
            if t.conforms(to: .url) || t.conforms(to: .text) { continue }
            if t.conforms(to: .data) { return t }
        }
        return nil
    }

    private nonisolated static func copy(_ provider: NSItemProvider, type: UTType) async -> URL? {
        let suggested = provider.suggestedName
        return await withCheckedContinuation { (c: CheckedContinuation<URL?, Never>) in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                guard let url, let folder = try? newFolder() else { c.resume(returning: nil); return }
                var name = url.lastPathComponent.isEmpty ? (suggested ?? "file") : url.lastPathComponent
                name = name.replacingOccurrences(of: "/", with: "-")
                let dest = folder.appendingPathComponent(name.isEmpty ? "file" : name)
                do {
                    try FileManager.default.copyItem(at: url, to: dest)
                    c.resume(returning: dest)
                } catch {
                    c.resume(returning: nil)
                }
            }
        }
    }

    private nonisolated static func loadURL(_ provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { (c: CheckedContinuation<URL?, Never>) in
            provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { value, _ in
                c.resume(returning: (value as? URL) ?? (value as? String).flatMap(URL.init(string:)))
            }
        }
    }

    private nonisolated static func loadText(_ provider: NSItemProvider) async -> String? {
        await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            provider.loadItem(forTypeIdentifier: UTType.plainText.identifier, options: nil) { value, _ in
                c.resume(returning: (value as? String) ?? (value as? URL)?.absoluteString)
            }
        }
    }

    private nonisolated static func size(_ url: URL) -> UInt64 {
        (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.uint64Value ?? 0
    }

    private static func describe(_ error: Error) -> String {
        if let files = error as? FilesError { return (files.errorDescription ?? "Couldn't send.") + " Nothing more was sent." }
        return "Couldn't send: \(error.localizedDescription)"
    }
}

/// The computer half of the share sheet.
struct FilesShareView: View {
    @ObservedObject var model: FilesShareModel
    let finish: () -> Void

    var body: some View {
        switch model.phase {
        case .picking:
            Form {
                Section {
                    ForEach(model.computers) { computer in
                        Button { model.selectedComputerId = computer.id } label: {
                            HStack {
                                Image(systemName: "desktopcomputer").foregroundStyle(.secondary)
                                Text(computer.name).foregroundStyle(.primary)
                                Spacer()
                                if model.selectedComputerId == computer.id { Image(systemName: "checkmark") }
                            }
                        }
                    }
                } header: {
                    Text("Save to Inbox on")
                } footer: {
                    Text("Files in Inbox wait for your OK before your agent reads them.")
                }
                Section {
                    Toggle("Ask my agent to read it", isOn: $model.askAgent)
                } footer: {
                    Text("Your agent reads only these files, summarizes them with no other tools, and saves the summary next to them.")
                }
                Section {
                    Button("Save") { model.send(finish: finish) }.disabled(model.selectedComputerId == nil)
                }
            }
        case .sending(let done, let total):
            VStack(spacing: 12) {
                if total > 0 {
                    ProgressView(value: Double(done), total: Double(total))
                    Text("Saving to \(model.selectedName)…").font(.footnote).foregroundStyle(.secondary)
                } else {
                    ProgressView("Saving to \(model.selectedName)…")
                }
            }
            .padding()
        case .wake:
            VStack(spacing: 16) {
                Image(systemName: "moon.fill").font(.title2).foregroundStyle(.secondary)
                Text("\(model.selectedName) is asleep").font(.headline)
                Text("Waking takes about 40 seconds, and while it is awake it bills credits at your plan's awake rate.")
                    .multilineTextAlignment(.center).font(.subheadline).foregroundStyle(.secondary)
                Button("Wake and save") { model.wakeAndSend(finish: finish) }.buttonStyle(.borderedProminent)
                Button("Not now") { model.phase = .picking }
            }
            .padding()
        case .done(let message):
            VStack(spacing: 16) {
                Text(message).multilineTextAlignment(.center)
                Button("Done", action: finish).buttonStyle(.borderedProminent)
            }.padding()
        case .failed(let message):
            VStack(spacing: 16) {
                Text(message).multilineTextAlignment(.center)
                Button("Back") { model.phase = .picking }
            }.padding()
        }
    }
}
