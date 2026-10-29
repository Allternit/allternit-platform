import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// Project detail in the desktop project page's layout (allternit-ai
/// ChatProjectDetail → BaseProjectView): a "Projects / name" breadcrumb, the
/// hero band (folder mark, serif name, description, section tabs), then
/// Overview (the project intro, "Start a thread" and its threads) or Details
/// ("Add files" / "Add instructions", the instructions editor → PUT
/// …/projects/:id, the attached files), and the composer card at the bottom,
/// which starts a chat in the project.
///
/// The old live backend has no files routes and doesn't persist
/// `metadata.projectId` on session create, so the files list renders its
/// error state and the chats section its empty state there; both light up on
/// a backend with the Phase-3 routes.
struct ProjectDetailView: View {
    let project: CoworkProject
    /// "+ New chat": selects this project and starts a fresh conversation
    /// stamped with it (handled by the sidebar, which owns the session id).
    let onNewChat: (CoworkProject) -> Void
    /// Tapping a chat row opens it in Home.
    let onOpenChat: (AgentSession) -> Void

    @StateObject private var projectStore = ProjectStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var instructions: String
    @State private var isEditingInstructions = false
    @State private var isSavingInstructions = false
    @State private var files: [CoworkProjectFile] = []
    @State private var filesError: String? = nil
    @State private var chats: [AgentSession] = []
    @State private var actionError: String? = nil

    @State private var isAddFilesDialogPresented = false
    @State private var isPhotosPickerPresented = false
    @State private var isFilePickerPresented = false
    @State private var selectedPhotoItems: [PhotosPickerItem] = []

    @State private var isRenameAlertPresented = false
    @State private var renameText = ""
    @State private var isDeleteConfirmPresented = false
    @State private var section: Section = .overview

    /// The hero band's tabs the phone has (desktop: Overview · Delegate ·
    /// Board · Details · Browser).
    private enum Section: String, CaseIterable {
        case overview, details
        var label: String { self == .overview ? "Overview" : "Details" }
        var icon: String? { self == .overview ? "list.bullet" : nil }
    }

    private let projectsClient = ProjectsClient()
    private let chatClient = AgentChatClient()

    init(project: CoworkProject,
         onNewChat: @escaping (CoworkProject) -> Void,
         onOpenChat: @escaping (AgentSession) -> Void) {
        self.project = project
        self.onNewChat = onNewChat
        self.onOpenChat = onOpenChat
        _instructions = State(initialValue: project.instructions ?? "")
        _isEditingInstructions = State(initialValue: (project.instructions ?? "").isEmpty)
    }

    /// The live store record (rename/instructions saves land here); falls
    /// back to the value captured on push.
    private var liveProject: CoworkProject {
        projectStore.project(withId: project.id) ?? project
    }

