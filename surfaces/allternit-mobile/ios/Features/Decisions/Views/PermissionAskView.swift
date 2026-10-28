import SwiftUI

/// Deny / Always / Allow for a permission the agent waits on (web
/// ProcessBlock inline approval). `compact` draws just the buttons, for a
/// tool row on the rail; otherwise a small card with what is being asked.
struct PermissionAskView: View {
    let ask: AgentPermissionAsk
    var compact = false

    @ObservedObject private var store = AgentAskStore.shared
    @State private var busy: String? = nil
    @State private var error: String? = nil

    var body: some View {
        if store.isResolved(ask.id) {
            EmptyView()
        } else if compact {
            VStack(alignment: .leading, spacing: 4) {
                buttons
                errorLine
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "hand.raised.fill")
                        .font(.caption)
                    Text("Waiting for approval")
                        .font(.caption.weight(.semibold))
                }
                .foregroundColor(Color("TextSecondary"))
                Text(ask.actionLabel)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let detail = ask.detail {
                    Text(detail)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(3)
                }
                buttons
                errorLine
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
        }
    }

    private var buttons: some View {
        HStack(spacing: 6) {
            button("Deny", reply: "reject", kind: .quiet)
            button("Always", reply: "always", kind: .quiet)
            button("Allow", reply: "once", kind: .primary)
        }
    }

    @ViewBuilder
    private var errorLine: some View {
        if let error {
            Text(error)
                .font(.caption)
                .foregroundColor(.red)
        }
    }

    private func button(_ title: String, reply: String, kind: QuestionCardButtonStyle.Kind) -> some View {
        Button {
            send(reply)
        } label: {
            if busy == reply {
                ProgressView().controlSize(.small)
            } else {
                Text(title)
            }
        }
        .buttonStyle(QuestionCardButtonStyle(kind: kind))
        .disabled(busy != nil)
        .accessibilityLabel("\(title): \(ask.actionLabel)")
    }

    private func send(_ reply: String) {
        busy = reply
        error = nil
        Task {
            do {
                try await store.reply(ask, reply: reply)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            } catch {
                self.error = "Couldn't send that. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
            busy = nil
        }
    }
}
