import Foundation
import SwiftUI
import LocalAuthentication
import AllternitFilesKit

/// State behind one computer's in-app Files screens (design 4A: a Files row
/// in the computer detail that pushes these screens; no new rail tab).
///
/// Browsing never wakes the computer: an asleep computer answers from its
/// snapshot (`freshness.served_by == snapshot`). Opening, saving, uploading
/// and scanning need it awake. With the computer's wake setting `auto` the
/// client wakes it silently; with `ask` the screen shows the wake sheet and
/// runs the action again after the user agrees; with `never` it says so.
@MainActor
final class FilesComputerModel: ObservableObject {
    struct Transfer: Identifiable, Equatable {
        let id = UUID()
        var name: String
        var done: UInt64 = 0
        var total: UInt64 = 0
    }

    /// The wake sheet: what was asked, and the action to run after waking.
    struct WakePrompt: Identifiable {
        let id = UUID()
        let reason: WakeReason
        let hint: WakeHint?
        let retry: @MainActor () async -> Void
    }

    let computerId: ComputerId
    let name: String
    let client: FilesClient

    @Published private(set) var roots: [Root] = []
    @Published private(set) var freshness: Freshness?
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String?
    @Published private(set) var pendingTasks: [FilesInboxTask] = []
    @Published private(set) var transfers: [Transfer] = []
    @Published var wakeSetting: WakeSetting?
    @Published var wakePrompt: WakePrompt?
    @Published var notice: String?
    /// Bumped after a write, so open folder screens list again.
    @Published private(set) var revision = 0

    /// Whole-disk roots from the console row (`wholeDisk.roots`), set only while the setting is on.
    @Published var diskRoots: [ACIComputersClient.WholeDiskRoot] = []
    /// The server's own message after a 403 `whole_disk_off`, with a way to the Settings switch.
    @Published private(set) var wholeDiskOffMessage: String?
    /// Opens the console Settings tab; set by the Files tab.
    var openSettings: (() -> Void)?

    init(computerId: ComputerId, name: String) {
        self.computerId = computerId
        self.name = name
        self.client = FilesDrive.shared.client(for: computerId)
    }

    var isAsleep: Bool { freshness?.servedBy == .snapshot || freshness?.computerState == .asleep }

    var stateLine: String {
        guard let freshness else { return "" }
        switch freshness.computerState {
        case .awake: return "Awake"
        case .waking: return "Waking up"
        case .offline: return "Offline"
        case .asleep:
            let at = FilesFormat.time(freshness.snapshotAt)
            return at.isEmpty ? "Asleep. Saved list." : "Asleep. Saved list from \(at)."
        }
    }

    func root(_ kind: RootKind) -> Root? { roots.first { $0.kind == kind } }
    func root(id: RootId) -> Root? { roots.first { $0.id == id } }

    var sortedRoots: [Root] {
        roots.sorted { (FilesFormat.order($0.kind), $0.name) < (FilesFormat.order($1.kind), $1.name) }
    }

    // MARK: Loading

    func load() async {
        isLoading = roots.isEmpty
        defer { isLoading = false }
        do {
            let response = try await client.roots()
            roots = response.roots
            freshness = response.freshness
            loadError = nil
        } catch is CancellationError {
        } catch {
            loadError = Self.message(error)
        }
        async let tasks: Void = loadTasks()
        async let wake: Void = loadWakeSetting()
        _ = await (tasks, wake)
    }

    func loadTasks() async {
        guard !isAsleep else { pendingTasks = []; return }
        pendingTasks = (try? await client.inboxTasks(state: "needs_approval")) ?? pendingTasks
    }

    private func loadWakeSetting() async {
        if let wake = try? await FilesCloudAPI.wakeSetting(computerId) { wakeSetting = wake }
    }

    /// Every page of one folder (folders first, then by name).
    func list(rootId: RootId, parentId: ItemId) async throws -> (items: [Item], freshness: Freshness) {
        var items: [Item] = []
        var token: PageToken?
        var freshness: Freshness
        repeat {
            let page = try await client.list(rootId: rootId, parentId: parentId, pageToken: token, limit: 500)
            items += page.items
            token = page.nextPageToken
            freshness = page.freshness
        } while token != nil && items.count < 5_000
        self.freshness = freshness
        items.sort { ($0.kind == .folder ? 0 : 1, $0.name.localizedLowercase) < ($1.kind == .folder ? 0 : 1, $1.name.localizedLowercase) }
        return (items, freshness)
    }

