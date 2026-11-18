import SwiftUI

/// Workspace files browser for Bot Home: lists the agent's .md context files
/// by layout category, opens the editor, and creates known docs or custom files.
struct BotWorkspaceFilesSheet: View {
    let agent: AgentRecord

    @Environment(\.dismiss) private var dismiss
    @State private var files: [WorkspaceFileInfo] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil
    @State private var isCreating = false
    @State private var fileToEdit: WorkspaceFileInfo? = nil
    @State private var isNewFileSheetPresented = false

    private let agentClient = AgentClient()

    private struct CategoryGroup: Identifiable {
        let category: AgentWorkspaceCategory
        var files: [WorkspaceFileInfo]
        var missingDocs: [String]
        var id: String { category.id }
    }

    private var categoryGroups: [CategoryGroup] {
        AgentWorkspaceLayout.categories.map { category in
            let prefix = category.directory.isEmpty ? "" : category.directory + "/"
            let categoryFiles = files.filter { info in
                category.directory.isEmpty
                    ? !info.path.contains("/")
                    : info.path.hasPrefix(prefix)
            }
            let missing = category.knownDocs.filter { doc in
                !categoryFiles.contains { $0.path == prefix + doc }
            }
            return CategoryGroup(category: category, files: categoryFiles, missingDocs: missing)
        }
    }

