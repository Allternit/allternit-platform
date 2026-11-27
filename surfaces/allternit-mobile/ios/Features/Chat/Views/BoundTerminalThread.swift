import SwiftUI

/// A chat thread bound to a CLI session on a picked computer (Start anywhere,
/// plan D2 terminal kind): the Code terminal and its agent composer
/// (`PhoneCodeTerminal` → `AgentComposerBar`), attached to the terminal the
/// start opened. Nothing new starts on the computer; a failed attach says so
/// and offers a retry.
struct BoundTerminalThread: View {
    let binding: StartBinding

    @StateObject private var store = CodeTerminalStore()
    /// nil while attaching; then whether the terminal was reached.
    @State private var attached: Bool? = nil

    private var title: String {
        RuntimeRegistryStore.shared.runtime(binding.runtime)?.name ?? binding.runtime.capitalized
    }

    private var folderName: String {
        binding.folder.split(separator: "/").last.map(String.init) ?? binding.folder
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "apple.terminal")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
                Text("\(title) on \(binding.deviceName)")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(folderName)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 36)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("boundTerminal-header")

            switch attached {
            case .none:
                VStack(spacing: 10) {
                    ProgressView()
                    Text("Connecting to \(binding.deviceName)…")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .some(true):
                // Attached first, so the terminal's own first-open does not start a new shell.
                PhoneCodeTerminal(store: store, shellOnly: true)
            case .some(false):
                VStack(spacing: 12) {
                    Text("Couldn't reach \(binding.deviceName). The session keeps running there.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Try again") {
                        Haptics.light()
                        Task { await attach() }
                    }
                    .font(.system(size: 14, weight: .semibold))
                    .frame(minHeight: 44)
                }
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .accessibilityIdentifier("boundTerminal-error")
            }
        }
        .task(id: binding.terminalId) { await attach() }
    }

    private func attach() async {
        guard let terminalId = binding.terminalId else {
            attached = false
            return
        }
        attached = nil
        // Addressed to the bound computer directly; the app's relay choice stays.
        attached = await store.attach(sessionId: terminalId, title: title, runtimeId: binding.runtimeId)
    }
}

/// Approvals for a chat bound to a Gizzi engine session (Start anywhere,
/// same as Android's `BoundEngine`): `GET /v1/permission` on the bound
/// computer through the relay, filtered to `binding.sessionId`, shown with
/// Code's approval banner and review sheet, and answered with
/// `POST /v1/permission/:id/reply` on that computer. Read when the thread
/// opens and again on each permission event from its event stream.
/// Questions from the same stream go to `AgentAskStore`, which reads and
/// answers them on the bound computer and shows them as the thread's usual
/// question cards. Decisions keeps serving every other chat and skips these.
struct BoundEngineApprovals: View {
    let binding: StartBinding

    @State private var pending: [PermissionRequest] = []
    @State private var reviewing: PermissionRequest? = nil

    private var client: PermissionClient {
        PermissionClient(baseURL: RuntimeRelayClient.relayBaseURL(runtimeId: binding.runtimeId))
    }

    var body: some View {
        Group {
            if let oldest = pending.first {
                Button {
                    Haptics.light()
                    reviewing = oldest
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "checkmark.shield")
                            .foregroundColor(Theme.statusWarning)
                        Text(pending.count == 1
                             ? "1 request on \(binding.deviceName) awaiting review"
                             : "\(pending.count) requests on \(binding.deviceName) awaiting review")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(2)
                        Spacer(minLength: 8)
                        Image(systemName: "chevron.right")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .frame(minHeight: 44)
                    .background(Color("BgPanel"))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("boundEngine-approvals")
            }
        }
        .task(id: "\(binding.runtimeId)/\(binding.sessionId ?? "")") { await follow() }
        .sheet(item: $reviewing) { request in
            ChangesetReviewSheet(request: request, onResolved: { resolved in
                pending.removeAll { $0.id == resolved.id }
            }, client: client)
        }
    }

    /// Reads the pending list, then follows the permission events; after a
    /// disconnect it reads again and resubscribes every 2 s.
    private func follow() async {
        guard let sessionId = binding.sessionId else { return }
        let client = self.client
        while !Task.isCancelled {
            if let all = try? await client.listPending() {
                pending = all.filter { $0.sessionID == sessionId }.sorted { $0.id < $1.id }
            }
            await AgentAskStore.shared.refresh(sessionId: sessionId)
            do {
                for try await event in client.subscribeToEvents() {
                    try Task.checkCancellation()
                    switch event {
                    case .asked(let request):
                        if request.sessionID == sessionId, !pending.contains(where: { $0.id == request.id }) {
                            pending.append(request)
                            pending.sort { $0.id < $1.id }
                        }
                    case .replied(let info):
                        if info.sessionID == sessionId {
                            pending.removeAll { $0.id == info.requestID }
                        }
                    case .questionAsked(let request):
                        if request.sessionID == sessionId { AgentAskStore.shared.questionAsked(request) }
                    case .questionClosed(let closedSession, let requestID):
                        if closedSession == sessionId { AgentAskStore.shared.questionClosed(id: requestID) }
                    case .ignored:
                        break
                    }
                }
            } catch {
                // Stream dropped (the computer slept or the relay closed); read again below.
            }
            try? await Task.sleep(for: .seconds(2))
        }
    }
}
