import SwiftUI

extension ArtifactStudioSession {
    /// Standing instructions for a session bound to an open editor: the web's
    /// `artifactNoteBody` for a bound binding, plus OWN_TOOLS_NOTE
    /// (src/lib/artifact-session/artifact-session.ts). nil without an editor.
    var editorSystemPrompt: String? {
        let note: String
        if let designProjectId {
            note = "## Artifact session\n\nThe user has the design canvas \"\(title)\" (Allternit Design project \(designProjectId)) open beside this chat. That canvas is what you work on, with the pane_artifact tool — the canvas's own tools, acting on exactly what the user sees:\n\n1. Call pane_artifact with action \"describe\" once, before the first change: it returns how the canvas works (frames, components, UI blocks, text), its tools with their input schemas, and every shape on it.\n2. Change the canvas with action \"call\" (tool = one of those tools, input = an object matching its schema). Use action \"read\" to see it again.\n\nBuild the design on the canvas itself — don't describe it in the chat instead. Afterwards, say briefly what changed. If pane_artifact reports the pane is closed, ask the user to open it."
        } else if let artifactId, kind.officeProduct != nil {
            let noun = kind.noun.lowercased()
            note = "## Artifact session\n\nThe user has the \(noun) \"\(title)\" (artifact id \(artifactId)) open in the editor beside this chat. That \(noun) is what you work on, and you work on it with the pane_artifact tool — the editor's own tools, acting on exactly what the user sees:\n\n1. Call pane_artifact with action \"describe\" once, before the first change: it returns how the editor's tools work, the tools with their input schemas, and the current \(noun).\n2. Read and change the \(noun) with action \"call\" (tool = one of those tools, input = an object matching its schema). Use action \"read\" to see its current state again.\n\nMake the change in the \(noun) itself — don't paste the \(noun) into the chat instead. Afterwards, say briefly what changed. If pane_artifact reports the editor is closed, ask the user to open it."
        } else {
            return nil
        }
        return note + "\n\n" + ArtifactStudioKind.ownToolsNote
    }
}

/// A studio session on the phone: the chat, and — one tap away, as the
/// PWA's phone layout does with its full-screen pane — the kind's real editor.
/// Docs, Sheets, Slides and Design open on their editor straight away, as the
/// web does (`takePhoneEditorOpen`). The other kinds show the kind's empty
/// state until the agent makes something; the pane then opens the newest v2
/// artifact whose `origin.session_id` is this session in the web's artifact
/// window. The pane refreshes while it is open.
struct ArtifactStudioView: View {
    let studio: ArtifactStudioSession

    @Environment(\.dismiss) private var dismiss
    @StateObject private var viewModel = ChatViewModel()
    @State private var showingPane: Bool
    @State private var results: [ArtifactV2Summary] = []
    @State private var paneError: String?
    @State private var editorSave = EditorSaveHandle()
    @State private var isSavingEditor = false
    /// Where the person was going when the editor couldn't save.
    @State private var leaveAfterFailedSave: (() -> Void)?
    /// What went wrong with the agent's last change to the open file. Shown over
    /// the chat and the editor alike; the model is told through its tool result.
    @State private var agentProblem: String?

    init(studio: ArtifactStudioSession) {
        self.studio = studio
        _showingPane = State(initialValue: studio.editorRoute != nil)
    }

