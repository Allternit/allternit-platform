import SwiftUI

/// Workspace .md file editor (mockup A2 → editor), pushed from the agent
/// detail's Workspace files section: a monospace editor over
/// `GET /agents/:id/workspace/file?path=…`, saved back with
/// `PUT /agents/:id/workspace/file`.
///
/// The "Edit with chat" panel mirrors the web platform's
/// WorkspaceChatEditor.tsx: the current content + a natural-language
/// instruction stream a proposed revision (single-shot, preview → Apply —
/// nothing touches disk until the user Applies and then Saves). The assist
/// runs in an EPHEMERAL session so it never appears in chat history, and
/// rides the agent's own id, so the bridge composes the agent's persona
/// and model for the revision (v1_routes.rs agent_chat_bridge).
///
/// STYLE.md is platform-managed (the response-style endpoint rewrites it on
/// every preferences sync, plan Phase 1.4), so it carries a banner warning
/// that hand edits will be overwritten.
struct WorkspaceFileEditorView: View {
    let agentId: String
    let agentName: String
    let file: WorkspaceFileInfo
    /// Soul page mode: non-nil shows the agent's Description above the file
    /// and saves it with the file; the page also saves when you leave it.
    var initialDescription: String? = nil

    @State private var content: String = ""
    @State private var isLoading = true
    @State private var loadError: String? = nil
    @State private var isSaving = false
    /// "Saved" confirmation flash after a successful PUT.
    @State private var didSave = false
    @State private var actionError: String? = nil
    /// What the server had when the file was loaded (dirty check, and the
    /// `expected_hash` a save sends so a change made elsewhere is noticed).
    @State private var loadedContent = ""
    @State private var loadedHash: String? = nil
    @State private var descriptionText = ""
    @State private var loadedDescription = ""
    /// The current server copy when a save found the file changed elsewhere.
    @State private var conflict: ConflictCopy? = nil

    private struct ConflictCopy { let hash: String?; let content: String }

    // Edit-with-chat state.
    @State private var instruction = ""
    @State private var isAssisting = false
    @State private var streamedDraft = ""
    /// Proposed revision awaiting Apply/Discard (the web's preview card).
    @State private var preview: String? = nil
    @State private var assistError: String? = nil
    @State private var assistTask: Task<Void, Never>? = nil
    /// Ephemeral backing session for the assist, reused across instructions.
    @State private var assistSessionId: String? = nil

    private let agentClient = AgentClient()
    private let chatClient = AgentChatClient()

    /// STYLE.md is fully platform-managed (rewritten by the response-style
    /// sync) — editing it is allowed but never survives the next sync.
    private var isManagedFile: Bool {
        file.path == "STYLE.md"
    }

    private var fileName: String {
        file.path.split(separator: "/").last.map(String.init) ?? file.path
    }

    /// The layout category this file lives in (from its top-level
    /// directory); root files are Core.
    private var categoryLabel: String {
        let directory = file.path.contains("/")
            ? String(file.path.split(separator: "/").first!)
            : ""
        return AgentWorkspaceLayout.categories
            .first { $0.directory == directory }?.label ?? "Core"
    }

