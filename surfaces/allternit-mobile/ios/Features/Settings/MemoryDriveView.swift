import CryptoKit
import SwiftUI

// Memory Drive: the git-backed memory repo the bots write to. Routes and
// shapes follow docs/MEMORY_DRIVE_CONTRACT.md, as the web's
// lib/agents/memory-drive-client.ts sends them (`/api/v1/memory/drive/*`,
// reached through cloud-api's data-plane fallback to the person's computer).

/// `GET /memory/drive/info[?drive=]`.
struct MemoryDriveInfo: Decodable, Equatable, Sendable {
    let ref: String
    let name: String
    let kind: String?
    let access: String?
    let revision: String?
    let indexDirty: Bool?
    let importedAt: String?
    let branch: String?
    let pendingWrites: Int?

    enum CodingKeys: String, CodingKey {
        case ref, name, kind, access, revision, branch
        case indexDirty = "index_dirty"
        case importedAt = "imported_at"
        case pendingWrites = "pending_writes"
    }
}

/// One drive this account can open (`GET /memory/drive/mounts`).
struct MemoryDriveMount: Decodable, Identifiable, Equatable, Sendable {
    let ref: String
    let kind: String
    let name: String
    /// `read` or `write`.
    let access: String
    let revision: String?

    var id: String { ref }
    var canWrite: Bool { access == "write" }
}

struct MemoryDriveFileEntry: Decodable, Identifiable, Equatable, Sendable {
    let path: String
    let bytes: Int
    var id: String { path }
}

private struct MemoryDriveTree: Decodable, Sendable {
    let revision: String?
    let files: [MemoryDriveFileEntry]?
}

struct MemoryDriveFile: Decodable, Sendable {
    let revision: String
    let path: String
    let content: String
}

/// The few Memory Drive calls the phone makes.
enum MemoryDriveAPI {
    static let personal = "personal"

    /// Only the personal drive is implied; anything else is sent.
    static func driveQuery(_ drive: String, prefix: String = "?") -> String {
        drive == personal ? "" : "\(prefix)drive=\(settingsQueryValue(drive))"
    }

    private struct MountsBody: Decodable { let mounts: [MemoryDriveMount]? }

    static func mounts() async throws -> [MemoryDriveMount] {
        let body: MountsBody = try await APIClient.shared.get(path: "memory/drive/mounts")
        let list = body.mounts ?? []
        if list.contains(where: { $0.ref == personal }) { return list }
        return [MemoryDriveMount(ref: personal, kind: "personal", name: "Personal", access: "write", revision: nil)] + list
    }

    static func info(_ drive: String) async throws -> MemoryDriveInfo {
        try await APIClient.shared.get(path: "memory/drive/info\(driveQuery(drive))")
    }

    static func tree(_ drive: String) async throws -> [MemoryDriveFileEntry] {
        let tree: MemoryDriveTree = try await APIClient.shared.get(path: "memory/drive/tree\(driveQuery(drive))")
        return (tree.files ?? []).sorted { $0.path < $1.path }
    }

    static func file(_ path: String, drive: String) async throws -> MemoryDriveFile {
        try await APIClient.shared.get(path: "memory/drive/file?path=\(settingsQueryValue(path))\(driveQuery(drive, prefix: "&"))")
    }

    private struct SetFileOp: Encodable { let path: String; let content: String }
    /// `{ "SetFile": { path, content } }` (externally tagged, as the contract sends it).
    private struct Operation: Encodable {
        let setFile: SetFileOp
        enum CodingKeys: String, CodingKey { case setFile = "SetFile" }
    }
    private struct WriteBody: Encodable {
        let drive: String?
        let expected_revision: String?
        let message: String
        let operations: [Operation]
    }
    private struct PurgeBody: Encodable {
        let drive: String?
        let expected_revision: String
        let entry_ids: [String]
        let confirm = "delete forever"
    }
    private struct PurgeResult: Decodable { let purged: Int? }

    private struct CommittedBody: Decodable { let committed: Bool? }

