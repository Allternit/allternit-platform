import SwiftUI

// Memory Drive history, diffs, restore and copies (the web's
// MemoryDrivePanel HistoryPane / DiffView / CopiesSection over
// lib/agents/memory-drive-client.ts `history`, `diff`, `commitDiff`, `file`
// with `revision`, `write`, `copies`, `saveCopies`, `runCopies`).

/// `GET /memory/drive/history` commit.
struct MemoryDriveCommit: Decodable, Identifiable, Equatable, Sendable {
    let revision: String
    let parents: [String]
    let author: String
    let timestamp: String
    let message: String

    var id: String { revision }
    var shortRevision: String { String(revision.prefix(8)) }
}

/// `GET /memory/drive/copies`. The token itself is never returned.
struct MemoryDriveCopies: Decodable, Equatable, Sendable {
    struct Status: Decodable, Equatable, Sendable {
        let lastAt: String?
        let lastError: String?
        let conflicts: [String]?
        let failed: [String]?

        enum CodingKeys: String, CodingKey {
            case lastAt = "last_at"
            case lastError = "last_error"
            case conflicts, failed
        }
    }
    struct Folder: Decodable, Equatable, Sendable {
        let path: String
        let status: Status
    }
    struct Git: Decodable, Equatable, Sendable {
        let url: String
        let hasToken: Bool
        let status: Status

        enum CodingKeys: String, CodingKey {
            case url, status
            case hasToken = "has_token"
        }
    }
    let folder: Folder?
    let git: Git?
}

extension MemoryDriveAPI {
    /// `git hash-object -t tree /dev/null`: the "from" for a root commit's diff.
    static let emptyTreeRevision = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"

    private struct HistoryBody: Decodable { let commits: [MemoryDriveCommit]? }
    private struct DiffBody: Decodable { let diff: String? }

    static func history(_ path: String, drive: String, limit: Int = 25) async throws -> [MemoryDriveCommit] {
        let body: HistoryBody = try await APIClient.shared.get(
            path: "memory/drive/history?path=\(settingsQueryValue(path))&limit=\(min(max(limit, 1), 100))\(driveQuery(drive, prefix: "&"))"
        )
        return body.commits ?? []
    }

    static func diff(from: String, to: String, path: String, drive: String) async throws -> String {
        let body: DiffBody = try await APIClient.shared.get(
            path: "memory/drive/diff?from=\(settingsQueryValue(from))&to=\(settingsQueryValue(to))&path=\(settingsQueryValue(path))\(driveQuery(drive, prefix: "&"))"
        )
        return body.diff ?? ""
    }

    /// What one commit changed in `path` (against its first parent).
    static func commitDiff(_ commit: MemoryDriveCommit, path: String, drive: String) async throws -> String {
        try await diff(from: commit.parents.first ?? emptyTreeRevision, to: commit.revision, path: path, drive: drive)
    }

    static func file(_ path: String, drive: String, revision: String) async throws -> MemoryDriveFile {
        try await APIClient.shared.get(
            path: "memory/drive/file?path=\(settingsQueryValue(path))&revision=\(settingsQueryValue(revision))\(driveQuery(drive, prefix: "&"))"
        )
    }

    /// Restore = one new `SetFile` commit with the old text, guarded by the
    /// current revision (history stays intact). Returns true when it saved
    /// but search is still catching up.
    static func restore(_ path: String, to commit: MemoryDriveCommit, drive: String) async throws -> Bool {
        let old = try await file(path, drive: drive, revision: commit.revision)
        let current = try await file(path, drive: drive)
        let body = RestoreWriteBody(
            drive: drive == personal ? nil : drive,
            expected_revision: current.revision,
            message: "Restore \(path) to \(commit.shortRevision) in Settings",
            operations: [RestoreOperation(setFile: RestoreSetFile(path: path, content: old.content))]
        )
        let client = APIClient.shared
        var request = try await client.authorizedRequest(path: "memory/drive/write", method: "POST")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await client.send(request)
        if (response as? HTTPURLResponse)?.statusCode == 503,
           (try? JSONDecoder().decode(RestoreCommitted.self, from: data))?.committed == true {
            return true
        }
        try client.validate(response, data: data)
        return false
    }

    // MARK: Copies

    static func copies() async throws -> MemoryDriveCopies {
        try await APIClient.shared.get(path: "memory/drive/copies")
    }

