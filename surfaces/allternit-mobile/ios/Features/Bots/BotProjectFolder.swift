import SwiftUI

/// Per-device working folders, keyed by project id (the web's
/// `allternit-bot-project-device` store). Bots run on a computer, so on the
/// phone this is always a folder on that computer, never one on the iPhone.
@MainActor
final class ProjectFolderStore: ObservableObject {
    static let shared = ProjectFolderStore()

    struct Folder: Codable, Hashable {
        let path: String
        let name: String
    }

    private static let storageKey = "allternit-bot-project-device"
    @Published private(set) var folders: [String: Folder] = [:]

    private init() {
        if let data = UserDefaults.standard.data(forKey: Self.storageKey),
           let stored = try? JSONDecoder().decode([String: Folder].self, from: data) {
            folders = stored
        }
    }

    func folder(for projectId: String) -> Folder? { folders[projectId] }

    func set(_ folder: Folder?, for projectId: String) {
        folders[projectId] = folder
        if let data = try? JSONEncoder().encode(folders) {
            UserDefaults.standard.set(data, forKey: Self.storageKey)
        }
    }

    /// Threads just started here, until the thread list carries them.
    private var sessionProjects: [String: String] = [:]

    func remember(sessionId: String, projectId: String) { sessionProjects[sessionId] = projectId }

    func project(forSession sessionId: String) -> String? { sessionProjects[sessionId] }

    /// buildProjectContextBlock's opening: goes in the thread's system prompt.
    /// Null without a folder or goal: a title alone isn't worth a block.
    func contextBlock(projectId: String) -> String? {
        let project = BotModeStore.shared.projects.first { $0.id == projectId }
        var parts: [String] = []
        if let goal = project?.description?.trimmingCharacters(in: .whitespacesAndNewlines), !goal.isEmpty {
            parts.append("Goal: \(goal)")
        }
        if let folder = folder(for: projectId) { parts.append("Working folder: \(folder.path)") }
        guard !parts.isEmpty else { return nil }
        if let title = project?.title { parts.insert("Project: \(title)", at: 0) }
        return "Project context (applies to every message in this project):\n\(parts.joined(separator: "\n\n"))"
    }
}

/// The project page's Folder section (BaseProjectView): Only you, add, the
/// chosen path, remove.
struct ProjectFolderCard: View {
    let projectId: String
    let title: String
    /// The folder changed; the new path, or nil when removed.
    var onChange: (@MainActor (String?) -> Void)? = nil

    @ObservedObject private var store = ProjectFolderStore.shared
    @State private var picking = false

    var body: some View {
        ProjectCard(title: "Folder") {
            HStack(spacing: 6) {
                HStack(spacing: 4) {
                    Image(systemName: "lock").font(.system(size: 10))
                    Text("Only you")
                }
                .font(.system(size: 11))
                .foregroundColor(Color("TextTertiary"))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderSubtle"), lineWidth: 1))
                ProjectIconButton(systemName: "plus", label: "Choose folder") { picking = true }
            }
        } content: {
            if let folder = store.folder(for: projectId) {
                HStack(spacing: 8) {
                    Text(folder.path)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ProjectIconButton(systemName: "xmark", label: "Remove folder") {
                        store.set(nil, for: projectId)
                        onChange?(nil)
                    }
                }
            } else {
                ProjectCardNote("Add a folder on your computer for Allternit to work in.")
            }
        }
        .sheet(isPresented: $picking) {
            ProjectFolderPickerSheet(title: title) { folder in
                store.set(folder, for: projectId)
                onChange?(folder.path)
                picking = false
            }
        }
    }
}

/// Picks a folder on the connected computer: browse from its working folder,
/// or type any path.
struct ProjectFolderPickerSheet: View {
    let title: String
    let onPick: (ProjectFolderStore.Folder) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var client: FileClient?
    @State private var root: CodeFolderInfo?
    @State private var isResolving = true
    @State private var typed = ""
    @State private var typedError: String?