    var body: some View {
        VStack(spacing: 0) {
            breadcrumb
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    hero
                    VStack(alignment: .leading, spacing: 24) {
                        if let actionError {
                            Text(actionError)
                                .font(.caption)
                                .foregroundColor(Theme.statusWarning)
                        }
                        if section == .overview {
                            overviewIntro
                            chatsSection
                        } else {
                            addButtonsRow
                            instructionsSection
                            filesSection
                        }
                    }
                    .padding(.horizontal, 20)
                    .padding(.top, 28)
                    .padding(.bottom, 24)
                }
            }
            composerCard
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .toolbar(.hidden, for: .navigationBar)
        .alert("Rename project", isPresented: $isRenameAlertPresented) {
            TextField("Project name", text: $renameText)
            Button("Save") { renameProject() }
                .disabled(renameText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("Cancel", role: .cancel) {}
        }
        .alert("Delete this project?", isPresented: $isDeleteConfirmPresented) {
            Button("Delete", role: .destructive) { deleteProject() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Its chats are kept, but they're no longer grouped under a project.")
        }
        .confirmationDialog("Add files", isPresented: $isAddFilesDialogPresented, titleVisibility: .visible) {
            Button("Photos") { isPhotosPickerPresented = true }
            Button("Files") { isFilePickerPresented = true }
            Button("Cancel", role: .cancel) {}
        }
        .photosPicker(
            isPresented: $isPhotosPickerPresented,
            selection: $selectedPhotoItems,
            maxSelectionCount: 5,
            matching: .images
        )
        .sheet(isPresented: $isFilePickerPresented) {
            DocumentPicker { url in stagePickedFile(url) }
        }
        .onChange(of: selectedPhotoItems) { _, items in
            guard !items.isEmpty else { return }
            stagePickedPhotos(items)
            selectedPhotoItems = []
        }
        .task {
            await loadFiles()
            await loadChats()
        }
    }

    // MARK: - Desktop chrome

    /// "Projects / name", with the project's options on the right.
    private var breadcrumb: some View {
        HStack(spacing: 8) {
            Button { dismiss() } label: {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold))
                    Text("Projects")
                }
                .foregroundColor(Color("TextSecondary"))
                .frame(height: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back to Projects")
            Text("/").foregroundColor(Color("TextTertiary"))
            Text(liveProject.title)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Spacer(minLength: 0)
            Menu {
                Button(action: {
                    renameText = liveProject.title
                    isRenameAlertPresented = true
                }) {
                    Label("Rename", systemImage: "pencil")
                }
                Button(role: .destructive, action: { isDeleteConfirmPresented = true }) {
                    Label("Delete project", systemImage: "trash")
                }
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 17))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Project options")
        }
        .font(.system(size: 14))
        .padding(.horizontal, 16)
        .background(Color("BgPrimary"))
    }

    /// The hero band: folder mark, serif name, description, section tabs.
    private var hero: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 16) {
                Image(systemName: "folder")
                    .font(.system(size: 46, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                VStack(alignment: .leading, spacing: 6) {
                    Text(liveProject.title)
                        .font(.system(size: 28, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                    if let description = liveProject.description, !description.isEmpty {
                        Text(description)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            HStack(spacing: 6) {
                ForEach(Section.allCases, id: \.self) { tab in
                    let on = section == tab
                    Button { section = tab } label: {
                        HStack(spacing: 6) {
                            if let icon = tab.icon { Image(systemName: icon).font(.system(size: 12)) }
                            Text(tab.label)
                        }
                        .font(.system(size: 14, weight: on ? .medium : .regular))
                        .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .frame(height: 32)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? Color("BgPanel").opacity(0.7) : Color.clear))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 24)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            ZStack {
                Color(hex: "#E6E9ED")
                RadialGradient(colors: [Color(hex: "#A9B4C2").opacity(0.75), .clear],
                               center: .init(x: 0.62, y: 0.9), startRadius: 4, endRadius: 260)
            }
        )
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
        .environment(\.colorScheme, .light)
    }

    /// The project intro over its threads (BaseProjectView empty header).
    private var overviewIntro: some View {
        VStack(alignment: .leading, spacing: 8) {
            Image(systemName: "folder")
                .font(.system(size: 20))
                .foregroundColor(Color("TextPrimary"))
            Text(liveProject.title)
                .font(.system(size: 26, design: .serif))
                .foregroundColor(Color("TextPrimary"))
            if let description = liveProject.description, !description.isEmpty {
                Text(description)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
            }
            Text("Keep goals, decisions and updates here. Start a thread for a specific task.")
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Button { onNewChat(liveProject) } label: {
                HStack(spacing: 8) {
                    Image(systemName: "plus").font(.system(size: 13))
                    Text("Start a thread")
                }
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .padding(.leading, 12)
                .frame(height: 36)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    /// The composer card at the bottom: starts a chat in this project.
    private var composerCard: some View {
        Button { onNewChat(liveProject) } label: {
            VStack(alignment: .leading, spacing: 0) {
                Text("Ask a question or start work in this project…")
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.top, 14)
                    .padding(.bottom, 10)
                HStack {
                    Image(systemName: "plus")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 40, height: 40)
                        .background(Circle().fill(Color("FillNeutral")))
                    Spacer()
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("OnAccent"))
                        .frame(width: 40, height: 40)
                        .background(Circle().fill(Color("AccentPrimary").opacity(0.35)))
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 8)
                .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.horizontal, 8) }
            }
            .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .shadow(color: .black.opacity(0.04), radius: 4, y: 2)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Start a chat in \(liveProject.title)")
        .padding(.horizontal, 12)
        .padding(.top, 6)
        .padding(.bottom, 8)
        .background(Color("BgPrimary"))
    }

    // MARK: - Sections

    /// Claude's project page leads with these two affordances.
    private var addButtonsRow: some View {
        HStack(spacing: 12) {
            addButton(icon: "doc.badge.plus", label: "Add files") {
                isAddFilesDialogPresented = true
            }
            addButton(icon: "text.badge.plus", label: "Add instructions") {
                isEditingInstructions = true
            }
        }
    }

    private func addButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            VStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 22, weight: .medium))
                Text(label)
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity)
            .frame(height: 76)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusLG)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: Instructions

    private var instructionsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Instructions")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            if isEditingInstructions {
                TextEditor(text: $instructions)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 120)
                    .padding(10)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )

                HStack(spacing: 12) {
                    Button(action: saveInstructions) {
                        Text(isSavingInstructions ? "Saving…" : "Save")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("BgPrimary"))
                            .padding(.horizontal, 14)
                            .frame(height: 32)
                            .background(Color("AccentPrimary"))
                            .clipShape(Capsule())
                    }
                    .disabled(isSavingInstructions)

                    if !(liveProject.instructions ?? "").isEmpty {
                        Button("Cancel") {
                            instructions = liveProject.instructions ?? ""
                            isEditingInstructions = false
                        }
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                    }
                }
            } else {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    isEditingInstructions = true
                }) {
                    Text(liveProject.instructions ?? "")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: Files

    private var filesSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Files")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            if let filesError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't load files",
                    message: FriendlyErrorMessage.from(filesError),
                    actionTitle: "Retry",
                    action: {
                        Task { await loadFiles() }
                    }
                )
            } else if files.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "folder",
                    title: "No files yet",
                    message: "Add files to give every chat in this project the same context."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(files) { file in
                        fileRow(file)
                    }
                }
            }
        }
    }

    private func fileRow(_ file: CoworkProjectFile) -> some View {
        HStack(spacing: 12) {
            Image(systemName: Self.icon(for: file.mediaType))
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(file.name)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if let mediaType = file.mediaType {
                    Text(mediaType)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Spacer()
            Button(action: { deleteFile(file) }) {
                Image(systemName: "trash")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private static func icon(for mediaType: String?) -> String {
        guard let mediaType else { return "doc" }
        if mediaType.hasPrefix("image/") { return "photo" }
        if mediaType == "application/pdf" { return "doc.richtext" }
        if mediaType.hasPrefix("text/") { return "doc.text" }
        return "doc"
    }

    // MARK: Chats

    private var chatsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Threads (\(chats.count))")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    onNewChat(liveProject)
                }) {
                    HStack(spacing: 6) {
                        Image(systemName: "plus")
                            .font(.system(size: 12, weight: .semibold))
                        Text("New chat")
                            .font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 14)
                    .frame(height: 32)
                    .background(Color("BgSecondary"))
                    .clipShape(Capsule())
                    .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }
            }

            if chats.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "bubble.left",
                    title: "No threads yet",
                    message: "Start one from the composer."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(chats) { session in
                        Button(action: {
                            let generator = UIImpactFeedbackGenerator(style: .light)
                            generator.impactOccurred()
                            onOpenChat(session)
                        }) {
                            HStack(spacing: 12) {
                                Image(systemName: "bubble.left")
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextSecondary"))
                                Text(session.name ?? "Untitled")
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .padding(.horizontal, 14)
                            .frame(height: 48)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
                            )
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    // MARK: - Actions

    private func renameProject() {
        let title = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, title != liveProject.title else { return }
        Task {
            do {
                try await projectStore.renameProject(id: project.id, title: title)
            } catch {
                actionError = "Couldn't rename the project: \(error.localizedDescription)"
            }
        }
    }

    private func deleteProject() {
        Task {
            do {
                try await projectStore.deleteProject(id: project.id)
                dismiss()
            } catch {
                actionError = "Couldn't delete the project: \(error.localizedDescription)"
            }
        }
    }

    private func saveInstructions() {
        isSavingInstructions = true
        Task {
            do {
                try await projectStore.saveInstructions(id: project.id, instructions: instructions)
                isEditingInstructions = instructions.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            } catch {
                actionError = "Couldn't save instructions: \(error.localizedDescription)"
            }
            isSavingInstructions = false
        }
    }

    private func loadFiles() async {
        do {
            files = try await projectsClient.listFiles(projectId: project.id)
            filesError = nil
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            filesError = error.localizedDescription
        }
    }

    /// Chats stamped with this project's id — filtered client-side on
    /// `metadata.project_id` (AgentSession.projectId).
    private func loadChats() async {
        do {
            let envelope: AgentSessionListResponse = try await APIClient.shared.get(path: "agent-sessions")
            chats = envelope.sessions
                .filter { $0.projectId == project.id }
                .sorted { $0.updatedAt > $1.updatedAt }
        } catch {
            // The section falls back to its empty state; the banner above
            // already carries the failure when it's actionable.
        }
    }

    // MARK: - File staging (Add files)

    private func stagePickedPhotos(_ items: [PhotosPickerItem]) {
        Task {
            for (index, item) in items.enumerated() {
                guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
                let mediaType = item.supportedContentTypes.first?.preferredMIMEType ?? "image/jpeg"
                await attachFile(data: data, filename: "photo-\(index + 1).jpg", mediaType: mediaType)
            }
        }
    }

    private func stagePickedFile(_ url: URL) {
        guard let data = try? Data(contentsOf: url) else {
            filesError = "Couldn't read that file."
            return
        }
        let mediaType = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
            ?? "application/octet-stream"
        Task { await attachFile(data: data, filename: url.lastPathComponent, mediaType: mediaType) }
    }

    /// Uploads the blob (POST /api/v1/uploads), then records the metadata
    /// row on the project. A failure on either call surfaces under the Files
    /// section.
    private func attachFile(data: Data, filename: String, mediaType: String) async {
        do {
            let upload = try await chatClient.upload(
                name: filename,
                mediaType: mediaType,
                dataBase64: data.base64EncodedString()
            )
            try await projectsClient.addFile(
                projectId: project.id,
                name: filename,
                url: upload.url,
                uploadId: upload.uploadId,
                mediaType: mediaType
            )
            await loadFiles()
        } catch {
            filesError = "Couldn't attach \(filename): \(error.localizedDescription)"
        }
    }

    private func deleteFile(_ file: CoworkProjectFile) {
        Task {
            do {
                try await projectsClient.deleteFile(projectId: project.id, fileId: file.id)
                files.removeAll { $0.id == file.id }
            } catch {
                filesError = "Couldn't remove \(file.name): \(error.localizedDescription)"
            }
        }
    }
}
