import SwiftUI
import UIKit
import UniformTypeIdentifiers
import AllternitFilesKit

/// Where a Files screen goes next (pushed in the computer detail's stack, or
/// the detail column on iPad).
/// A file picked for its sheet (`Item` itself is a generated contract type).
struct FilesSelection: Identifiable {
    let item: Item
    var id: ItemId { item.id }
}

enum FilesRoute: Hashable {
    case roots
    case folder(rootId: RootId, parentId: ItemId, title: String)
    case requests
}

// MARK: - Shared row style (design 4A: inset grouped rows on --neutral-fill)

struct FilesRowLabel: View {
    let systemImage: String
    let title: String
    var subtitle: String? = nil
    var trailing: AnyView? = nil

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(size: 18))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if let trailing { trailing }
        }
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }
}

extension View {
    /// White page, rows on the neutral fill, graphite in dark mode (asset colors).
    func filesListStyle() -> some View {
        self.listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
    }
}

/// "Asleep. Saved list from 2:14 PM." with a Wake button (opens the sheet).
struct FilesAsleepBanner: View {
    @ObservedObject var model: FilesComputerModel
    var onWake: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "moon.fill").foregroundColor(Color("TextSecondary"))
            Text(model.stateLine)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            if model.wakeSetting != .never {
                Button("Wake", action: onWake)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .buttonStyle(.plain)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Bottom bar while files move: "Uploading receipt.jpg · 42%".
struct FilesTransferBar: View {
    @ObservedObject var model: FilesComputerModel

    var body: some View {
        if let first = model.transfers.first {
            HStack(spacing: 10) {
                ProgressView(value: first.total == 0 ? 0 : Double(first.done) / Double(first.total))
                    .frame(width: 80)
                Text(model.transfers.count == 1 ? first.name : "\(first.name) and \(model.transfers.count - 1) more")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Color("BgPanel"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            .padding(.bottom, 12)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Transferring \(first.name)")
        }
    }
}

// MARK: - Roots (the computer's folders)

/// The computer's roots: Inbox, Shared, Agent Output, Downloads, Meetings,
/// Scans, then Memory, Code, Models and folders the user added.
struct FilesRootsScreen: View {
    @ObservedObject var model: FilesComputerModel
    @State private var openedDisk: Root?
    @State private var openingDisk: String?

    var body: some View {
        List {
            if model.isAsleep {
                Section { FilesAsleepBanner(model: model) { model.askToWake() } }
                    .listRowBackground(Color("FillNeutral"))
            }
            if !model.pendingTasks.isEmpty {
                Section {
                    NavigationLink(value: FilesRoute.requests) {
                        FilesRowLabel(systemImage: "checkmark.shield", title: "Inbox requests",
                                      subtitle: FilesRootsScreen.requestsLine(model.pendingTasks.count))
                    }
                }
                .listRowBackground(Color("FillNeutral"))
            }
            if model.isLoading {
                Section { ProgressView().frame(maxWidth: .infinity) }.listRowBackground(Color.clear)
            } else if let error = model.loadError, model.roots.isEmpty {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load Files",
                                  message: error, actionTitle: "Retry", action: { Task { await model.load() } })
                    .listRowBackground(Color.clear)
            } else {
                Section {
                    ForEach(model.driveRoots, id: \.id) { root in
                        NavigationLink(value: FilesRoute.folder(rootId: root.id, parentId: root.rootItemId, title: root.name)) {
                            FilesRowLabel(systemImage: FilesFormat.symbol(root.kind), title: root.name,
                                          subtitle: root.usageBytes.map(FilesFormat.bytes))
                        }
                    }
                }
                .listRowBackground(Color("FillNeutral"))
                if !model.diskRoots.isEmpty { wholeDiskSection }
            }
        }
        .filesListStyle()
        .navigationDestination(item: $openedDisk) { root in
            FilesFolderScreen(model: model, rootId: root.id, parentId: root.rootItemId, title: root.name)
        }
        .navigationTitle("Files")
        .refreshable { await model.load() }
    }

    /// Whole-disk roots after the Files drive; shown only while the setting is on.
    @ViewBuilder private var wholeDiskSection: some View {
        Section {
            ForEach(model.diskRoots) { disk in
                Button {
                    Task {
                        openingDisk = disk.id
                        defer { openingDisk = nil }
                        do { openedDisk = try await model.diskRoot(disk) }
                        catch {
                            if (error as? FilesError)?.isWholeDiskOff != true { model.notice = FilesComputerModel.message(error) }
                        }
                    }
                } label: {
                    FilesRowLabel(systemImage: "internaldrive", title: disk.label, subtitle: disk.path)
                }
                .disabled(openingDisk != nil)
            }
            if let message = model.wholeDiskOffMessage {
                VStack(alignment: .leading, spacing: 8) {
                    Text(message).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                    if let open = model.openSettings { ACIPillButton(title: "Open Settings", action: open) }
                }
            }
        } header: {
            Text("Whole disk")
        }
        .listRowBackground(Color("FillNeutral"))
    }

    static func requestsLine(_ count: Int) -> String {
        count == 1 ? "1 file waits for your OK" : "\(count) files wait for your OK"
    }
}

// MARK: - A folder

struct FilesFolderScreen: View {
    @ObservedObject var model: FilesComputerModel
    let rootId: RootId
    let parentId: ItemId
    let title: String

    @State private var items: [Item] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var isLocked = false
    @State private var diskOff = false
    @State private var selected: FilesSelection?
    @State private var isAddOpen = false
    @State private var isDropTargeted = false

    private var root: Root? { model.root(id: rootId) }
    private var canWrite: Bool { root?.policy.write ?? false }

    var body: some View {
        List {
            if model.isAsleep {
                Section { FilesAsleepBanner(model: model) { model.askToWake() } }
                    .listRowBackground(Color("FillNeutral"))
            }
            if root?.kind == .inbox, parentId == root?.rootItemId, !model.pendingTasks.isEmpty {
                Section {
                    NavigationLink(value: FilesRoute.requests) {
                        FilesRowLabel(systemImage: "checkmark.shield", title: "Inbox requests",
                                      subtitle: FilesRootsScreen.requestsLine(model.pendingTasks.count))
                    }
                }
                .listRowBackground(Color("FillNeutral"))
            }
            if isLoading && items.isEmpty {
                Section { ProgressView().frame(maxWidth: .infinity) }.listRowBackground(Color.clear)
            } else if isLocked, items.isEmpty {
                FriendlyStateView(style: .empty, icon: "lock.fill", title: "\(title) is protected",
                                  message: "Unlock it with Face ID, Touch ID or your passcode. It stays open on this iPhone for 5 minutes.",
                                  actionTitle: "Unlock", action: { Task { if await model.unlock("Unlock \(title) on \(model.name)") { await reload() } } })
                    .listRowBackground(Color.clear)
            } else if let error, items.isEmpty {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't open \(title)",
                                  message: error, actionTitle: diskOff ? (model.openSettings != nil ? "Open Settings" : nil) : "Retry",
                                  action: { if diskOff { model.openSettings?() } else { Task { await reload() } } })
                    .listRowBackground(Color.clear)
            } else if items.isEmpty {
                FriendlyStateView(style: .empty, icon: "folder", title: "Nothing here yet",
                                  message: canWrite ? "Tap + to add a photo, a file or a scan, or drop files here." : "This folder is empty.",
                                  art: .sheets)
                    .listRowBackground(Color.clear)
            } else {
                Section {
                    ForEach(items, id: \.id) { item in row(item) }
                }
                .listRowBackground(Color("FillNeutral"))
            }
        }
        .filesListStyle()
        .overlay {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: 16)
                    .stroke(Color("TextPrimary"), style: StrokeStyle(lineWidth: 2, dash: [6]))
                    .padding(8)
                    .allowsHitTesting(false)
            }
        }
        .onDrop(of: [.item], isTargeted: $isDropTargeted) { providers in
            guard canWrite else { return false }
            Task { await drop(providers) }
            return true
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            if canWrite {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { isAddOpen = true } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Add to \(title)")
                }
            }
        }
        .sheet(item: $selected) { selection in
            FilesItemSheet(model: model, item: selection.item, rootKind: root?.kind)
        }
        .sheet(isPresented: $isAddOpen) {
            FilesAddSheet(model: model, rootId: rootId, parentId: parentId, folderName: title)
        }
        .refreshable { await reload() }
        .task(id: model.revision) { await reload() }
    }

    @ViewBuilder
    private func row(_ item: Item) -> some View {
        if item.kind == .folder {
            NavigationLink(value: FilesRoute.folder(rootId: rootId, parentId: item.id, title: item.name)) {
                FilesRowLabel(systemImage: "folder", title: item.name, subtitle: FilesFormat.date(item.modifiedAt))
            }
        } else {
            Button { selected = FilesSelection(item: item) } label: {
                FilesRowLabel(systemImage: Self.symbol(for: item), title: item.name,
                              subtitle: "\(FilesFormat.bytes(item.size)) · \(FilesFormat.date(item.modifiedAt))",
                              trailing: badge(item))
            }
            .buttonStyle(.plain)
            .onDrag { FilesComputerModel.dragProvider(for: item, client: model.client) }
            .accessibilityHint("Shows what you can do with this file")
        }
    }

    /// Design badge set A (colour chips), as glyphs in the row.
    private func badge(_ item: Item) -> AnyView? {
        if model.pendingTask(for: item) != nil {
            return AnyView(Image(systemName: "bolt.fill").foregroundColor(Theme.statusInfo).accessibilityLabel("Waits for your OK"))
        }
        if item.name.contains("(conflict") {
            return AnyView(Image(systemName: "exclamationmark.triangle.fill").foregroundColor(Theme.statusWarning).accessibilityLabel("Conflict"))
        }
        if model.isKeptOffline(item) {
            return AnyView(Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess).accessibilityLabel("Kept on this iPhone"))
        }
        return nil
    }

    private func reload() async {
        isLoading = true
        defer { isLoading = false }
        do {
            items = try await model.list(rootId: rootId, parentId: parentId).items
            error = nil
            isLocked = false
            diskOff = false
        } catch let failure as FilesError where failure.code == .protectedRootLocked {
            isLocked = true
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FilesComputerModel.message(error)
            diskOff = (error as? FilesError)?.isWholeDiskOff == true
            model.noteWholeDiskOff(error)
        }
    }

    /// Files dropped from another app (iPad) or another screen.
    private func drop(_ providers: [NSItemProvider]) async {
        var urls: [URL] = []
        for provider in providers {
            if let url = await FilesComputerModel.copyToTemporary(provider) { urls.append(url) }
        }
        guard !urls.isEmpty else { return }
        let (rootId, parentId) = (self.rootId, self.parentId)
        await model.needingWake(.upload) { [model] in
            try await model.upload(urls, rootId: rootId, parentId: parentId)
        }
    }

    static func symbol(for item: Item) -> String {
        let type = UTType(filenameExtension: (item.name as NSString).pathExtension)
        if type?.conforms(to: .image) == true { return "photo" }
        if type?.conforms(to: .movie) == true || type?.conforms(to: .audiovisualContent) == true { return "film" }
        if type?.conforms(to: .pdf) == true { return "doc.richtext" }
        if type?.conforms(to: .spreadsheet) == true { return "tablecells" }
        if type?.conforms(to: .text) == true || type?.conforms(to: .sourceCode) == true { return "doc.text" }
        return "doc"
    }
}

