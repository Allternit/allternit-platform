import SwiftUI

/// `GET /api/v1/memory/drive/info` (docs/MEMORY_DRIVE_CONTRACT.md).
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

/// Settings → Assistant → Memory → Memory Drive: the git-backed memory repo
/// the bots write to (the PWA's MemoryDrivePanel). The phone reads it: drive
/// status, the file list, and each file's text. Editing, history and diffs
/// stay on Desktop / the web for now.
struct MemoryDriveView: View {
    @State private var info: MemoryDriveInfo? = nil
    @State private var files: [MemoryDriveFileEntry] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil
    @State private var search = ""

    private var filtered: [MemoryDriveFileEntry] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return files }
        return files.filter { $0.path.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        List {
            if let info {
                Section {
                    row("Drive", info.name)
                    if let revision = info.revision {
                        row("Revision", String(revision.prefix(8)))
                    }
                    if let pending = info.pendingWrites, pending > 0 {
                        row("Waiting to save", "\(pending)")
                    }
                    if info.indexDirty == true {
                        Text("Search index is catching up.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }

            Section {
                if isLoading && files.isEmpty {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let loadError, files.isEmpty {
                    let offline = loadError.localizedCaseInsensitiveContains("offline")
                        || loadError.localizedCaseInsensitiveContains("connect")
                    FriendlyInlineStateView(
                        style: offline ? .offline : .error,
                        icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                        title: offline ? "Your computer is offline" : "Couldn't open Memory Drive",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Try again",
                        action: { Task { await load() } }
                    )
                } else if files.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "brain",
                        title: "Nothing saved yet",
                        message: "Your bots save what they learn here as they work."
                    )
                } else {
                    ForEach(filtered) { file in
                        NavigationLink {
                            MemoryDriveFileView(path: file.path)
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
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Memory Drive")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Find a file")
        .task { await load() }
        .refreshable { await load() }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.subheadline).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value).font(.subheadline).foregroundColor(Color("TextSecondary"))
        }
    }

    @MainActor
    private func load() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        info = try? await APIClient.shared.get(path: "memory/drive/info")
        do {
            let tree: MemoryDriveTree = try await APIClient.shared.get(path: "memory/drive/tree")
            files = (tree.files ?? []).sorted { $0.path < $1.path }
        } catch {
            loadError = error.localizedDescription
        }
    }
}

/// One Memory Drive file, read-only.
struct MemoryDriveFileView: View {
    let path: String
    @State private var file: MemoryDriveFile? = nil
    @State private var loadError: String? = nil

    var body: some View {
        Group {
            if let file {
                ScrollView {
                    Text(file.content)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(16)
                }
            } else if let loadError {
                FriendlyStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't open this file",
                    message: FriendlyErrorMessage.from(loadError),
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
        .task { await load() }
    }

    @MainActor
    private func load() async {
        loadError = nil
        let escaped = path.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=+"))) ?? path
        do {
            file = try await APIClient.shared.get(path: "memory/drive/file?path=\(escaped)")
        } catch {
            loadError = error.localizedDescription
        }
    }
}