    /// `""` stops a copy; a nil field leaves it as it is. Copies once right away.
    static func saveCopies(folderPath: String? = nil, gitURL: String? = nil, gitToken: String? = nil) async throws -> MemoryDriveCopies {
        var request = try await APIClient.shared.authorizedRequest(path: "memory/drive/copies", method: "PUT")
        request.httpBody = try JSONEncoder().encode(CopiesChange(folder_path: folderPath, git_url: gitURL, git_token: gitToken))
        return try await decode(request)
    }

    static func runCopies() async throws -> MemoryDriveCopies {
        var request = try await APIClient.shared.authorizedRequest(path: "memory/drive/copies/run", method: "POST")
        request.httpBody = Data("{}".utf8)
        return try await decode(request)
    }

    private static func decode<T: Decodable>(_ request: URLRequest) async throws -> T {
        let (data, response) = try await APIClient.shared.send(request)
        try APIClient.shared.validate(response, data: data)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    static func isNotFound(_ error: Error) -> Bool {
        if case let APIError.httpError(status, _) = error { return status == 404 }
        return false
    }

    static func dateText(_ iso: String) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = formatter.date(from: iso) ?? ISO8601DateFormatter().date(from: iso) else { return iso }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

private struct RestoreSetFile: Encodable { let path: String; let content: String }
private struct RestoreOperation: Encodable {
    let setFile: RestoreSetFile
    enum CodingKeys: String, CodingKey { case setFile = "SetFile" }
}
private struct RestoreWriteBody: Encodable {
    let drive: String?
    let expected_revision: String?
    let message: String
    let operations: [RestoreOperation]
}
private struct RestoreCommitted: Decodable { let committed: Bool? }

/// `nil` fields are left out, so the server leaves that copy as it is.
private struct CopiesChange: Encodable {
    let folder_path: String?
    let git_url: String?
    let git_token: String?

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(folder_path, forKey: .folder_path)
        try c.encodeIfPresent(git_url, forKey: .git_url)
        try c.encodeIfPresent(git_token, forKey: .git_token)
    }

    enum CodingKeys: String, CodingKey { case folder_path, git_url, git_token }
}

// MARK: - History list

/// Every saved version of one file, newest first. Tap one to see what
/// changed, compare it with the current file, or restore it.
struct MemoryDriveHistoryView: View {
    let path: String
    let drive: String
    let canWrite: Bool
    /// Called after a restore with a message for the file view.
    let onRestored: (String) -> Void

    @State private var commits: [MemoryDriveCommit]? = nil
    @State private var failure: SettingsLoadFailure? = nil
    @State private var failureText: String? = nil

    var body: some View {
        List {
            if let commits {
                if commits.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "clock.arrow.circlepath",
                        title: "No history yet",
                        message: "No changes recorded for this file yet."
                    )
                } else {
                    Section {
                        ForEach(Array(commits.enumerated()), id: \.element.id) { index, commit in
                            NavigationLink {
                                MemoryDriveVersionView(
                                    path: path,
                                    drive: drive,
                                    commit: commit,
                                    isCurrent: index == 0,
                                    canWrite: canWrite,
                                    onRestored: { message in
                                        onRestored(message)
                                        Task { await load() }
                                    }
                                )
                            } label: {
                                VStack(alignment: .leading, spacing: 3) {
                                    HStack(spacing: 6) {
                                        Text(commit.message.isEmpty ? "No message" : commit.message)
                                            .font(.subheadline)
                                            .foregroundColor(Color("TextPrimary"))
                                            .lineLimit(2)
                                        if index == 0 {
                                            Text("Current")
                                                .font(.caption2.weight(.semibold))
                                                .foregroundColor(Color("TextSecondary"))
                                                .padding(.horizontal, 6)
                                                .padding(.vertical, 2)
                                                .background(Capsule().fill(Color("BgSecondary")))
                                        }
                                    }
                                    Text("\(commit.author) · \(MemoryDriveAPI.dateText(commit.timestamp)) · \(commit.shortRevision)")
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                            }
                        }
                    } footer: {
                        if commits.count >= 25 { Text("Showing the 25 most recent versions.") }
                    }
                }
            } else if let failure {
                SettingsFailureView(failure: display(failure), subject: "this file's history", retry: { Task { await load() } })
            } else {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading history…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("History")
        .navigationBarTitleDisplayMode(.inline)
        .task { if commits == nil { await load() } }
        .refreshable { await load() }
    }

    private func display(_ failure: SettingsLoadFailure) -> SettingsLoadFailure {
        if case .error = failure, let failureText { return .error(failureText) }
        return failure
    }

    @MainActor
    private func load() async {
        failure = nil
        failureText = nil
        do {
            commits = try await MemoryDriveAPI.history(path, drive: drive)
        } catch {
            if commits == nil {
                failure = SettingsLoadFailure.from(error)
                failureText = MemoryDriveAPI.message(error)
            }
        }
    }
}

// MARK: - One version

struct MemoryDriveVersionView: View {
    let path: String
    let drive: String
    let commit: MemoryDriveCommit
    let isCurrent: Bool
    let canWrite: Bool
    let onRestored: (String) -> Void

