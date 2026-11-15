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

/// A code thread on the phone: the pending-changes banner, then the shared
/// chat feed and the Code composer. The session's name (its menu: rename,
/// copy ID, archive), the panes and the sessions button sit in the Code
/// header (CodeModeView).
struct CodeThreadChatView: View {
    let target: CodeThreadTarget
    let folder: CodeFolderInfo?
    /// The backend session this thread is on (nil until the first send).
    let onSessionChanged: (String?) -> Void
    /// The Terminal choice: this thread drawn as a terminal session.
    var terminal: Bool = false
    /// The composer switch's "Chat": back to the chat feed.
    var onLeaveTerminal: () -> Void = {}
    /// The composer switch's "Terminal": this thread in its terminal look.
    var onEnterTerminal: () -> Void = {}
    /// Hidden behind the open shell: the permission event stream (the
    /// computer's whole bus) stops so it doesn't compete with keystrokes.
    var paused: Bool = false

    @StateObject private var viewModel = ChatViewModel()
    @StateObject private var modelStore = ModelStore.shared
    @State private var didSendInitialPrompt = false

    /// Pending gizzi-code approval requests for this thread's session
    /// (`GET /v1/permission` + the permission event stream), oldest first so
    /// the banner always opens the longest-waiting request.
    @State private var pendingPermissions: [PermissionRequest] = []
    @State private var reviewingPermission: PermissionRequest? = nil

    private var sessionId: String? { viewModel.currentSessionId ?? target.sessionId }
    /// Code mode draws a stage background behind the thread.
    @Environment(\.backdropUnderneath) private var backdropUnderneath

    var body: some View {
        VStack(spacing: 0) {
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
        .background(backdropUnderneath ? Color.clear : (terminal ? TerminalPalette.background : Color("BgPrimary")))
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
        .task(id: paused ? nil : sessionId) {
            guard !paused else { return }
            await pollPendingPermissions()
        }
        .sheet(item: $reviewingPermission) { request in
            ChangesetReviewSheet(request: request) { resolved in
                pendingPermissions.removeAll { $0.id == resolved.id }
            }
        }
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