    /// `POST /write` with one `SetFile`, guarded by the file's revision.
    /// Returns true when it saved but search is still catching up (503 with
    /// `committed: true`).
    @discardableResult
    static func save(_ file: MemoryDriveFile, content: String, drive: String) async throws -> Bool {
        let body = WriteBody(
            drive: drive == personal ? nil : drive,
            expected_revision: file.revision,
            message: "Edit \(file.path) in Settings",
            operations: [Operation(setFile: SetFileOp(path: file.path, content: content))]
        )
        let client = APIClient.shared
        var request = try await client.authorizedRequest(path: "memory/drive/write", method: "POST")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await client.send(request)
        if (response as? HTTPURLResponse)?.statusCode == 503,
           (try? JSONDecoder().decode(CommittedBody.self, from: data))?.committed == true {
            return true
        }
        try client.validate(response, data: data)
        return false
    }

    /// `POST /purge`: removes entries from the drive and its history.
    static func purge(entryIds: [String], revision: String, drive: String) async throws -> Int {
        let body = PurgeBody(drive: drive == personal ? nil : drive, expected_revision: revision, entry_ids: entryIds)
        let result: PurgeResult = try await APIClient.shared.post(path: "memory/drive/purge", body: body)
        return result.purged ?? entryIds.count
    }

    static func reindex() async throws {
        try await APIClient.shared.post(path: "memory/drive/reindex")
    }

    /// Plain-language text for a Memory Drive failure (memory-drive-client.ts FALLBACK_MESSAGES).
    static func message(_ error: Error) -> String {
        if case let APIError.httpError(status, message) = error {
            switch status {
            case 401: return "Sign in to see your Memory Drive."
            case 403: return message ?? "You don't have access to this drive."
            case 404: return message ?? "That drive or file wasn't found."
            case 409: return "This file changed since you opened it."
            case 400, 413, 422: return message ?? "That request wasn't accepted."
            case 503:
                return message ?? "Your Memory Drive lives on your computer, and none is online. Open Allternit Desktop or start your cloud computer, then try again."
            default: break
            }
        }
        return SettingsLoadFailure.from(error).message
    }

    static func isConflict(_ error: Error) -> Bool {
        if case let APIError.httpError(status, _) = error { return status == 409 }
        return false
    }

    /// Folder Allternit maintains; read-only (memory-drive-client.ts `managedFolderNote`).
    static func managedNote(_ path: String) -> String? {
        path.hasPrefix("twin/") ? "Managed by Allternit. Change twin memories in Settings → Your twin." : nil
    }

    /// Memory entry lines `- text [source: …; added: …; id: …]` (MemoryDrivePanel.tsx `parseEntryLines`).
    struct Entry: Identifiable, Sendable {
        let index: Int
        let text: String
        let source: String
        let added: String
        let entryId: String?
        var id: Int { index }

        /// The entry's id, or `entry-<sha256(text\0source\0added)>` when the line has none.
        var resolvedId: String {
            if let entryId { return entryId }
            let digest = SHA256.hash(data: Data("\(text)\u{0}\(source)\u{0}\(added)".utf8))
            return "entry-" + digest.map { String(format: "%02x", $0) }.joined()
        }
    }

    static func entries(in content: String) -> [Entry] {
        var out: [Entry] = []
        for line in content.components(separatedBy: "\n") {
            guard line.hasPrefix("- "), !line.hasPrefix("- [["), line.hasSuffix("]"),
                  let at = line.range(of: " [", options: .backwards),
                  line.distance(from: line.startIndex, to: at.lowerBound) >= 2 else { continue }
            let text = String(line[line.index(line.startIndex, offsetBy: 2)..<at.lowerBound])
            let metaText = line[at.upperBound..<line.index(before: line.endIndex)]
            var meta: [String: String] = [:]
            for part in metaText.components(separatedBy: "; ") {
                guard let sep = part.range(of: ": "), sep.lowerBound > part.startIndex else { continue }
                meta[String(part[..<sep.lowerBound])] = String(part[sep.upperBound...])
            }
            guard let source = meta["source"], let added = meta["added"] else { continue }
            out.append(Entry(index: out.count, text: text, source: source, added: added, entryId: meta["id"]))
        }
        return out
    }
}