    var body: some View {
        NavigationStack {
            List {
                Section("Path on your computer") {
                    TextField("/Users/you/project", text: $typed)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.system(size: 14, design: .monospaced))
                    if let typedError {
                        Text(typedError).font(.system(size: 12)).foregroundColor(Theme.statusError)
                    }
                    Button("Use this path") { useTyped() }
                        .frame(minHeight: 44)
                        .disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                Section("On your computer") {
                    if isResolving {
                        ProgressView("Connecting…").frame(maxWidth: .infinity, minHeight: 44)
                    } else if let client, let root {
                        NavigationLink {
                            ProjectFolderLevel(client: client, path: nil, name: root.name, absolute: root.directory, onPick: { pick(path: $0) })
                        } label: {
                            folderLabel(root.name, detail: root.directory)
                        }
                    } else {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Connect a computer or Allternit Cloud to pick a folder.")
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextSecondary"))
                            Button("Retry") { Task { await resolve() } }
                                .buttonStyle(.borderless)
                                .frame(minHeight: 44)
                        }
                    }
                }
            }
            .navigationTitle("Folder for \(title)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
            .task { await resolve() }
        }
    }

    private func folderLabel(_ name: String, detail: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "folder").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            VStack(alignment: .leading, spacing: 1) {
                Text(name).font(.system(size: 14))
                Text(detail)
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .frame(minHeight: 44)
    }

    private func pick(path: String) {
        let trimmed = path.hasSuffix("/") && path.count > 1 ? String(path.dropLast()) : path
        let name = trimmed.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init) ?? trimmed
        onPick(ProjectFolderStore.Folder(path: trimmed, name: name))
    }

    /// A folder only means something as a full path on the computer.
    private func useTyped() {
        let path = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        let looksFull = path.hasPrefix("/") || path.hasPrefix("~") || path.contains(":\\") || path.contains(":/")
        guard looksFull else {
            typedError = "Enter the full path, starting with /."
            return
        }
        typedError = nil
        pick(path: path)
    }

    @MainActor
    private func resolve() async {
        isResolving = true
        defer { isResolving = false }
        let instances = InstanceStore.shared
        await instances.refreshIfNeeded()
        guard let baseURL = await InstanceConnection.resolveBaseURL(from: instances),
              let info = try? await CodeFolderClient.load(baseURL: baseURL) else {
            client = nil
            root = nil
            return
        }
        client = FileClient(baseURL: baseURL)
        root = info
    }
}

/// One level of the computer's folders, one call per level (`v1/file/tree`).
private struct ProjectFolderLevel: View {
    let client: FileClient
    let path: String?
    let name: String
    let absolute: String
    let onPick: (String) -> Void

    @State private var folders: [FileNode] = []
    @State private var isLoading = true
    @State private var failure: String?

    var body: some View {
        List {
            Section {
                Button("Use this folder") { onPick(absolute) }
                    .frame(minHeight: 44)
            } footer: {
                Text(absolute).font(.system(size: 11.5))
            }
            Section {
                if isLoading && folders.isEmpty {
                    ProgressView().frame(maxWidth: .infinity, minHeight: 44)
                } else if let failure, folders.isEmpty {
                    ProjectInlineError(text: "Couldn't load this folder. \(failure)") { Task { await load() } }
                } else if folders.isEmpty {
                    Text("No folders inside.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                }
                ForEach(folders) { node in
                    NavigationLink {
                        ProjectFolderLevel(client: client, path: node.path, name: node.name, absolute: node.absolute, onPick: onPick)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "folder").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                            Text(node.name)
                                .font(.system(size: 14))
                                .foregroundColor(node.ignored ? Color("TextTertiary") : Color("TextPrimary"))
                        }
                        .frame(minHeight: 44)
                    }
                }
            }
        }
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            folders = try await client.tree(path: path).filter { $0.type == .directory }
            failure = nil
        } catch is CancellationError {
        } catch {
            failure = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
