import SwiftUI
import UIKit

/// What a bot's message tool returned, parsed from the tool result text.
enum MessageToolResult: Equatable, Sendable {
    case draft(MessageDraft)
    /// `messages_send` was held for approval in the Needs-you inbox.
    case held(inboxId: String)

    struct MessageDraft: Equatable, Sendable {
        var to: String
        var toName: String?
        var text: String
    }

    static func parse(toolName: String, output: String?) -> MessageToolResult? {
        guard let output, let data = output.data(using: .utf8),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        let name = toolName.lowercased()
        if name == "message_draft" || (json["type"] as? String) == "message_draft",
           let to = json["to"] as? String, let text = json["text"] as? String, !to.isEmpty {
            let toName = (json["toName"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            return .draft(MessageDraft(to: to, toName: toName, text: text))
        }
        if json["held"] as? Bool == true, let id = json["inboxId"] as? String, !id.isEmpty {
            return .held(inboxId: id)
        }
        return nil
    }
}

/// A text the bot prepared for the owner. It never sends by itself: the
/// button opens the system Messages sheet.
struct MessageDraftCard: View {
    let draft: MessageToolResult.MessageDraft
    @State private var sent = false
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "message").foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(draft.toName ?? draft.to).font(.subheadline.weight(.semibold))
                    if draft.toName != nil {
                        Text(draft.to).font(.caption).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 0)
            }
            ScrollView {
                Text(draft.text)
                    .font(.callout)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 180)
            HStack(spacing: 10) {
                Button {
                    MessageComposer.present(to: draft.to, text: draft.text) { outcome in
                        if outcome == .sent { sent = true }
                    }
                } label: {
                    Label(sent ? "Sent" : "Send with Messages", systemImage: sent ? "checkmark" : "paperplane")
                        .font(.subheadline.weight(.semibold))
                }
                .buttonStyle(.borderedProminent)
                .disabled(sent)
                .accessibilityLabel(sent ? "Sent" : "Send with Messages to \(draft.toName ?? draft.to)")
                Button {
                    UIPasteboard.general.string = draft.text
                    copied = true
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .font(.subheadline)
                }
                .buttonStyle(.bordered)
                .accessibilityLabel("Copy message text")
            }
        }
        .padding(12)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .contain)
    }
}

/// One line for a `messages_send` that waits for the owner's approval.
struct MessageHeldLine: View {
    let inboxId: String
    @Environment(\.openURL) private var openURL

    var body: some View {
        Button {
            if let url = URL(string: "allternit://inbox/\(inboxId)") { openURL(url) }
        } label: {
            Label("Waiting for your approval", systemImage: "hourglass")
                .font(.footnote)
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .accessibilityHint("Opens the request in your inbox")
    }
}
