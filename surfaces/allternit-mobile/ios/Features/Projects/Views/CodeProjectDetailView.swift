import SwiftUI
import UIKit

/// One code project (allternit-ai CodeProjectView, phone sizes): a "Projects /
/// name" breadcrumb, the description and folder, New session, and the
/// project's sessions (`metadata.workspaceId`). A session opens in the Code
/// thread view; a new one is stamped with the workspace once it exists, the
/// way the web's createCodeSession does. The folder is on the connected
/// computer; a project made here asks for it once, and Cancel skips it.
struct CodeProjectDetailView: View {
    let project: WorkspaceLibraryProject
    var asksForFolder = false

    @ObservedObject private var library = ProjectsLibraryStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var thread: CodeThreadTarget? = nil
    @State private var isLoading = false
    @State private var picksFolder = false
    @State private var didAskForFolder = false
    @State private var folderError: String? = nil

    /// The row as the store has it now, with a folder just picked.
    private var live: WorkspaceLibraryProject {
        library.codeProjects.first { $0.id == project.id } ?? project
    }

    private var sessions: [AgentSession] { library.codeSessions[project.nativeId] ?? [] }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            breadcrumb
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    hero
                    newSessionButton
                    sessionList
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 32)
            }
            .refreshable { await reload() }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .toolbar(.hidden, for: .navigationBar)
        .navigationDestination(item: $thread) { target in
            CodeProjectThreadScreen(target: target, project: live)
        }
        .sheet(isPresented: $picksFolder) {
            ProjectFolderPickerSheet(title: live.title) { folder in
                picksFolder = false
                saveFolder(folder.path)
            }
        }
        .task {
            if asksForFolder, !didAskForFolder {
                didAskForFolder = true
                picksFolder = true
            }
            await reload()
        }
        // Back from a thread: a new session or a new name shows here.
        .onChange(of: thread) { _, open in
            if open == nil { Task { await reload() } }
        }
    }

    private func saveFolder(_ path: String) {
        Task {
            do {
                folderError = nil
                try await library.setDirectory(path, for: live)
            } catch {
                folderError = "Couldn't save the folder. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func reload() async {
        isLoading = true
        await library.loadThreadCounts()
        isLoading = false
    }

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
            Text(project.title)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .font(.system(size: 14))
        .padding(.horizontal, 16)
    }

    private var hero: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(project.title)
                .font(.system(size: 24, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            if let description = project.description, !description.isEmpty {
                Text(description)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
            }
            Button { picksFolder = true } label: {
                Label(live.directory ?? "Choose a folder", systemImage: "folder")
                    .font(.system(size: 12.5, design: live.directory == nil ? .default : .monospaced))
                    .foregroundColor(Color(live.directory == nil ? "TextSecondary" : "TextTertiary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(live.directory.map { "Folder \($0). Change" } ?? "Choose a folder")
            if let folderError {
                Text(folderError)
                    .font(.system(size: 12.5))
                    .foregroundColor(Theme.statusError)
            }
        }
    }

    private var newSessionButton: some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            thread = CodeThreadTarget(sessionId: nil)
        } label: {
            Label("New session", systemImage: "plus")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("BgPrimary"))
                .frame(maxWidth: .infinity, minHeight: 44)
                .background(Color("TextPrimary"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var sessionList: some View {
        if sessions.isEmpty {
            if isLoading {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 32)
            } else {
                Text("No sessions yet. Start one to work on this project.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 32)
            }
        } else {
            Text("Sessions")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .padding(.top, 6)
            LazyVStack(spacing: 8) {
                ForEach(sessions) { session in
                    Button {
                        thread = CodeThreadTarget(sessionId: session.id)
                    } label: {
                        row(session)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    private func row(_ session: AgentSession) -> some View {
        HStack(spacing: 10) {
            Text(session.name ?? "Untitled session")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Spacer(minLength: 0)
            Text(libraryShortAgo(libraryParseDate(session.updatedAt)))
                .font(.system(size: 12).monospacedDigit())
                .foregroundColor(Color("TextTertiary"))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 48)
        .background(Color("BgPanel"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
    }
}

/// A code session opened from its project. The chat stamps and draws a thread
/// by the app's mode, so the mode is Code while this is on screen and goes back
/// after; the section stays Projects, so Back returns here.
private struct CodeProjectThreadScreen: View {
    let target: CodeThreadTarget
    let project: WorkspaceLibraryProject

    @EnvironmentObject private var modeStore: AppModeStore
    @State private var previousMode: AppMode? = nil
    @State private var terminal = false
    @State private var linked = false
    @State private var linkError: String? = nil

    var body: some View {
        Group {
            // The first render must already see Code.
            if previousMode != nil {
                CodeThreadChatView(
                    target: target,
                    folder: project.directory.map { CodeFolderInfo(directory: $0, branch: nil) },
                    onSessionChanged: link,
                    terminal: terminal,
                    onLeaveTerminal: { terminal = false },
                    onEnterTerminal: { terminal = true }
                )
            } else {
                Color("BgPrimary")
            }
        }
        .navigationTitle(project.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .onAppear {
            guard previousMode == nil else { return }
            previousMode = modeStore.mode
            modeStore.mode = .code
        }
        .onDisappear {
            if let previousMode { modeStore.mode = previousMode }
            previousMode = nil
        }
        .alert("Couldn't add the session to this project", isPresented: Binding(
            get: { linkError != nil },
            set: { if !$0 { linkError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(linkError ?? "")
        }
    }

    /// A new session joins the project as soon as it exists.
    private func link(_ sessionId: String?) {
        guard target.sessionId == nil, let sessionId, !linked else { return }
        linked = true
        var metadata = ["workspaceId": project.nativeId, "projectMode": "code"]
        if let directory = project.directory { metadata["directory"] = directory }
        Task {
            do {
                try await AgentChatClient().updateSessionMetadata(
                    sessionId: sessionId,
                    metadata: metadata
                )
            } catch {
                linkError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}