    /// What the pane shows: the agent's newest result, else the session's editor.
    private var paneRoute: ArtifactWebRoute? {
        if let newest = results.first { return .window(id: newest.id) }
        return studio.editorRoute
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            ZStack(alignment: .top) {
                ChatContentView(sessionId: studio.sessionId, viewModel: viewModel)
                    .opacity(showingPane ? 0 : 1)
                    .allowsHitTesting(!showingPane)
                if studio.editorRoute != nil {
                    // The agent works on this file while the person is in the
                    // chat, so the editor stays open out of sight. Not fully
                    // transparent: WebKit may suspend a page it can't see.
                    pane
                        .ignoresSafeArea(edges: .bottom)
                        .opacity(showingPane ? 1 : 0.001)
                        .allowsHitTesting(showingPane)
                        .accessibilityHidden(!showingPane)
                } else if showingPane {
                    pane
                        .ignoresSafeArea(edges: .bottom)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                if let agentProblem {
                    TransientErrorBanner(message: agentProblem) { self.agentProblem = nil }
                        .padding(.horizontal, 12)
                        .padding(.top, 8)
                }
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .task(id: studio.sessionId) {
            viewModel.editorSystemPrompt = studio.editorSystemPrompt
            // The app's scripted sample workspace has no agent to answer.
            guard !SampleWorkspace.isEnabled else { return }
            await viewModel.servePaneArtifactRequests(
                sessionId: studio.sessionId,
                run: { request in await runAgentRequest(request) },
                onProblem: { agentProblem = $0 }
            )
        }
        .alert("Your changes couldn't be saved", isPresented: Binding(
            get: { leaveAfterFailedSave != nil },
            set: { if !$0 { leaveAfterFailedSave = nil } }
        )) {
            Button("Keep editing", role: .cancel) {}
            Button("Leave anyway", role: .destructive) { leaveAfterFailedSave?() }
        } message: {
            Text("Check your connection and try again.")
        }
        .task(id: studio.sessionId) {
            // Refresh the pane's results every few seconds while the studio
            // is open, so a result appears as soon as the agent saves it.
            while !Task.isCancelled {
                await refreshResults()
                try? await Task.sleep(for: .seconds(showingPane ? 4 : 8))
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { saveEditor { dismiss() } } label: {
                Group {
                    if isSavingEditor {
                        ProgressView()
                    } else {
                        Image(systemName: "xmark")
                            .font(.body.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                    }
                }
                .frame(width: 44, height: 44)
            }
            .accessibilityLabel(isSavingEditor ? "Saving" : "Close")

            VStack(alignment: .leading, spacing: 1) {
                Text(studio.title)
                    .font(.system(size: 15, weight: .semibold, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                Text(studio.kind.label)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 4)
            Picker("View", selection: Binding(
                get: { showingPane },
                set: { value in
                    if value {
                        withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = true }
                    } else {
                        saveEditor { showChat() }
                    }
                }
            )) {
                Text("Chat").tag(false)
                Text(paneLabel).tag(true)
            }
            .pickerStyle(.segmented)
            .fixedSize()
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
    }

    /// Short enough not to truncate beside the title ("Spreadsheet" did).
    private var paneLabel: String {
        let name = studio.editorRoute != nil ? "Edit" : studio.kind.noun
        return results.isEmpty ? name : "\(name) · \(results.count)"
    }

    @ViewBuilder
    private var pane: some View {
        Group {
            if let route = paneRoute {
                ArtifactLiveWebView(
                    route: route,
                    onClose: { saveEditor { showChat() } },
                    onChat: { text in
                        saveEditor {
                            showChat()
                            if !text.isEmpty { viewModel.sendMessage(text) }
                        }
                    },
                    saveHandle: editorSave
                )
                .id(route.paneKey)
            } else {
                VStack(spacing: 14) {
                    Image(studio.kind.iconAsset)
                        .resizable()
                        .frame(width: 40, height: 40)
                    Text(studio.kind.emptyTitle)
                        .font(.system(.title3, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                    Text(paneError.map(FriendlyErrorMessage.from) ?? studio.kind.studioHint)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                    Button("Describe it in the chat") { showChat() }
                        .font(.subheadline.weight(.semibold))
                        .padding(.top, 4)
                }
                .padding(32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Color("BgPrimary"))
    }

    /// The editor's web view goes away when the pane closes, so its unsaved
    /// changes are saved first.
    private func saveEditor(then leave: @escaping () -> Void) {
        guard !isSavingEditor else { return }
        isSavingEditor = true
        Task {
            let saved = await editorSave.save()
            isSavingEditor = false
            if saved {
                leave()
            } else {
                leaveAfterFailedSave = leave
            }
        }
    }

    private func showChat() {
        // The editor stays open behind the chat; typing must not go to it.
        editorSave.endEditing()
        withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = false }
    }

    /// One pane_artifact request on the open file. Its result goes to the model
    /// either way; a failure or an unsaved change is also shown to the person.
    @MainActor
    private func runAgentRequest(_ request: PaneArtifactRequest) async -> PaneArtifactResult {
        // A stored result's window names itself; the session's editor is named here.
        let label: String? = results.isEmpty ? "\(studio.kind.noun) \"\(studio.title)\"" : nil
        let result = await editorSave.runAgentRequest(
            payload: request.payload,
            label: label,
            agentName: viewModel.sessionContext.agentName
        )
        if !result.ok {
            agentProblem = result.error.map(FriendlyErrorMessage.from) ?? "The agent couldn't change the file."
        } else if result.mutated == true {
            agentProblem = await editorSave.save() ? nil : "The agent's change is in the editor but couldn't be saved. Check your connection."
        }
        return result
    }

    private func refreshResults() async {
        do {
            let page = try await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(scope: "mine", limit: 50))
            let next = page.items.filter { $0.origin?.sessionId == studio.sessionId }
            // A new result replaces the open editor in the pane; retried on the next refresh.
            if next.first?.id != results.first?.id, !(await editorSave.save()) { return }
            results = next
            paneError = nil
        } catch is CancellationError {
            return
        } catch {
            paneError = error.localizedDescription
        }
    }
}