    enum Pane: String, CaseIterable, Identifiable {
        case changes = "What changed"
        case compare = "Compare with now"
        case content = "This version"
        var id: String { rawValue }
    }

    @Environment(\.dismiss) private var dismiss
    @State private var pane: Pane = .changes
    @State private var text: [Pane: String] = [:]
    @State private var errors: [Pane: String] = [:]
    @State private var restoring = false
    @State private var confirmRestore = false
    @State private var restoreError: String? = nil

    private var panes: [Pane] { isCurrent ? [.changes, .content] : Pane.allCases }
    private var canRestore: Bool { canWrite && !isCurrent && MemoryDriveAPI.managedNote(path) == nil }

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text(commit.message.isEmpty ? "No message" : commit.message)
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(commit.author) · \(MemoryDriveAPI.dateText(commit.timestamp)) · \(commit.shortRevision)")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                Picker("View", selection: $pane) {
                    ForEach(panes) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(.top, 6)
            }
            .padding(16)

            ScrollView {
                Group {
                    if let error = errors[pane] {
                        FriendlyInlineStateView(
                            style: .error,
                            icon: "exclamationmark.triangle",
                            title: "Couldn't load this",
                            message: error,
                            actionTitle: "Try again",
                            action: { Task { await load(pane, force: true) } }
                        )
                    } else if let value = text[pane] {
                        if pane == .content {
                            Text(value.isEmpty ? "This version of the file is empty." : value)
                                .font(.system(.footnote, design: .monospaced))
                                .foregroundColor(Color("TextPrimary"))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } else {
                            MemoryDriveDiffView(
                                text: value,
                                emptyText: pane == .compare ? "This version matches the current file." : "No changes to this file in that version."
                            )
                        }
                    } else {
                        ProgressView().frame(maxWidth: .infinity).padding(.top, 24)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 16)
            }

            if canRestore {
                VStack(spacing: 6) {
                    if let restoreError {
                        Text(restoreError).font(.footnote).foregroundColor(Theme.statusError)
                    }
                    Button { confirmRestore = true } label: {
                        HStack {
                            if restoring { ProgressView() }
                            Text(restoring ? "Restoring…" : "Restore this version")
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(restoring)
                }
                .padding(16)
                .background(Color("BgPrimary"))
            }
        }
        .background(Color("BgPrimary"))
        .navigationTitle(commit.shortRevision)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: pane) { await load(pane, force: false) }
        .confirmationDialog("Restore this version?", isPresented: $confirmRestore, titleVisibility: .visible) {
            Button("Restore", role: .destructive) { Task { await restore() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("\(path) goes back to how it was in \(commit.shortRevision). This saves a new version; the current text stays in history.")
        }
    }

    @MainActor
    private func load(_ which: Pane, force: Bool) async {
        if !force, text[which] != nil { return }
        errors[which] = nil
        if force { text[which] = nil }
        do {
            switch which {
            case .changes:
                text[which] = try await MemoryDriveAPI.commitDiff(commit, path: path, drive: drive)
            case .compare:
                let current = try await MemoryDriveAPI.file(path, drive: drive)
                text[which] = try await MemoryDriveAPI.diff(from: commit.revision, to: current.revision, path: path, drive: drive)
            case .content:
                text[which] = try await MemoryDriveAPI.file(path, drive: drive, revision: commit.revision).content
            }
        } catch {
            errors[which] = MemoryDriveAPI.isNotFound(error)
                ? "This file didn't exist in that version."
                : MemoryDriveAPI.message(error)
        }
    }

    @MainActor
    private func restore() async {
        restoring = true
        restoreError = nil
        defer { restoring = false }
        do {
            let pending = try await MemoryDriveAPI.restore(path, to: commit, drive: drive)
            onRestored(pending
                ? "Restored. Search will catch up shortly."
                : "Restored \(path) to \(commit.shortRevision).")
            dismiss()
        } catch {
            restoreError = MemoryDriveAPI.isConflict(error)
                ? "This file changed while you were looking. Try again."
                : MemoryDriveAPI.isNotFound(error) ? "This file didn't exist in that version." : MemoryDriveAPI.message(error)
        }
    }
}

/// Unified diff, added lines green and removed lines red (the web's DiffView).
struct MemoryDriveDiffView: View {
    let text: String
    var emptyText = "No changes."

    var body: some View {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            Text(emptyText)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(text.components(separatedBy: "\n").enumerated()), id: \.offset) { _, line in
                    let added = line.hasPrefix("+") && !line.hasPrefix("+++")
                    let removed = line.hasPrefix("-") && !line.hasPrefix("---")
                    Text(line.isEmpty ? " " : line)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(added ? Theme.statusSuccess : removed ? Theme.statusError : Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 8)
                        .background(added ? Theme.statusSuccess.opacity(0.10) : removed ? Theme.statusError.opacity(0.10) : Color.clear)
                        .accessibilityLabel(added ? "Added: \(line.dropFirst())" : removed ? "Removed: \(line.dropFirst())" : line)
                }
            }
            .padding(.vertical, 8)
            .textSelection(.enabled)
            .background(RoundedRectangle(cornerRadius: Theme.radiusSM).fill(Color("BgSecondary")))
        }
    }
}

