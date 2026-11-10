import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Share extension (plan step I7). Takes text, links, images and files, lets
/// the person pick a bot and add a note, asks for one-time consent per bot,
/// and writes the result to `OutboxStore`. It never sends: the app drains the
/// outbox (`Core/ShareOutboxDrainer.swift`).
final class ShareViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        let model = ShareModel(
            providers: (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] },
            finish: { [weak self] in self?.extensionContext?.completeRequest(returningItems: [], completionHandler: nil) },
            cancel: { [weak self] in
                self?.extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
            },
            openApp: { [weak self] in self?.openAllternit() }
        )
        let host = UIHostingController(rootView: ShareView(model: model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
    }

    /// Share extensions have no public way to open the host app; walk the
    /// responder chain to `UIApplication` and call `open(_:options:completionHandler:)`.
    private func openAllternit() {
        guard let url = URL(string: "allternit://") else { return }
        var responder: UIResponder? = self
        while let r = responder {
            if r is UIApplication {
                let sel = NSSelectorFromString("openURL:options:completionHandler:")
                if r.responds(to: sel), let imp = r.method(for: sel) {
                    typealias Open = @convention(c) (AnyObject, Selector, URL, [AnyHashable: Any], Any?) -> Void
                    unsafeBitCast(imp, to: Open.self)(r, sel, url, [:], nil)
                }
                break
            }
            responder = r.next
        }
        extensionContext?.completeRequest(returningItems: [], completionHandler: nil)
    }
}

// MARK: - Model

/// `NSItemProvider` is not Sendable; every use here stays on the main actor.
@MainActor
final class ShareModel: ObservableObject {
    enum Phase: Equatable {
        case needsApp(String)
        case picking
        case consent
        case saving
        case failed(String)
    }

    @Published var phase: Phase
    @Published var selectedBotId: String?
    @Published var note = ""
    let bots: [StatusSnapshot.Bot]

    private let providers: [NSItemProvider]
    private let finish: () -> Void
    private let cancelAction: () -> Void
    private let openAppAction: () -> Void

    init(providers: [NSItemProvider],
         finish: @escaping () -> Void,
         cancel: @escaping () -> Void,
         openApp: @escaping () -> Void) {
        self.providers = providers
        self.finish = finish
        self.cancelAction = cancel
        self.openAppAction = openApp
        let snapshot = AppGroupStore.snapshot
        bots = snapshot?.bots.filter { !$0.botId.isEmpty } ?? []
        if !AppGroupStore.signedIn || SharedAccess.readToken() == nil {
            phase = .needsApp("Sign in to Allternit to share to a bot.")
        } else if bots.isEmpty {
            phase = .needsApp("No bots yet. Open Allternit once so your bots show up here.")
        } else {
            phase = .picking
            selectedBotId = bots.first?.botId
        }
    }

    var selectedBot: StatusSnapshot.Bot? { bots.first { $0.botId == selectedBotId } }
    var providerName: String? { selectedBotId.flatMap { OutboxStore.botProviders[$0] } }
    var consentLine: String {
        let bot = selectedBot?.name ?? "this bot"
        let provider = providerName.map { "which runs on \($0)" } ?? "which runs on its configured model provider"
        return "This will be sent to \(bot), \(provider)."
    }
    var itemSummary: String {
        let n = providers.count
        return n == 1 ? "1 item" : "\(n) items"
    }

    func cancel() { cancelAction() }
    func openApp() { openAppAction() }

    func send() {
        guard let bot = selectedBot else { return }
        if !OutboxStore.hasConsent(botId: bot.botId) { phase = .consent; return }
        save(bot)
    }

    func consentAndSend() {
        guard let bot = selectedBot else { return }
        OutboxStore.recordConsent(botId: bot.botId)
        save(bot)
    }

    // MARK: Saving

    private func save(_ bot: StatusSnapshot.Bot) {
        phase = .saving
        Task {
            do {
                var item = OutboxStore.Item(
                    id: UUID().uuidString, createdAt: Date(),
                    botId: bot.botId, botName: bot.name,
                    note: note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : note
                )
                try await collect(into: &item)
                guard item.text != nil || item.url != nil || !item.files.isEmpty || item.note != nil else {
                    throw ShareError.empty
                }
                guard OutboxStore.save(item) else { throw ShareError.storage }
                finish()
            } catch {
                phase = .failed((error as? ShareError)?.message ?? "Couldn't save this share. Nothing was sent.")
            }
        }
    }

    private enum ShareError: Error {
        case tooBig(String)
        case tooMany
        case empty
        case storage

        var message: String {
            switch self {
            case .tooBig(let name):
                return "\(name) is larger than \(OutboxStore.maxFileBytes / 1_048_576) MB, which is the most Allternit takes from the share sheet. Nothing was sent."
            case .tooMany: return "Share up to \(OutboxStore.maxFilesPerItem) files at a time. Nothing was sent."
            case .empty: return "There was nothing to share."
            case .storage: return "Allternit's shared storage isn't available. Nothing was sent."
            }
        }
    }