/// Settings → Assistant → Memory Drive (the web's MemoryDrivePanel): the
/// files Allternit remembers from. Pick a drive, read and edit files, and
/// delete memories forever.
///
/// Each file has its history (diffs, compare with now, restore), and the
/// drive's copies (a folder or your own git repository with a token) are
/// under Keep a copy.
///
/// Phone subset: git clone tokens, Dreaming, imports (ChatGPT/Claude/agents)
/// and the ledger stay on Desktop / the web; they need a wide two-pane view
/// or a desktop file picker.
struct MemoryDriveView: View {
    @EnvironmentObject private var authManager: AuthManager
    @State private var mounts: [MemoryDriveMount] = []
    @State private var drive = MemoryDriveAPI.personal
    @State private var info: MemoryDriveInfo? = nil
    @State private var files: [MemoryDriveFileEntry] = []
    @State private var isLoading = false
    @State private var loaded = false
    @State private var failure: SettingsLoadFailure? = nil
    @State private var failureText: String? = nil
    @State private var search = ""
    @State private var notice: String? = nil
    @State private var reindexing = false

    private var mount: MemoryDriveMount? { mounts.first { $0.ref == drive } }
    private var canWrite: Bool { (mount?.canWrite ?? (drive == MemoryDriveAPI.personal)) && info?.access != "read" }

