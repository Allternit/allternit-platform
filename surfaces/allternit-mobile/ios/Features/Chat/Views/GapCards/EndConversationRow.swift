import SwiftUI

/// Conversation ended card: the agent ended this chat.
struct EndConversationRow: View {
    let gap: GapToolMetadata

    private var line: String {
        gap.reason.map { "This chat was ended. \($0)" } ?? "This chat was ended."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "xmark.bubble")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Conversation ended")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))
            if !line.isEmpty {
                Text(line)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
    }
}