    /// A root for a whole-disk path: the one the computer already has, else it is added first.
    /// Throws `FilesError.wholeDiskOff` (message kept) when the setting is off on the server.
    func diskRoot(_ disk: ACIComputersClient.WholeDiskRoot) async throws -> Root {
        if let existing = roots.first(where: { $0.pathHint == disk.path }) { return existing }
        do {
            let added = try await client.addRoot(name: disk.label, localPath: disk.path)
            if !roots.contains(where: { $0.id == added.id }) { roots.append(added) }
            wholeDiskOffMessage = nil
            return added
        } catch let failure as FilesError where failure.isWholeDiskOff {
            wholeDiskOffMessage = failure.localizedDescription
            throw failure
        }
    }

    /// Roots shown first: the Files drive folders, without the whole-disk roots added from the switcher.
    var driveRoots: [Root] {
        sortedRoots.filter { root in !diskRoots.contains { $0.path == root.pathHint } }
    }

    /// Remember a 403 `whole_disk_off` seen anywhere in Files so the screen can offer the switch.
    func noteWholeDiskOff(_ error: Error) {
        if let failure = error as? FilesError, failure.isWholeDiskOff { wholeDiskOffMessage = failure.localizedDescription }
    }

    // MARK: Actions that may need the computer awake

    /// Runs `action`; a missing wake consent turns into the wake sheet (or a
    /// notice when the setting is `never`), and the sheet runs it again.
    func needingWake(_ reason: WakeReason, _ action: @escaping @MainActor () async throws -> Void) async {
        do {
            try await action()
        } catch FilesError.wakeConsentRequired(let hint, _) {
            let setting = hint?.setting ?? wakeSetting ?? .ask
            if setting == .never {
                notice = "\(name) is set to never wake for Files. Change it in Settings > Files."
            } else {
                wakePrompt = WakePrompt(reason: reason, hint: hint) { [weak self] in
                    await self?.needingWake(reason, action)
                }
            }
        } catch let error as FilesError where error.code == .protectedRootLocked {
            // The user's tap started this, so Face ID asks right away, then
            // the action runs once more with the signed unlock token.
            if await unlock("Open protected files on \(name)") {
                do { try await action() } catch { notice = Self.message(error) }
            }
        } catch is CancellationError {
        } catch {
            notice = Self.message(error)
        }
    }

    /// Protected roots (Memory Drive, SPEC section 12): Face ID / Touch ID or
    /// the passcode, then a signed 5-minute unlock token from cloud-api that
    /// this app and the File Provider extension send. False when the user
    /// cancels; a failure shows as a notice.
    func unlock(_ reason: String) async -> Bool {
        do {
            try await client.unlock(reason: reason)
            return true
        } catch let error as LAError where [.userCancel, .appCancel, .systemCancel].contains(error.code) {
            return false
        } catch {
            notice = Self.message(error)
            return false
        }
    }

    /// The user tapped Wake on the sheet.
    func wake(_ prompt: WakePrompt, remember: Bool) async {
        wakePrompt = nil
        do {
            if remember {
                wakeSetting = try await FilesCloudAPI.setWakeSetting(.auto, for: computerId)
            }
            try await client.requestWake(reason: prompt.reason, hint: prompt.hint)
            await prompt.retry()
            await load()
        } catch {
            notice = Self.message(error)
        }
    }

    /// Downloads `item` (once per revision) for Quick Look or the share sheet.
    func localCopy(of item: Item) async throws -> URL {
        let folder = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("FilesOpen", isDirectory: true)
            .appendingPathComponent(FilesClient.pathSegment(item.id), isDirectory: true)
            .appendingPathComponent(FilesClient.pathSegment(item.rev), isDirectory: true)
        let url = folder.appendingPathComponent(Self.safeName(item.name))
        if FileManager.default.fileExists(atPath: url.path) { return url }
        try? FileManager.default.removeItem(at: folder.deletingLastPathComponent())
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let transfer = Transfer(name: item.name, total: item.size)
        transfers.append(transfer)
        defer { transfers.removeAll { $0.id == transfer.id } }
        try await client.download(itemId: item.id, rev: item.rev, size: item.size, contentHash: item.contentHash,
                                  rootId: item.rootId, to: url, wake: .whenAutomatic) { [weak self] done, _ in
            Task { @MainActor in
                guard let self, let index = self.transfers.firstIndex(where: { $0.id == transfer.id }) else { return }
                self.transfers[index].done = done
            }
        }
        return url
    }