    private var filtered: [MemoryDriveFileEntry] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return files }
        return files.filter { $0.path.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        List {
            if !authManager.canCallAccountAPIs {
                Section { SettingsSignedOutView(message: "Sign in to see your Memory Drive.") }
            } else {
                driveSection
                filesSection
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Memory Drive")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Find a file")
        .task { if !loaded { await load(mountsToo: true) } }
        .refreshable { await load(mountsToo: true) }
        .onChange(of: drive) { _, _ in Task { await load(mountsToo: false) } }
        .onChange(of: authManager.isSignedIn) { _, _ in
            loaded = false
            Task { await load(mountsToo: true) }
        }
    }

    @ViewBuilder
    private var driveSection: some View {
        Section {
            Text("The files Allternit remembers from. You can read them and edit them here.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
            if mounts.count > 1 {
                Picker(selection: $drive) {
                    ForEach(mounts) { m in
                        Text(m.canWrite ? m.name : "\(m.name) (read only)").tag(m.ref)
                    }
                } label: {
                    SettingsRowLabel(title: "Drive", systemImage: "externaldrive")
                }
            }
            NavigationLink { MemoryDriveCopiesView() } label: {
                SettingsRowLabel(title: "Keep a copy", systemImage: "arrow.triangle.2.circlepath")
            }
            if let info {
                if mounts.count <= 1 { row("Drive", info.name) }
                if let revision = info.revision { row("Revision", String(revision.prefix(8))) }
                if let pending = info.pendingWrites, pending > 0 { row("Waiting to save", "\(pending)") }
                if info.indexDirty == true {
                    HStack {
                        Text("Search is catching up.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        Spacer()
                        Button(reindexing ? "Retrying…" : "Retry now") { Task { await reindex() } }
                            .font(.caption.weight(.semibold))
                            .disabled(reindexing)
                    }
                }
                if !canWrite {
                    Text("You can read this drive but not change it.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        } footer: {
            if let notice { Text(notice) }
        }
    }

    @ViewBuilder
    private var filesSection: some View {
        Section {
            if isLoading && files.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let failure, files.isEmpty {
                SettingsFailureView(
                    failure: displayFailure(failure),
                    subject: "Memory Drive",
                    retry: { Task { await load(mountsToo: true) } }
                )
            } else if files.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "brain",
                    title: "Your Memory Drive is empty",
                    message: "Things you ask Allternit to remember are saved here as files."
                )
            } else {
                ForEach(filtered) { file in
                    NavigationLink {
                        MemoryDriveFileView(path: file.path, drive: drive, canWrite: canWrite, onSaved: { message in
                            notice = message
                            Task { await load(mountsToo: false) }
                        })
                    } label: {
                        HStack {
                            Image(systemName: "doc.text")
                                .foregroundColor(Color("TextSecondary"))
                            Text(file.path)
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Text(ByteCountFormatter.string(fromByteCount: Int64(file.bytes), countStyle: .file))
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
            }
        } header: {
            Text("Files")
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.subheadline).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value).font(.subheadline).foregroundColor(Color("TextSecondary"))
        }
    }

    /// Plain errors carry the drive's own wording.
    private func displayFailure(_ failure: SettingsLoadFailure) -> SettingsLoadFailure {
        if case .error = failure, let failureText { return .error(failureText) }
        return failure
    }

    @MainActor
    private func load(mountsToo: Bool) async {
        guard authManager.canCallAccountAPIs else { return }
        isLoading = true
        failure = nil
        failureText = nil
        defer {
            isLoading = false
            loaded = true
        }
        if mountsToo {
            // Older computers have no /mounts: the personal drive alone.
            mounts = (try? await MemoryDriveAPI.mounts()) ?? []
            if !mounts.isEmpty, !mounts.contains(where: { $0.ref == drive }) { drive = MemoryDriveAPI.personal }
        }
        let current = drive
        info = try? await MemoryDriveAPI.info(current)
        do {
            let list = try await MemoryDriveAPI.tree(current)
            guard current == drive else { return }
            files = list
        } catch {
            guard current == drive else { return }
            files = []
            failure = SettingsLoadFailure.from(error)
            failureText = MemoryDriveAPI.message(error)
        }
    }

    @MainActor
    private func reindex() async {
        reindexing = true
        defer { reindexing = false }
        do {
            try await MemoryDriveAPI.reindex()
            notice = "Memory is up to date."
            info = try? await MemoryDriveAPI.info(drive)
        } catch {
            notice = MemoryDriveAPI.message(error)
        }
    }
}

/// One Memory Drive file: read it, edit it (saved as one commit, guarded
/// against someone else's newer edit), or delete memories in it forever.
struct MemoryDriveFileView: View {
    let path: String
    var drive: String = MemoryDriveAPI.personal
    var canWrite: Bool = false
    var onSaved: (String) -> Void = { _ in }

    @State private var file: MemoryDriveFile? = nil
    @State private var loadError: String? = nil
    @State private var editing = false
    @State private var draft = ""
    @State private var saving = false
    @State private var conflict = false
    @State private var saveError: String? = nil
    @State private var isPurging = false
    @State private var confirmDiscard = false
    @State private var isShowingHistory = false

    private var managedNote: String? { MemoryDriveAPI.managedNote(path) }
    private var canEdit: Bool { canWrite && managedNote == nil }

    var body: some View {
        Group {
            if let file {
                if editing {
                    editor(file)
                } else {
                    reader(file)
                }
            } else if let loadError {
                FriendlyStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't open this file",
                    message: loadError,
                    actionTitle: "Try again",
                    action: { Task { await load() } }
                )
            } else {
                ProgressView()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color("BgPrimary"))
        .navigationTitle((path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(editing)
        .toolbar { toolbar }
        .task { if file == nil { await load() } }
        .sheet(isPresented: $isPurging) {
            if let file {
                MemoryDrivePurgeView(file: file, drive: drive) { message in
                    onSaved(message)
                    Task { await load() }
                }
            }
        }
        .navigationDestination(isPresented: $isShowingHistory) {
            MemoryDriveHistoryView(path: path, drive: drive, canWrite: canEdit, onRestored: { message in
                onSaved(message)
                Task { await load() }
            })
        }
        .confirmationDialog("Discard your edits?", isPresented: $confirmDiscard, titleVisibility: .visible) {
            Button("Discard", role: .destructive) { stopEditing() }
            Button("Keep editing", role: .cancel) {}
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        if editing {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") {
                    if draft != file?.content { confirmDiscard = true } else { stopEditing() }
                }
                .disabled(saving)
            }
            ToolbarItem(placement: .confirmationAction) {
                if saving { ProgressView() } else {
                    Button("Save") { Task { await save() } }
                        .disabled(conflict || draft == file?.content)
                }
            }
        } else {
            if file != nil {
                ToolbarItem(placement: .primaryAction) {
                    Button { isShowingHistory = true } label: { Label("History", systemImage: "clock.arrow.circlepath") }
                }
            }
            if canEdit, let file {
                ToolbarItem(placement: .primaryAction) {
                    Button("Edit") {
                        draft = file.content
                        conflict = false
                        saveError = nil
                        editing = true
                    }
                }
            }
        }
    }

    private func reader(_ file: MemoryDriveFile) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let managedNote {
                    Text(managedNote)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Text(file.content)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if canEdit && !MemoryDriveAPI.entries(in: file.content).isEmpty {
                    Button(role: .destructive) { isPurging = true } label: {
                        Label("Delete memories forever…", systemImage: "trash")
                            .font(.subheadline)
                    }
                    .padding(.top, 8)
                }
            }
            .padding(16)
        }
    }

    private func editor(_ file: MemoryDriveFile) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if conflict {
                HStack(alignment: .top, spacing: 8) {
                    Text("This file changed since you opened it. Reload to see the latest version; your edits here will be replaced.")
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer(minLength: 0)
                    Button("Reload") {
                        stopEditing()
                        Task { await load() }
                    }
                    .font(.footnote.weight(.semibold))
                }
                .padding(10)
                .background(RoundedRectangle(cornerRadius: Theme.radiusSM).fill(Color("BgSecondary")))
            }
            if let saveError {
                Text(saveError).font(.footnote).foregroundColor(Theme.statusError)
            }
            TextEditor(text: $draft)
                .font(.system(.footnote, design: .monospaced))
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .scrollContentBackground(.hidden)
                .padding(8)
                .background(RoundedRectangle(cornerRadius: Theme.radiusSM).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: Theme.radiusSM).stroke(Color("BorderSubtle"), lineWidth: 1))
                .accessibilityLabel("Edit \(file.path)")
        }
        .padding(16)
    }

    private func stopEditing() {
        editing = false
        conflict = false
        saveError = nil
    }

    @MainActor
    private func load() async {
        loadError = nil
        do {
            file = try await MemoryDriveAPI.file(path, drive: drive)
        } catch {
            loadError = MemoryDriveAPI.message(error)
        }
    }

    @MainActor
    private func save() async {
        guard let file else { return }
        saving = true
        saveError = nil
        defer { saving = false }
        do {
            let indexPending = try await MemoryDriveAPI.save(file, content: draft, drive: drive)
            stopEditing()
            onSaved(indexPending ? "Saved. Search will catch up shortly." : "Saved \(file.path).")
            await load()
        } catch {
            if MemoryDriveAPI.isConflict(error) {
                conflict = true
            } else {
                saveError = MemoryDriveAPI.message(error)
            }
        }
    }
}

