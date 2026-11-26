import SwiftUI
import MarkdownView

/// Settings → Response style → "Try this style" (NEW-FEATURES-SPEC Feature A).
///
/// One real model turn answered in the style and custom instructions shown on
/// screen, saved or not. It goes through the chat's own stream
/// (`AgentChatClient.sendMessageStream`) with `stylePreview` set. Nothing is
/// saved and no chat is listed. The runtime proves it honoured the preview by
/// echoing `stylePreview: true` on the first frame; with no echo (an older
/// runtime) the card hides itself. Each send replaces the last answer.
struct StylePreviewCard: View {
    /// The style on screen (the saved one: a tap on a style row saves at once).
    let style: ResponseStyle
    /// The saved style, for the "not saved" line.
    let savedStyle: ResponseStyle
    /// The custom instructions text on screen, trimmed or not.
    let instructions: String
    /// The saved custom instructions.
    let savedInstructions: String

    @ObservedObject private var auth = AuthManager.shared
    @ReducedMotion private var reduceMotion

    @State private var question = ""
    @State private var answer = ""
    @State private var phase: Phase = .idle
    @State private var errorText: String? = nil
    /// The style and instructions the shown answer was made with.
    @State private var answered: Snapshot? = nil
    /// No echo frame: the runtime cannot preview, so the card is gone.
    @State private var unsupported = false
    /// The runtime's answer to `GET agent-chat/style-preview`; the card shows only on `supported`.
    @State private var probed = false
    /// Previews left today; nil when the runtime did not say.
    @State private var remaining: Int? = nil
    @State private var task: Task<Void, Never>? = nil
    @FocusState private var focused: Bool

    private enum Phase: Equatable { case idle, streaming, done, failed }

    private struct Snapshot: Equatable {
        let style: ResponseStyle
        let instructions: String
    }

    private var current: Snapshot {
        Snapshot(style: style, instructions: instructions.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    private var signedIn: Bool { auth.isSignedIn }
    private var trimmedQuestion: String { question.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var outOfPreviews: Bool { signedIn && remaining == 0 }
    private var canSend: Bool { signedIn && !outOfPreviews && !trimmedQuestion.isEmpty }
    /// An answer exists for a style that is no longer the one on screen.
    private var isStale: Bool { phase == .done && answered != nil && answered != current }
    private var isUnsaved: Bool {
        style != savedStyle || current.instructions != savedInstructions.trimmingCharacters(in: .whitespacesAndNewlines)
    }
    private var statusLine: String {
        isUnsaved ? "Preview of \(style.label). Not saved yet." : "Preview of your saved style."
    }

    var body: some View {
        if !unsupported && (probed || !signedIn) {
            SettingsCardBox {
                Text("Try this style")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Ask a question and see the reply in the style above, before you save. Counts as one chat turn.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)

                inputRow

                if !signedIn {
                    Button { AuthManager.shared.signIn() } label: {
                        Text("Sign in to try a style")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.plain)
                } else {
                    if !answer.isEmpty || phase == .streaming {
                        answerView
                    }
                    if outOfPreviews {
                        Text(ChatErrorInfo.make(message: nil, status: 429).reason)
                            .font(.system(size: 14))
                            .foregroundColor(Theme.statusError)
                    }
                    if let errorText {
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: "exclamationmark.circle")
                                .font(.system(size: 15))
                            Text(errorText)
                                .font(.system(size: 14))
                                .fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                        .foregroundColor(Theme.statusError)
                        .accessibilityElement(children: .combine)
                    }
                    if phase == .failed {
                        actionButton("Retry", systemImage: "arrow.clockwise")
                    } else if isStale {
                        actionButton("Run again", systemImage: "arrow.clockwise")
                    }
                    Text(statusLine)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: phase)
            .onDisappear { task?.cancel() }
            .task(id: signedIn) { if !probed { await probe() } }
        }
        // Not shown, but it still has to probe: the card appears only once the runtime says it can.
        else if !unsupported {
            Color.clear.frame(height: 0).task(id: signedIn) { await probe() }
        }
    }

    /// Nothing is sent unless this returns `supported: true`. A 404 or any error hides the card.
    @MainActor
    private func probe() async {
        guard signedIn else { return }
        do {
            let status = try await AgentChatClient().stylePreviewStatus()
            if status.supported {
                remaining = status.remaining
                probed = true
            } else {
                unsupported = true
            }
        } catch is CancellationError {
            return
        } catch {
            unsupported = true
        }
    }

    private var inputRow: some View {
        HStack(spacing: 4) {
            TextField("Ask anything", text: $question)
                .font(.system(size: 16))
                .textFieldStyle(.plain)
                .submitLabel(.send)
                .focused($focused)
                .onSubmit { if canSend, phase != .streaming { send() } }
                .disabled(!signedIn)
                .padding(.leading, 12)
                .accessibilityLabel("Question to try")
            Button {
                if phase == .streaming { stop() } else { send() }
            } label: {
                Image(systemName: phase == .streaming ? "stop.fill" : "arrow.up")
                    .font(.system(size: phase == .streaming ? 14 : 16, weight: .semibold))
                    .foregroundColor(Color("OnAccent"))
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(Color("AccentPrimary").opacity(phase == .streaming || canSend ? 1 : 0.35)))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(phase != .streaming && !canSend)
            .accessibilityLabel(phase == .streaming ? "Stop" : "Send")
        }
        .frame(minHeight: 44)
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(Color("BorderStrong"), lineWidth: 1)
        )
        .opacity(signedIn ? 1 : 0.5)
    }

    private var answerView: some View {
        MarkdownView(answer.isEmpty ? " " : answer)
            .frame(maxWidth: .infinity, alignment: .leading)
            .opacity(isStale ? 0.5 : 1)
            .accessibilityLabel(isStale ? "Previous preview answer" : "Preview answer")
    }

    private func actionButton(_ title: String, systemImage: String) -> some View {
        Button { send() } label: {
            Label(title, systemImage: systemImage)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, minHeight: 44)
        }
        .buttonStyle(.plain)
        .disabled(!canSend)
    }

