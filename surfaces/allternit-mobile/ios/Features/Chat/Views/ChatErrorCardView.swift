import SwiftUI

/// A structured, renderable version of a stream failure — replaces the old
/// behavior of appending the backend's raw error string ("gizzi message
/// failed (400 Bad Request): {…json…}") into the message text.
///
/// Built from the agent-chat `finish` frame's `metadata.errorDetails` (the
/// runtime's structured error, passed through by v1_routes.rs).
struct ChatError: Equatable, Sendable {
    /// Runtime error name when known (e.g. "ProviderModelNotFoundError").
    var name: String?
    /// Short human title ("Model not available").
    var title: String
    /// One or two sentences of guidance.
    var detail: String
    /// The full raw payload, shown behind the disclosure row for debugging.
    var raw: String
    /// True for provider/model resolution failures — the card's primary
    /// action becomes "Choose a model" instead of "Retry".
    var isModelError: Bool

    /// Builds a card from the finish frame's metadata fields.
    init(name: String?, message: String?, raw: String) {
        let isNoBrain = name == "no_brain"
        let isModelError = name == "ProviderModelNotFoundError" || isNoBrain
        self.name = name
        self.isModelError = isModelError
        self.raw = raw

        if isNoBrain {
            self.title = AgentChatClientError.noBrainMessage
            self.detail = "Choose the brain this chat runs on, then send again."
        } else if isModelError {
            self.title = "Model not available"
            self.detail = "The model for this chat couldn't be found. Pick another model and try again."
        } else {
            self.title = "Something went wrong"
            let trimmed = message?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            self.detail = trimmed.isEmpty ? "The response failed before it could complete." : trimmed
        }
    }

    /// Transport/creation failures (no runtime payload to parse).
    static func connectionInterrupted(_ reason: String) -> ChatError {
        ChatError(
            name: nil,
            message: reason,
            raw: reason
        )
    }
}

/// Inline error card rendered under an assistant message (Claude-style
/// status card instead of raw JSON in the bubble). Primary action is
/// contextual: model errors offer "Choose a model", everything else offers
/// "Retry". The raw payload lives behind a disclosure row.
struct ChatErrorCardView: View {
    let error: ChatError
    let onChooseModel: () -> Void
    let onRetry: () -> Void

    @State private var showRaw = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundColor(Theme.statusWarning)
                Text(error.title)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
            }

            Text(error.detail)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 10) {
                if error.isModelError {
                    actionButton(title: "Choose a model", systemImage: "cpu", action: onChooseModel)
                }
                actionButton(title: "Retry", systemImage: "arrow.clockwise",
                             primary: !error.isModelError, action: onRetry)
            }

            Button(action: {
                withAnimation(.easeInOut(duration: 0.2)) { showRaw.toggle() }
            }) {
                HStack(spacing: 4) {
                    Text("Details")
                        .font(.caption2)
                    Image(systemName: showRaw ? "chevron.up" : "chevron.down")
                        .font(.system(size: 8, weight: .bold))
                }
                .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)

            if showRaw {
                Text(error.raw)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: 320, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Theme.statusWarning.opacity(0.4), lineWidth: 1)
        )
    }

    private func actionButton(title: String, systemImage: String, primary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            HStack(spacing: 5) {
                Image(systemName: systemImage)
                    .font(.system(size: 10, weight: .semibold))
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
            }
            .foregroundColor(primary ? .black : Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(height: 30)
            .background(primary ? Color("AccentPrimary") : Color("BgSecondary"))
            .clipShape(Capsule())
            .overlay(
                Capsule().stroke(primary ? .clear : Theme.borderWarmDefault, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}