    var body: some View {
        VStack(spacing: 0) {
            if isManagedFile {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.caption)
                    Text("Managed by response-style settings — changes will be overwritten")
                        .font(.caption)
                    Spacer(minLength: 0)
                }
                .foregroundColor(Theme.statusWarning)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.statusWarning.opacity(0.12))
            }

            if isLoading {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError {
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load the file",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { Task { await loadContent() } }
                )
            } else {
                if initialDescription != nil { descriptionField }
                // Meta strip: category tag + live character count.
                HStack(spacing: 8) {
                    Text(categoryLabel.uppercased())
                        .font(.system(size: 10, weight: .bold))
                        .tracking(0.8)
                        .foregroundColor(Color("AccentPrimary"))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(Color("AccentPrimary").opacity(0.12))
                        .clipShape(Capsule())
                    Text(file.path)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text("\(content.count) chars")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                .padding(.horizontal, 20)
                .padding(.top, 12)
                .padding(.bottom, 8)

                // Editor card — Runestone gutter + Tree-sitter highlighting,
                // language resolved from the file's extension.
                CodeEditorView(text: $content, language: CodeLanguage.language(forPath: file.path))
                    .padding(.vertical, 6)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                    .padding(.horizontal, 16)
                    .padding(.bottom, 10)
            }

            if !isLoading, loadError == nil {
                assistPanel
            }

            if let actionError {
                Text(actionError)
                    .font(.caption)
                    .foregroundColor(Theme.statusWarning)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
            }
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(fileName)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: save) {
                    if isSaving {
                        ProgressView()
                    } else {
                        Text(didSave ? "Saved" : "Save")
                            .fontWeight(.semibold)
                            .foregroundColor(didSave ? Theme.statusSuccess : Color("AccentPrimary"))
                    }
                }
                .disabled(isLoading || loadError != nil || isSaving)
            }
        }
        .task {
            descriptionText = initialDescription ?? ""
            loadedDescription = descriptionText
            await loadContent()
        }
        .onDisappear {
            // Leaving mid-revision: cancel locally; the ephemeral session
            // is purged server-side on abort.
            assistTask?.cancel()
            // The Soul page saves when you leave it.
            if initialDescription != nil, isDirty, !isSaving, conflict == nil, !isLoading, loadError == nil {
                Task { @MainActor in _ = await performSave(force: false) }
            }
        }
        .alert("Changed elsewhere", isPresented: Binding(
            get: { conflict != nil }, set: { if !$0 { conflict = nil } }
        )) {
            Button("Reload") {
                if let conflict {
                    content = conflict.content
                    loadedContent = conflict.content
                    loadedHash = conflict.hash
                }
                conflict = nil
            }
            Button("Keep mine") {
                // Take the current revision as the base, then save over it.
                loadedHash = conflict?.hash
                conflict = nil
                Task { @MainActor in _ = await performSave(force: false) }
            }
            Button("Cancel", role: .cancel) { conflict = nil }
        } message: {
            Text("This file was changed on another device or by the bot while you were editing. Reload to see the new version, or keep yours and save over it.")
        }
    }

    private var isDirty: Bool {
        content != loadedContent || (initialDescription != nil && descriptionText != loadedDescription)
    }

    private var descriptionField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Description")
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
            TextField("What \(agentName) is for", text: $descriptionText, axis: .vertical)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1...3)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD).stroke(Theme.borderWarmDefault, lineWidth: 1))
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
    }

    // MARK: - Edit with chat

    /// The assist panel (web: WorkspaceChatEditor — border-top panel under
    /// the editor): header, optional error, optional preview card, and the
    /// instruction composer.
    private var assistPanel: some View {
        VStack(spacing: 8) {
            Divider().background(Color("BorderSubtle"))

            HStack(spacing: 6) {
                Image(systemName: "sparkles")
                    .font(.caption)
                    .foregroundColor(Color("AccentPrimary"))
                Text("Edit with chat")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                if isAssisting {
                    ProgressView()
                        .scaleEffect(0.7)
                }
                Spacer()
            }
            .padding(.horizontal, 16)

            if let assistError {
                Text(assistError)
                    .font(.caption)
                    .foregroundColor(Theme.statusWarning)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
            }

            if let preview {
                previewCard(preview)
            }

            HStack(alignment: .bottom, spacing: 8) {
                TextField("Tell \(agentName) how to change this file…", text: $instruction, axis: .vertical)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1...4)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                    .disabled(isAssisting)

                if isAssisting {
                    Button(action: stopAssist) {
                        Image(systemName: "stop.fill")
                            .font(.system(size: 13))
                            .foregroundColor(.black)
                            .frame(width: 32, height: 32)
                            .background(Color("AccentPrimary"))
                            .clipShape(Circle())
                    }
                } else {
                    Button(action: sendAssistInstruction) {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(.black)
                            .frame(width: 32, height: 32)
                            .background(Color("AccentPrimary"))
                            .clipShape(Circle())
                    }
                    .disabled(instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 10)
        }
        .background(Color("BgSecondary"))
    }

    /// Proposed-revision card (web parity: accent border, Apply/Discard —
    /// the file on disk is untouched until Apply loads it into the editor
    /// and the user Saves).
    private func previewCard(_ proposal: String) -> some View {
        VStack(spacing: 0) {
            HStack {
                Text("Proposed content — review, then Apply")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 8)
                Button(action: {
                    content = proposal
                    preview = nil
                    didSave = false
                }) {
                    Label("Apply", systemImage: "checkmark")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("AccentPrimary"))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .background(Color("AccentPrimary").opacity(0.12))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(Color("AccentPrimary").opacity(0.35), lineWidth: 1)
                        )
                }
                Button(action: { preview = nil }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(Color("BorderSubtle"), lineWidth: 1)
                        )
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider().background(Color("BorderSubtle"))

            // Inline green/red diff against the on-screen content, rather
            // than a flat dump of the proposed file — reuses the same
            // DiffLine/DiffRenderer the session diff viewer and permission
            // approval sheet already render with.
            ScrollView {
                DiffRenderer(lines: DiffLine.diffLines(before: content, after: proposal))
                    .padding(8)
            }
            .frame(maxHeight: 220)
        }
        .background(Color("BgPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Color("AccentPrimary").opacity(0.35), lineWidth: 1)
        )
        .padding(.horizontal, 16)
    }

    // MARK: - Actions

    private func loadContent() async {
        isLoading = true
        loadError = nil
        do {
            let file = try await HermesClient.shared.readFile(agentId: agentId, path: file.path)
            content = file.content
            loadedContent = file.content
            loadedHash = file.hash
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch APIError.httpError(let status, _) where status == 404 && initialDescription != nil {
            // No SOUL.md yet: start an empty one (saved with no hash check).
            content = ""
            loadedContent = ""
            loadedHash = nil
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    private func save() {
        actionError = nil
        didSave = false
        Task { @MainActor in
            if await performSave(force: false) { didSave = true }
        }
    }

    /// Saves the description (Soul page) and the file. The file goes with the
    /// hash it was loaded at; a 409 opens the "changed elsewhere" prompt.
    /// Returns true when everything was saved.
    @MainActor
    private func performSave(force: Bool) async -> Bool {
        guard !isSaving else { return false }
        isSaving = true
        defer { isSaving = false }
        do {
            if initialDescription != nil, descriptionText != loadedDescription {
                try await agentClient.updateAgent(id: agentId, description: descriptionText)
                loadedDescription = descriptionText
            }
            if content != loadedContent {
                let result = try await HermesClient.shared.writeFile(
                    agentId: agentId, path: file.path, content: content,
                    expectedHash: force ? nil : loadedHash
                )
                switch result {
                case .saved(let hash):
                    loadedContent = content
                    loadedHash = hash
                case .conflict(let hash, let current):
                    conflict = ConflictCopy(hash: hash, content: current)
                    return false
                }
            }
            return true
        } catch {
            actionError = "Couldn't save: \(error.localizedDescription)"
            return false
        }
    }

    /// Single-shot revision (web: WorkspaceChatEditor.handleSend): the
    /// current content + instruction stream into the draft buffer; the
    /// fence-stripped result becomes the preview. The ephemeral session is
    /// created lazily and reused so follow-up instructions keep context.
    private func sendAssistInstruction() {
        let text = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isAssisting else { return }
        assistError = nil
        preview = nil
        streamedDraft = ""
        isAssisting = true

        assistTask = Task {
            defer { isAssisting = false }
            do {
                if assistSessionId == nil {
                    let session = try await chatClient.createSession(
                        name: "Workspace edit — \(file.path)",
                        originSurface: "chat",
                        sessionMode: "agent",
                        agentId: agentId,
                        agentName: agentName,
                        ephemeral: true
                    )
                    assistSessionId = session.id
                }
                guard let sessionId = assistSessionId else { return }

                // Same framing as WorkspaceChatEditor.tsx:69-78 — sent as
                // the client systemPrompt, which the bridge appends LAST
                // (on top of the agent's own persona + preferences).
                let systemPrompt = """
                You are editing one workspace file of the agent "\(agentName)" on the Allternit platform.
                File: \(file.path)
                Revise the file according to the user instruction and return ONLY the complete new file content.
                Do not add explanations, do not wrap in markdown code fences, preserve the existing format unless the instruction says otherwise.

                --- CURRENT FILE CONTENT ---
                \(content)
                --- END CURRENT FILE CONTENT ---
                """

                for try await event in chatClient.sendMessageStream(
                    sessionId: sessionId,
                    text: text,
                    agentId: agentId,
                    systemPrompt: systemPrompt
                ) {
                    try Task.checkCancellation()
                    if case .textDelta(let payload) = event {
                        streamedDraft += payload.text
                    }
                }

                let next = Self.stripCodeFence(streamedDraft)
                if next.isEmpty {
                    assistError = "The model returned an empty response"
                } else {
                    preview = next
                }
            } catch is CancellationError {
                // Stopped by the user — keep whatever state we had.
            } catch {
                assistError = error.localizedDescription
            }
            instruction = ""
        }
    }

    private func stopAssist() {
        assistTask?.cancel()
        if let sessionId = assistSessionId {
            Task { try? await chatClient.abort(sessionId: sessionId) }
        }
    }

    /// Strip a single wrapping markdown code fence, if the model added one
    /// (web: WorkspaceChatEditor.tsx stripCodeFence).
    private static func stripCodeFence(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("```"), trimmed.hasSuffix("```") else { return trimmed }
        var lines = trimmed.split(separator: "\n", omittingEmptySubsequences: false)
        if lines.count >= 2, lines.first?.hasPrefix("```") == true, lines.last == "```" {
            lines.removeFirst()
            lines.removeLast()
            return lines.joined(separator: "\n")
        }
        return trimmed
    }
}