    private func stop() {
        task?.cancel()
        phase = answer.isEmpty ? .idle : .done
    }

    private func send() {
        guard canSend else { return }
        focused = false
        task?.cancel()
        let text = trimmedQuestion
        let snapshot = current
        answer = ""
        errorText = nil
        answered = snapshot
        phase = .streaming
        task = Task { await run(text: text, snapshot: snapshot) }
    }

    @MainActor
    private func run(text: String, snapshot: Snapshot) async {
        // A throwaway id: the server never lists or keeps it.
        let sessionId = "style-preview-\(UUID().uuidString.lowercased())"
        let preview = AgentChatClient.StylePreview(
            response_style: snapshot.style.rawValue,
            custom_instructions: snapshot.instructions
        )
        var echoed = false
        do {
            let stream = AgentChatClient().sendMessageStream(sessionId: sessionId, text: text, stylePreview: preview)
            for try await event in stream {
                try Task.checkCancellation()
                switch event {
                case .messageStart(let start):
                    if !echoed {
                        guard start.stylePreview else { return markUnsupported() }
                        echoed = true
                        if let left = remaining { remaining = max(0, left - 1) }
                    }
                case .textDelta(let delta):
                    guard echoed else { return markUnsupported() }
                    answer += delta.text
                case .streamErrorFrame(let frame):
                    return fail(frame.info.reason)
                case .streamError(let message):
                    return fail(ChatErrorInfo.make(message: message).reason)
                case .finish(let finish):
                    guard echoed else { return markUnsupported() }
                    if finish.status == "error" || finish.metadata?.status == "error" {
                        let raw = finish.metadata?.error ?? finish.metadata?.errorDetails?.message
                        return fail(ChatErrorInfo.make(message: raw.map(FriendlyErrorMessage.from)).reason)
                    }
                default:
                    break
                }
            }
            guard echoed else { return markUnsupported() }
            phase = answer.isEmpty ? .idle : .done
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            if case APIError.httpError(429, _) = error { remaining = 0 }
            fail(Self.message(for: error))
        }
    }

    @MainActor
    private func markUnsupported() {
        task?.cancel()
        answer = ""
        answered = nil
        phase = .idle
        unsupported = true
    }

    @MainActor
    private func fail(_ text: String) {
        answer = ""
        answered = nil
        errorText = text
        phase = .failed
        Haptics.error()
    }

    /// The words the chat shows for the same failure.
    private static func message(for error: Error) -> String {
        if let chat = error as? AgentChatClientError {
            return chat.errorDescription ?? "Something went wrong. Try again."
        }
        if case APIError.httpError(let status, let message) = error {
            return ChatErrorInfo.make(message: message.map(FriendlyErrorMessage.from), status: status).reason
        }
        return FriendlyErrorMessage.from(error)
    }
}