// MARK: - Copies

/// Keep a copy of the drive and ledger in a folder on the computer or in
/// your own git repository (the web's CopiesSection).
struct MemoryDriveCopiesView: View {
    @EnvironmentObject private var authManager: AuthManager
    @State private var copies: MemoryDriveCopies? = nil
    @State private var unsupported = false
    @State private var failure: SettingsLoadFailure? = nil
    @State private var failureText: String? = nil
    @State private var folder = ""
    @State private var gitURL = ""
    @State private var token = ""
    @State private var busy: String? = nil
    @State private var error: String? = nil
    @State private var notice: String? = nil
    @State private var confirmStop: String? = nil

    var body: some View {
        Form {
            Section {
                Text("Allternit keeps your memories and ledger on the computer that holds your drive. You can also keep a copy in a folder or in your own git repository. Copies update every hour, and changes you make in a copy come back into Allternit.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            if !authManager.canCallAccountAPIs {
                Section { SettingsSignedOutView(message: "Sign in to manage copies of your Memory Drive.") }
            } else if unsupported {
                Section {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "arrow.triangle.2.circlepath",
                        title: "Update your computer",
                        message: "The computer holding your drive doesn't support copies yet. Update Allternit Desktop, then try again."
                    )
                }
            } else if let copies {
                folderSection(copies)
                gitSection(copies)
                if copies.folder != nil || copies.git != nil {
                    Section {
                        Button(busy == "run" ? "Copying…" : "Copy now") { Task { await save("run") { try await MemoryDriveAPI.runCopies() } } }
                            .disabled(busy != nil)
                    } footer: {
                        if let error { Text(error).foregroundColor(Theme.statusError) }
                        else if let notice { Text(notice) }
                    }
                } else if let error {
                    Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
                }
            } else if let failure {
                Section {
                    SettingsFailureView(failure: display(failure), subject: "your copies", retry: { Task { await load() } })
                }
            } else {
                Section { ProgressView().frame(maxWidth: .infinity) }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Keep a copy")
        .navigationBarTitleDisplayMode(.inline)
        .task { if copies == nil { await load() } }
        .refreshable { await load() }
        .confirmationDialog(
            confirmStop == "git" ? "Stop copying to this repository?" : "Stop copying to this folder?",
            isPresented: Binding(get: { confirmStop != nil }, set: { if !$0 { confirmStop = nil } }),
            titleVisibility: .visible
        ) {
            Button("Stop copying", role: .destructive) {
                let which = confirmStop
                Task {
                    if which == "git" {
                        await save("git") { try await MemoryDriveAPI.saveCopies(gitURL: "") }
                    } else {
                        await save("folder") { try await MemoryDriveAPI.saveCopies(folderPath: "") }
                    }
                }
            }
            Button("Keep copying", role: .cancel) {}
        } message: {
            Text(confirmStop == "git" ? "The saved token is removed. What's already in the repository stays there." : "What's already in the folder stays there.")
        }
    }

    @ViewBuilder
    private func folderSection(_ copies: MemoryDriveCopies) -> some View {
        Section {
            TextField("/Users/you/Dropbox", text: $folder)
                .font(.system(.body, design: .monospaced))
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            Button(busy == "folder" ? "Copying…" : "Copy here") {
                let value = folder.trimmingCharacters(in: .whitespaces)
                Task { await save("folder") { try await MemoryDriveAPI.saveCopies(folderPath: value) } }
            }
            .disabled(busy != nil || folder.trimmingCharacters(in: .whitespaces).isEmpty || folder == copies.folder?.path)
            if copies.folder != nil {
                Button("Stop copying here", role: .destructive) { confirmStop = "folder" }
                    .disabled(busy != nil)
            }
        } header: {
            Text("A folder on that computer")
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text("Allternit writes an Allternit folder inside it. An iCloud Drive or Dropbox folder keeps the copy on your other devices too.")
                if let status = copies.folder?.status { statusLine(status, place: "in the folder") }
            }
        }
    }

    @ViewBuilder
    private func gitSection(_ copies: MemoryDriveCopies) -> some View {
        Section {
            TextField("https://github.com/you/allternit-memory.git", text: $gitURL)
                .font(.system(.body, design: .monospaced))
                .keyboardType(.URL)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            SecureField(copies.git?.hasToken == true ? "Token saved. Paste a new one to replace it" : "Access token", text: $token)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            Button(busy == "git" ? "Pushing…" : "Copy to this repository") {
                let url = gitURL.trimmingCharacters(in: .whitespaces)
                let newToken = token.isEmpty ? nil : token
                Task { await save("git") { try await MemoryDriveAPI.saveCopies(gitURL: url, gitToken: newToken) } }
            }
            .disabled(busy != nil || gitURL.trimmingCharacters(in: .whitespaces).isEmpty || (gitURL == copies.git?.url && token.isEmpty))
            if copies.git != nil {
                Button("Stop copying there", role: .destructive) { confirmStop = "git" }
                    .disabled(busy != nil)
            }
        } header: {
            Text("Your own git repository")
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text("Use an empty private repository. Your memories go to the branch allternit-memory and your ledger to allternit-ledger. A token that can only push to that repository is safest; it's stored sealed and never shown again.")
                if let status = copies.git?.status { statusLine(status, place: "in the repository") }
            }
        }
    }