    /// Reads every provider: a web link or text inline, images and files
    /// copied into the App Group (refused above the size cap).
    private func collect(into item: inout OutboxStore.Item) async throws {
        var texts: [String] = []
        var urls: [String] = []
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) == false,
               provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
               let url = await loadURL(provider), !url.isFileURL {
                urls.append(url.absoluteString)
            } else if let type = Self.fileType(of: provider) {
                guard item.files.count < OutboxStore.maxFilesPerItem else { throw ShareError.tooMany }
                item.files.append(try await copyFile(provider, type: type, itemId: item.id))
            } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
                      let text = await loadText(provider), !text.isEmpty {
                texts.append(text)
            }
        }
        if !texts.isEmpty { item.text = texts.joined(separator: "\n\n") }
        if !urls.isEmpty { item.url = urls.joined(separator: "\n") }
    }

    /// The type to copy a file-like provider as: images and file URLs and
    /// other data, but not a web link or plain text.
    private static func fileType(of provider: NSItemProvider) -> UTType? {
        if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) { return .image }
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) { return .data }
        for id in provider.registeredTypeIdentifiers {
            guard let t = UTType(id) else { continue }
            if t.conforms(to: .url) || t.conforms(to: .text) { continue }
            if t.conforms(to: .data) { return t }
        }
        return nil
    }

    private func loadURL(_ provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { (c: CheckedContinuation<URL?, Never>) in
            provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { value, _ in
                c.resume(returning: (value as? URL) ?? (value as? String).flatMap(URL.init(string:)))
            }
        }
    }

    private func loadText(_ provider: NSItemProvider) async -> String? {
        await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            provider.loadItem(forTypeIdentifier: UTType.plainText.identifier, options: nil) { value, _ in
                c.resume(returning: (value as? String) ?? (value as? URL)?.absoluteString)
            }
        }
    }

    /// Copies the provider's file into the item's folder. The temp file the
    /// system hands over disappears when the callback returns, so the size
    /// check and copy happen inside it.
    private func copyFile(_ provider: NSItemProvider, type: UTType, itemId: String) async throws -> OutboxStore.FileRef {
        guard let folder = OutboxStore.filesFolder(for: itemId) else { throw ShareError.storage }
        let suggested = provider.suggestedName
        let limit = OutboxStore.maxFileBytes
        let result: Result<OutboxStore.FileRef, Error> = await withCheckedContinuation { c in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, error in
                guard let url else { c.resume(returning: .failure(error ?? ShareError.storage)); return }
                let name = Self.cleanName(url.lastPathComponent.isEmpty ? (suggested ?? "file") : url.lastPathComponent)
                let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
                if size > limit { c.resume(returning: .failure(ShareError.tooBig(name))); return }
                let dest = folder.appendingPathComponent(name)
                do {
                    try? FileManager.default.removeItem(at: dest)
                    try FileManager.default.copyItem(at: url, to: dest)
                } catch { c.resume(returning: .failure(ShareError.storage)); return }
                let mime = UTType(filenameExtension: dest.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                c.resume(returning: .success(OutboxStore.FileRef(
                    name: name, mediaType: mime, size: size, relativePath: "\(itemId)/\(name)")))
            }
        }
        return try result.get()
    }

    private nonisolated static func cleanName(_ name: String) -> String {
        let cleaned = name.replacingOccurrences(of: "/", with: "-")
        return cleaned.isEmpty ? "file" : cleaned
    }
}

// MARK: - View

struct ShareView: View {
    @ObservedObject var model: ShareModel

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Share to Allternit")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { model.cancel() } }
                }
        }
    }

    @ViewBuilder private var content: some View {
        switch model.phase {
        case .needsApp(let message):
            VStack(spacing: 16) {
                Text(message).multilineTextAlignment(.center)
                Button("Open Allternit") { model.openApp() }.buttonStyle(.borderedProminent)
            }.padding()
        case .picking:
            Form {
                Section("Send \(model.itemSummary) to") {
                    ForEach(model.bots, id: \.botId) { bot in
                        Button { model.selectedBotId = bot.botId } label: {
                            HStack {
                                Text(bot.name.isEmpty ? bot.botId : bot.name).foregroundStyle(.primary)
                                Spacer()
                                if model.selectedBotId == bot.botId { Image(systemName: "checkmark") }
                            }
                        }
                    }
                }
                Section("Note (optional)") {
                    TextField("Add a note", text: $model.note, axis: .vertical).lineLimit(1...4)
                }
                Section {
                    Button("Send") { model.send() }.disabled(model.selectedBotId == nil)
                }
            }
        case .consent:
            VStack(spacing: 16) {
                Text(model.consentLine).multilineTextAlignment(.center)
                Text("You will only be asked this once for this bot.").font(.footnote).foregroundStyle(.secondary)
                Button("Send") { model.consentAndSend() }.buttonStyle(.borderedProminent)
                Button("Back") { model.phase = .picking }
            }.padding()
        case .saving:
            ProgressView("Saving").padding()
        case .failed(let message):
            VStack(spacing: 16) {
                Text(message).multilineTextAlignment(.center)
                Button("Back") { model.phase = .picking }
            }.padding()
        }
    }
}
