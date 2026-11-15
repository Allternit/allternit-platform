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
                if ask.permission.lowercased() == "run_code" {
                    RunCodeScript(code: ask.code ?? "", files: ask.files)
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

/// The script a run_code ask wants to run: the attached file names, then the
/// code in monospace, scrollable, collapsed after 15 lines ("Show all").
private struct RunCodeScript: View {
    let code: String
    let files: [String]

    private static let collapsedLines = 15
    @State private var expanded = false

    private var lines: [Substring] { code.split(separator: "\n", omittingEmptySubsequences: false) }
    private var isLong: Bool { lines.count > Self.collapsedLines }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !files.isEmpty {
                Text("Files: " + files.joined(separator: ", "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            if !code.isEmpty {
                ScrollView([.horizontal, .vertical]) {
                    Text(isLong && !expanded ? lines.prefix(Self.collapsedLines).joined(separator: "\n") : code)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: true, vertical: true)
                        .padding(8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: expanded ? 360 : 260)
                .background(Color("BgPrimary"))
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                .accessibilityLabel("Script to run")
                if isLong {
                    Button(expanded ? "Show less" : "Show all (\(lines.count) lines)") {
                        withAnimation(.easeInOut(duration: 0.15)) { expanded.toggle() }
                    }
                    .font(.caption.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
                }
            }
        }
    }
}