    @ViewBuilder
    private func statusLine(_ status: MemoryDriveCopies.Status, place: String) -> some View {
        if let lastError = status.lastError, !lastError.isEmpty {
            Text("Last copy failed: \(lastError)").foregroundColor(Theme.statusError)
        } else if let lastAt = status.lastAt {
            Text("Last copied \(MemoryDriveAPI.dateText(lastAt)).")
        } else {
            Text("Not copied yet.")
        }
        if let conflicts = status.conflicts, !conflicts.isEmpty {
            Text("Changed in both places: \(conflicts.joined(separator: ", ")). Allternit kept its version; the copy's is saved beside it \(place).")
        }
        if let failed = status.failed, !failed.isEmpty {
            Text("Couldn't bring back: \(failed.joined(separator: "; "))").foregroundColor(Theme.statusError)
        }
    }

    private func display(_ failure: SettingsLoadFailure) -> SettingsLoadFailure {
        if case .error = failure, let failureText { return .error(failureText) }
        return failure
    }

    @MainActor
    private func apply(_ value: MemoryDriveCopies) {
        copies = value
        folder = value.folder?.path ?? ""
        gitURL = value.git?.url ?? ""
        token = ""
    }

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        failure = nil
        failureText = nil
        do {
            apply(try await MemoryDriveAPI.copies())
            unsupported = false
        } catch {
            if MemoryDriveAPI.isNotFound(error) {
                unsupported = true
            } else {
                failure = SettingsLoadFailure.from(error)
                failureText = MemoryDriveAPI.message(error)
            }
        }
    }

    @MainActor
    private func save(_ which: String, _ call: @escaping @Sendable () async throws -> MemoryDriveCopies) async {
        busy = which
        error = nil
        notice = nil
        defer { busy = nil }
        do {
            apply(try await call())
            notice = "Copy settings saved."
        } catch {
            self.error = MemoryDriveAPI.message(error)
        }
    }
}