    /// Uploads local files into a folder. Each lands under its own name; a
    /// taken name becomes `name (2)`. Returns the new items.
    @discardableResult
    func upload(_ urls: [URL], rootId: RootId, parentId: ItemId) async throws -> [Item] {
        var created: [Item] = []
        for url in urls {
            let entry = Transfer(name: url.lastPathComponent,
                                 total: (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.uint64Value ?? 0)
            transfers.append(entry)
            defer { transfers.removeAll { $0.id == entry.id } }
            let result = try await client.upload(fileAt: url, to: .create(rootId: rootId, parentId: parentId, name: url.lastPathComponent),
                                                 onConflict: .keepBoth, wake: .whenAutomatic) { [weak self] done, _ in
                Task { @MainActor in
                    guard let self, let index = self.transfers.firstIndex(where: { $0.id == entry.id }) else { return }
                    self.transfers[index].done = done
                }
            }
            created.append(result.item)
        }
        revision += 1
        await FilesDomains.signalChanges(computerId)
        if root(id: rootId)?.kind == .inbox { await loadTasks() }
        return created
    }

    /// A scan goes to the Scans root, or to the given folder when that
    /// folder is inside Scans.
    func saveScan(_ pdf: URL, near rootId: RootId? = nil, parentId: ItemId? = nil) async {
        let target: (RootId, ItemId)?
        if let rootId, let parentId, root(id: rootId)?.kind == .scans {
            target = (rootId, parentId)
        } else if let scans = root(.scans) {
            target = (scans.id, scans.rootItemId)
        } else if let rootId, let parentId {
            target = (rootId, parentId)
        } else {
            target = root(.inbox).map { ($0.id, $0.rootItemId) }
        }
        guard let target else {
            notice = "\(name) has no Scans folder yet."
            return
        }
        await needingWake(.upload) { [weak self] in
            guard let self else { return }
            try await self.upload([pdf], rootId: target.0, parentId: target.1)
            self.notice = "Scan saved to \(self.root(id: target.0)?.name ?? "Scans")."
        }
    }

    func makeFolder(named name: String, rootId: RootId, parentId: ItemId) async throws {
        _ = try await client.mkdir(parentId: parentId, name: name)
        revision += 1
        await FilesDomains.signalChanges(computerId)
    }

    func trash(_ item: Item) async throws {
        _ = try await client.delete(DeleteRequest(itemId: item.id, baseRev: item.rev, permanent: false))
        revision += 1
        await FilesDomains.signalChanges(computerId)
    }

    func setKeptOffline(_ item: Item, _ pinned: Bool) async throws {
        _ = try await client.pin(itemId: item.id, pinned: pinned)
        await FilesDomains.signalChanges(computerId)
        if pinned { await FilesPinKeeper.materialize(store: FilesDrive.shared.store) }
    }

    func isKeptOffline(_ item: Item) -> Bool {
        FilesDrive.shared.store.isPinned(computerId: computerId, itemId: item.id)
    }

    // MARK: Inbox approvals

    func approve(_ task: FilesInboxTask) async {
        do {
            _ = try await client.approveInboxTask(task.taskId)
            pendingTasks.removeAll { $0.taskId == task.taskId }
            notice = "Your agent is reading \(task.name). The result lands next to it."
        } catch {
            notice = Self.message(error)
        }
    }

    func reject(_ task: FilesInboxTask) async {
        do {
            _ = try await client.rejectInboxTask(task.taskId)
            pendingTasks.removeAll { $0.taskId == task.taskId }
        } catch {
            notice = Self.message(error)
        }
    }

    func pendingTask(for item: Item) -> FilesInboxTask? {
        pendingTasks.first { $0.itemId == item.id }
    }

    // MARK: Helpers

    nonisolated static func message(_ error: Error) -> String {
        if let files = error as? FilesError { return files.localizedDescription }
        return FriendlyErrorMessage.from(error.localizedDescription)
    }

    nonisolated static func safeName(_ name: String) -> String {
        let cleaned = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        return cleaned.isEmpty ? "file" : cleaned
    }
}