// MARK: - Drag and drop

extension FilesComputerModel {
    /// Drag a file out (to Files, Mail or another app on iPad): the bytes are
    /// fetched only when the drop lands.
    nonisolated static func dragProvider(for item: Item, client: FilesClient) -> NSItemProvider {
        let provider = NSItemProvider()
        provider.suggestedName = item.name
        let type = UTType(filenameExtension: (item.name as NSString).pathExtension) ?? .data
        provider.registerFileRepresentation(forTypeIdentifier: type.identifier, fileOptions: [], visibility: .all) { completion in
            let progress = Progress(totalUnitCount: Int64(clamping: item.size))
            Task {
                do {
                    let folder = FileManager.default.temporaryDirectory
                        .appendingPathComponent("FilesDrag-\(UUID().uuidString)", isDirectory: true)
                    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                    let url = folder.appendingPathComponent(safeName(item.name))
                    try await client.download(itemId: item.id, rev: item.rev, size: item.size, contentHash: item.contentHash,
                                              rootId: item.rootId, to: url, wake: .whenAutomatic) { done, _ in
                        progress.completedUnitCount = Int64(clamping: done)
                    }
                    completion(url, false, nil)
                } catch {
                    completion(nil, false, error)
                }
            }
            return progress
        }
        return provider
    }