/// Pick memories to remove from the drive and every earlier version of it
/// (the web's PurgePane). Typing "delete forever" confirms.
struct MemoryDrivePurgeView: View {
    let file: MemoryDriveFile
    let drive: String
    let onDone: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var chosen: Set<Int> = []
    @State private var typed = ""
    @State private var busy = false
    @State private var error: String? = nil

    private var entries: [MemoryDriveAPI.Entry] { MemoryDriveAPI.entries(in: file.content) }
    private var ready: Bool { !chosen.isEmpty && typed.trimmingCharacters(in: .whitespaces).lowercased() == "delete forever" }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Choose memories to remove from this drive and from every earlier version of it. This can't be undone, and they can't be added back. Copies someone already cloned with a token are not affected.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                Section("Memories") {
                    ForEach(entries) { entry in
                        Button {
                            if chosen.contains(entry.index) { chosen.remove(entry.index) } else { chosen.insert(entry.index) }
                        } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: chosen.contains(entry.index) ? "checkmark.circle.fill" : "circle")
                                    .foregroundColor(chosen.contains(entry.index) ? Theme.statusError : Color("TextSecondary"))
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(entry.text).font(.subheadline).foregroundColor(Color("TextPrimary"))
                                    Text(entry.added).font(.caption).foregroundColor(Color("TextSecondary"))
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(chosen.contains(entry.index) ? .isSelected : [])
                    }
                }
                Section {
                    TextField("delete forever", text: $typed)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Type delete forever to confirm")
                } footer: {
                    if let error { Text(error).foregroundColor(Theme.statusError) }
                }
                Section {
                    Button(role: .destructive) { Task { await run() } } label: {
                        HStack {
                            if busy { ProgressView() }
                            Text(busy ? "Deleting…" : chosen.count == 1 ? "Delete 1 memory forever" : "Delete \(chosen.count) memories forever")
                        }
                    }
                    .disabled(!ready || busy)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Delete forever")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) }
            }
        }
        .interactiveDismissDisabled(busy)
    }

    @MainActor
    private func run() async {
        busy = true
        error = nil
        defer { busy = false }
        let ids = entries.filter { chosen.contains($0.index) }.map(\.resolvedId)
        do {
            let purged = try await MemoryDriveAPI.purge(entryIds: ids, revision: file.revision, drive: drive)
            onDone(purged == 1 ? "Deleted 1 memory from the drive and its history." : "Deleted \(purged) memories from the drive and its history.")
            dismiss()
        } catch {
            self.error = MemoryDriveAPI.isConflict(error)
                ? "This file changed since you opened it. Go back and try again."
                : MemoryDriveAPI.message(error)
        }
    }
}