    var body: some View {
        NavigationStack {
            Group {
                if isLoading && files.isEmpty {
                    ProgressView()
                } else if let loadError, files.isEmpty {
                    VStack(spacing: 12) {
                        Text("Couldn't load workspace files")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text(loadError)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .multilineTextAlignment(.center)
                        Button("Retry") {
                            Task { await loadFiles() }
                        }
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                    }
                    .padding(.horizontal, 20)
                } else {
                    List {
                        ForEach(Array(categoryGroups.enumerated()), id: \.element.id) { index, group in
                            Section {
                                ForEach(group.files) { file in
                                    Button(action: {
                                        hapticLight()
                                        fileToEdit = file
                                    }) {
                                        HStack {
                                            Text(file.path.split(separator: "/").last.map(String.init) ?? file.path)
                                                .font(.system(.subheadline, design: .monospaced))
                                                .foregroundColor(Color("TextPrimary"))
                                            Spacer()
                                            Text(file.sizeLabel)
                                                .font(.caption)
                                                .foregroundColor(Color("TextSecondary"))
                                            Image(systemName: "chevron.right")
                                                .font(.system(size: 11, weight: .bold))
                                                .foregroundColor(Color("TextSecondary"))
                                        }
                                    }
                                    .buttonStyle(.plain)
                                }

                                ForEach(group.missingDocs, id: \.self) { doc in
                                    Button(action: {
                                        Task { await createKnownDoc(doc, in: group.category) }
                                    }) {
                                        HStack(spacing: 8) {
                                            Image(systemName: "plus.circle")
                                                .font(.subheadline)
                                                .foregroundColor(Color("AccentPrimary"))
                                            Text(doc)
                                                .font(.system(.subheadline, design: .monospaced))
                                                .foregroundColor(Color("TextSecondary"))
                                            Spacer()
                                            if isCreating {
                                                ProgressView()
                                            } else {
                                                Text("Create")
                                                    .font(.caption)
                                                    .foregroundColor(Color("TextSecondary"))
                                            }
                                        }
                                    }
                                    .buttonStyle(.plain)
                                    .disabled(isCreating)
                                }
                            } header: {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(group.category.label.uppercased())
                                        .font(.system(size: 10, weight: .bold))
                                        .tracking(0.8)
                                    Text(group.category.guidance)
                                        .font(.caption2)
                                        .foregroundColor(Color("TextSecondary").opacity(0.8))
                                }
                            }
                        }
                    }
                    .listStyle(.insetGrouped)
                    .scrollContentBackground(.hidden)
                }
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Workspace Files")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: { isNewFileSheetPresented = true }) {
                        Image(systemName: "plus")
                            .font(.system(size: 15, weight: .semibold))
                    }
                    .disabled(isLoading && files.isEmpty)
                }
            }
            .navigationDestination(item: $fileToEdit) { file in
                WorkspaceFileEditorView(agentId: agent.id, agentName: agent.name, file: file)
            }
            .sheet(isPresented: $isNewFileSheetPresented) {
                BotNewWorkspaceFileSheet(
                    agentId: agent.id,
                    existingPaths: Set(files.map(\.path))
                ) { created in
                    fileToEdit = created
                }
            }
            .task {
                await loadFiles()
            }
        }
    }

    private func loadFiles() async {
        isLoading = true
        loadError = nil
        do {
            files = try await agentClient.listWorkspaceFiles(agentId: agent.id)
        } catch is CancellationError {
            // no-op
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    private func createKnownDoc(_ doc: String, in category: AgentWorkspaceCategory) async {
        isCreating = true
        let path = category.path(for: doc)
        let content = AgentWorkspaceLayout.starterContent(for: path)
        do {
            try await agentClient.writeWorkspaceFile(agentId: agent.id, path: path, content: content)
            await loadFiles()
            fileToEdit = WorkspaceFileInfo(path: path, sizeBytes: content.utf8.count, modifiedAt: nil)
        } catch {
            loadError = "Couldn't create \(doc): \(error.localizedDescription)"
        }
        isCreating = false
    }

    private func hapticLight() {
        Haptics.light()
    }
}

/// Guided new context-file creation, scoped to Bot Home's workspace files sheet.
private struct BotNewWorkspaceFileSheet: View {
    let agentId: String
    let existingPaths: Set<String>
    let onCreated: (WorkspaceFileInfo) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var category: AgentWorkspaceCategory = AgentWorkspaceLayout.categories[0]
    @State private var name = ""
    @State private var isCreating = false
    @State private var createError: String? = nil

    private let agentClient = AgentClient()

    private var fileName: String {
        let base = name.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: " ", with: "-")
        guard !base.isEmpty else { return "" }
        let lower = base.lowercased()
        return lower.hasSuffix(".md") ? lower : lower + ".md"
    }

    private var fullPath: String {
        fileName.isEmpty ? "" : category.path(for: fileName)
    }

    private var isDuplicate: Bool {
        !fullPath.isEmpty && existingPaths.contains(fullPath)
    }

    private var canCreate: Bool {
        !fileName.isEmpty && !isDuplicate && !isCreating
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Picker("Category", selection: $category) {
                        ForEach(AgentWorkspaceLayout.categories) { entry in
                            Text(entry.label).tag(entry)
                        }
                    }
                    Text(category.guidance)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                } header: {
                    Text("Category")
                }

                Section {
                    TextField("e.g. Launch Notes", text: $name)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                } header: {
                    Text("Document")
                } footer: {
                    if isDuplicate {
                        Text("A file at this path already exists — open it from the list instead.")
                            .foregroundColor(Theme.statusWarning)
                    } else if !fullPath.isEmpty {
                        Text(fullPath)
                            .font(.system(.caption, design: .monospaced))
                    } else {
                        Text("Saved as a lowercase .md name inside the category.")
                    }
                }

                if let createError {
                    Section {
                        Text(createError)
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("New Context File")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: create) {
                        if isCreating {
                            ProgressView()
                        } else {
                            Text("Create")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(!canCreate)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func create() {
        isCreating = true
        createError = nil
        let path = fullPath
        let content = AgentWorkspaceLayout.starterContent(for: path)
        Task {
            do {
                try await agentClient.writeWorkspaceFile(agentId: agentId, path: path, content: content)
                dismiss()
                onCreated(WorkspaceFileInfo(path: path, sizeBytes: content.utf8.count, modifiedAt: nil))
            } catch {
                createError = "Couldn't create the file: \(error.localizedDescription)"
            }
            isCreating = false
        }
    }
}