    /// Copies a dropped or shared item to a temporary file (the system's copy
    /// goes away when its callback returns).
    nonisolated static func copyToTemporary(_ provider: NSItemProvider) async -> URL? {
        let type = provider.registeredTypeIdentifiers.compactMap(UTType.init).first { $0.conforms(to: .data) || $0.conforms(to: .package) }
            ?? .data
        let suggested = provider.suggestedName
        return await withCheckedContinuation { (continuation: CheckedContinuation<URL?, Never>) in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                guard let url else { continuation.resume(returning: nil); return }
                let folder = FileManager.default.temporaryDirectory
                    .appendingPathComponent("FilesDrop-\(UUID().uuidString)", isDirectory: true)
                var name = url.lastPathComponent
                if let suggested, !suggested.isEmpty, (suggested as NSString).pathExtension.isEmpty == false || name.hasPrefix(".") {
                    name = suggested
                }
                let dest = folder.appendingPathComponent(safeName(name))
                do {
                    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                    try FileManager.default.copyItem(at: url, to: dest)
                    continuation.resume(returning: dest)
                } catch {
                    continuation.resume(returning: nil)
                }
            }
        }
    }

    /// The wake sheet without a pending action: the Wake button on the
    /// asleep banner. After waking, the folders list again.
    func askToWake() {
        wakePrompt = WakePrompt(reason: .sync, hint: nil) { [weak self] in await self?.load() }
    }
}
