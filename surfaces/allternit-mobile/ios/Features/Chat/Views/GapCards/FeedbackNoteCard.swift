import SwiftUI

/// Feedback note card: a title row and one line of metadata. Stub: G4-G7 fill it in.
struct FeedbackNoteCard: View {
    let gap: GapToolMetadata

    private var line: String {
        [gap.kind, (gap.noteId == nil ? nil : "Saved on this device")].compactMap { $0 }.joined(separator: " \u{00B7} ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "note.text")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Feedback note")
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
