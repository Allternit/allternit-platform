import SwiftUI

/// The code thread on screen. `sessionId == nil` is a fresh thread: the
/// backend session is created on the first send (ChatViewModel
/// .ensureSessionId), stamped `origin_surface='code'`. `initialPrompt` is
/// what the launch prompt sent.
struct CodeThreadTarget: Hashable, Identifiable {
    let key = UUID()
    let sessionId: String?
    var initialPrompt: String? = nil

    var id: UUID { key }
}

/// A code thread on the phone (CodeSessionHeader with `phone`, then the
/// thread): one top row with the session menu (title ⌄) and one folder ·
/// branch chip, the pending-changes banner, then the shared chat feed and
/// composer. Panes and session actions live in the Code header's panes
/// menu (CodeModeView).
struct CodeThreadChatView: View {
    let target: CodeThreadTarget
    let title: String?
    let folder: CodeFolderInfo?
    /// The backend session this thread is on (nil until the first send).
    let onSessionChanged: (String?) -> Void
    let onRename: () -> Void
    let onArchive: () -> Void
    /// The Terminal choice: this thread drawn as a terminal session.
    var terminal: Bool = false
    /// The terminal view's "Chat" item: back to the chat feed.
    var onLeaveTerminal: () -> Void = {}
    /// The chat view's "Terminal" item: this thread in its terminal look.
    var onEnterTerminal: () -> Void = {}
    /// The terminal view's Shell button: the computer's raw shell.
    var onOpenShell: () -> Void = {}

    @StateObject private var viewModel = ChatViewModel()
    @StateObject private var modelStore = ModelStore.shared
    @State private var didSendInitialPrompt = false

    /// Pending gizzi-code approval requests for this thread's session
    /// (`GET /v1/permission` + the permission event stream), oldest first so
    /// the banner always opens the longest-waiting request.
    @State private var pendingPermissions: [PermissionRequest] = []
    @State private var reviewingPermission: PermissionRequest? = nil

    private var sessionId: String? { viewModel.currentSessionId ?? target.sessionId }

    private var displayTitle: String {
        if let title, !title.isEmpty { return title }
        if let prompt = target.initialPrompt, !prompt.isEmpty {
            return prompt.prefix(1).uppercased() + prompt.dropFirst()
        }
        return "New thread"
    }

    var body: some View {
        VStack(spacing: 0) {
            if terminal {
                terminalHeader
            } else if sessionId != nil || target.initialPrompt != nil {
                sessionHeader
            }
            pendingPermissionBanner
            ChatContentView(
                sessionId: target.sessionId,
                viewModel: viewModel,
                terminalLook: terminal,
                codeFolder: folder,
                onLeaveTerminal: onLeaveTerminal,
                onEnterTerminal: onEnterTerminal
            )
        }
        .background(terminal ? TerminalPalette.background : Color("BgPrimary"))
        .task {
            // The launch prompt's text: send it once the chat has set its
            // session context (its onAppear runs before this task).
            guard let prompt = target.initialPrompt, !didSendInitialPrompt else { return }
            didSendInitialPrompt = true
            await Task.yield()
            viewModel.sendMessage(prompt, runtimeModelId: modelStore.selectedModelId, effort: modelStore.effortForSend)
        }
        .onChange(of: viewModel.currentSessionId) { _, id in
            onSessionChanged(id)
        }
        .task(id: sessionId) {
            await pollPendingPermissions()
        }
        .sheet(item: $reviewingPermission) { request in
            ChangesetReviewSheet(request: request) { resolved in
                pendingPermissions.removeAll { $0.id == resolved.id }
            }
        }
    }

    /// Title ⌄ (the session menu) on the left, the folder · branch chip on
    /// the right; 6pt × 14pt padding, a hairline under it.
    private var sessionHeader: some View {
        HStack(spacing: 8) {
            Menu {
                Button(action: onRename) { Label("Rename", systemImage: "pencil") }
                if let sessionId {
                    Button(action: { UIPasteboard.general.string = sessionId }) {
                        Label("Copy session ID", systemImage: "doc.on.doc")
                    }
                }
                Divider()
                Button(role: .destructive, action: onArchive) { Label("Archive", systemImage: "archivebox") }
            } label: {
                HStack(spacing: 6) {
                    Text(displayTitle)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextPrimary").opacity(0.6))
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 4)
            }
            .disabled(sessionId == nil)
            Spacer(minLength: 8)
            if let folder {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.triangle.branch")
                        .font(.system(size: 11))
                    Text(folder.branch.map { "\(folder.name) · \($0)" } ?? folder.name)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .font(.system(size: 11, weight: .bold))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Color("FillNeutral"))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                .frame(maxWidth: UIScreen.main.bounds.width * 0.48, alignment: .trailing)
                .accessibilityLabel("\(folder.directory) · \(folder.branch ?? "")")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    /// The terminal view's top row: the thread's name (its session menu) on
    /// the dark strip, and Shell — the computer's raw shell — on the right.
    private var terminalHeader: some View {
        HStack(spacing: 8) {
            Menu {
                Button(action: onRename) { Label("Rename", systemImage: "pencil") }
                if let sessionId {
                    Button(action: { UIPasteboard.general.string = sessionId }) {
                        Label("Copy session ID", systemImage: "doc.on.doc")
                    }
                }
                Divider()
                Button(role: .destructive, action: onArchive) { Label("Archive", systemImage: "archivebox") }
            } label: {
                HStack(spacing: 6) {
                    Text(displayTitle)
                        .font(.system(size: 14, weight: .semibold, design: .monospaced))
                        .foregroundColor(TerminalPalette.foreground)
                        .lineLimit(1)
                    if sessionId != nil {
                        Image(systemName: "chevron.down")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(TerminalPalette.secondary)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 4)
            }
            .disabled(sessionId == nil)
            Spacer(minLength: 8)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .background(TerminalPalette.strip)
        .overlay(alignment: .bottom) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
    }

    /// A pending file edit (or other approval) for this session; tapping
    /// opens ChangesetReviewSheet for the oldest one.
    @ViewBuilder
    private var pendingPermissionBanner: some View {
        if let oldest = pendingPermissions.first {
            Button {
                reviewingPermission = oldest
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.shield")
                        .foregroundColor(Theme.statusWarning)
                    Text(pendingPermissions.count == 1
                         ? "1 change awaiting review"
                         : "\(pendingPermissions.count) changes awaiting review")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("BgPanel"))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    /// `GET /v1/permission` for this session, then the permission event
    /// stream; retried every 2s after a disconnect. A thread with no session
    /// yet has nothing to filter on.
    private func pollPendingPermissions() async {
        guard let sessionId else { return }
        while !Task.isCancelled {
            if let all = try? await PermissionClient.shared.listPending() {
                pendingPermissions = all
                    .filter { $0.sessionID == sessionId }
                    .sorted { $0.id < $1.id }
            }
            do {
                for try await event in PermissionClient.shared.subscribeToEvents() {
                    try Task.checkCancellation()
                    switch event {
                    case .asked(let request):
                        if request.sessionID == sessionId, !pendingPermissions.contains(where: { $0.id == request.id }) {
                            pendingPermissions.append(request)
                            pendingPermissions.sort { $0.id < $1.id }
                        }
                    case .replied(let info):
                        if info.sessionID == sessionId {
                            pendingPermissions.removeAll { $0.id == info.requestID }
                        }
                    case .ignored:
                        break
                    }
                }
            } catch {
                // Stream disconnected; retry below.
            }
            try? await Task.sleep(for: .seconds(2))
        }
    }
}
