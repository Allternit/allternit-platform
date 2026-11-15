import SwiftUI
import UIKit

/// Quick task (allternit-ai src/shell/QuickTaskStack.tsx): a small composer
/// that starts a chat without leaving the current screen. Sending is real —
/// it creates a chat session (`POST /api/v1/agent-sessions`) and streams the
/// prompt into it; the sheet then offers Open to jump to that chat. The run
/// keeps going after the sheet closes (QuickTaskRunner owns it).
struct QuickTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var runner = QuickTaskRunner.shared
    @State private var text = ""
    @State private var runId: UUID? = nil
    @FocusState private var focused: Bool

    private var run: QuickTaskRunner.Run? {
        runId.flatMap { id in runner.runs.first { $0.id == id } }
    }

    private var canSend: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && run == nil
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                if let run {
                    statusView(run)
                } else {
                    TextField("Describe a task…", text: $text, axis: .vertical)
                        .lineLimit(3...8)
                        .focused($focused)
                        .padding(14)
                        .background(RoundedRectangle(cornerRadius: 14).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .accessibilityIdentifier("quickTaskInput")
                    Text("Starts a new chat in the background. You can keep doing what you were doing.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 0)
            }
            .padding(16)
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Quick task")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button(run == nil ? "Cancel" : "Close") { dismiss() }
                }
                if run == nil {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Start") {
                            UIImpactFeedbackGenerator(style: .light).impactOccurred()
                            runId = runner.start(text: text)
                        }
                        .fontWeight(.semibold)
                        .disabled(!canSend)
                        .accessibilityIdentifier("quickTaskStart")
                    }
                }
            }
            .onAppear { focused = true }
        }
        .presentationDetents([.medium, .large])
    }

    @ViewBuilder
    private func statusView(_ run: QuickTaskRunner.Run) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(run.text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(4)
            HStack(spacing: 8) {
                switch run.status {
                case .starting:
                    ProgressView().controlSize(.small)
                    Text("Starting…").foregroundColor(Color("TextSecondary"))
                case .running:
                    ProgressView().controlSize(.small)
                    Text("Working").foregroundColor(Color("TextSecondary"))
                case .finished:
                    Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                    Text("Done").foregroundColor(Color("TextSecondary"))
                case .failed(let message):
                    Image(systemName: "exclamationmark.triangle.fill").foregroundColor(Theme.statusError)
                    Text(FriendlyErrorMessage.from(message))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(3)
                }
            }
            .font(.footnote)

            HStack(spacing: 10) {
                if let sessionId = run.sessionId {
                    Button {
                        dismiss()
                        NotificationCenter.default.post(name: .openChatSession, object: nil, userInfo: ["sessionId": sessionId])
                    } label: {
                        Text("Open chat")
                            .font(.subheadline.weight(.semibold))
                            .padding(.horizontal, 16)
                            .frame(height: 36)
                            .background(Capsule().fill(Color("TextPrimary")))
                            .foregroundColor(Color("BgPrimary"))
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("quickTaskOpen")
                }
                if case .failed = run.status {
                    Button("Try again") {
                        let retryText = run.text
                        runId = runner.start(text: retryText)
                    }
                    .font(.subheadline.weight(.semibold))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 14).fill(Color("BgPanel")))
    }
}

/// Owns quick-task runs so a stream keeps going after its sheet closes.
@MainActor
final class QuickTaskRunner: ObservableObject {
    static let shared = QuickTaskRunner()

    enum Status: Equatable {
        case starting
        case running
        case finished
        case failed(String)
    }

    struct Run: Identifiable {
        let id: UUID
        let text: String
        var sessionId: String?
        var status: Status
    }

    @Published private(set) var runs: [Run] = []
    private let client = AgentChatClient()

    /// Starts a run; returns its id.
    func start(text: String) -> UUID {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = UUID()
        runs.append(Run(id: id, text: trimmed, sessionId: nil, status: .starting))
        // Keep the list bounded — finished runs only matter to their sheet.
        if runs.count > 10 { runs.removeFirst(runs.count - 10) }

        Task { [weak self] in
            guard let self else { return }
            do {
                let name = String(trimmed.prefix(60))
                let session = try await self.client.createSession(
                    name: name.isEmpty ? "Quick task" : name,
                    originSurface: AppMode.chat.originSurface,
                    sessionMode: "regular",
                    projectId: ProjectStore.shared.selectedProjectId
                )
                self.update(id) { $0.sessionId = session.id; $0.status = .running }
                NotificationCenter.default.post(name: .historyMutated, object: nil)
                for try await event in self.client.sendMessageStream(sessionId: session.id, text: trimmed) {
                    if case .streamError(let message) = event {
                        throw QuickTaskError(message: message)
                    }
                }
                self.update(id) { $0.status = .finished }
            } catch {
                self.update(id) { $0.status = .failed(error.localizedDescription) }
            }
            NotificationCenter.default.post(name: .historyMutated, object: nil)
        }
        return id
    }

    private func update(_ id: UUID, _ change: (inout Run) -> Void) {
        guard let index = runs.firstIndex(where: { $0.id == id }) else { return }
        change(&runs[index])
    }
}

private struct QuickTaskError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}
