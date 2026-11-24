import SwiftUI

/// Feedback notes seen in chat this session. Notes are saved on the computer
/// where the runtime runs and are not read back to the phone; this list is
/// memory only (never saved to disk) and only holds what a chat showed.
@MainActor
final class SeenFeedbackNotes: ObservableObject {
    static let shared = SeenFeedbackNotes()
    @Published private(set) var notes: [GapToolMetadata] = []

    func add(_ gap: GapToolMetadata) {
        guard let id = gap.noteId, !notes.contains(where: { $0.noteId == id }) else { return }
        notes.insert(gap, at: 0)
    }
}

/// Feedback note card. The note stays private: the card says it was saved on
/// the computer, and the summary shows only when the person asks.
struct FeedbackNoteCard: View {
    let gap: GapToolMetadata
    @State private var revealed = false

    private var line: String {
        [gap.kind, (gap.noteId == nil ? nil : "Saved on the computer running this chat")]
            .compactMap { $0 }.joined(separator: " \u{00B7} ")
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
            if let summary = gap.summary, !summary.isEmpty {
                if revealed {
                    Text(summary)
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                }
                Button(revealed ? "Hide note" : "Show note") { revealed.toggle() }
                    .font(.caption.weight(.medium))
                    .foregroundColor(Color("AccentPrimary"))
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
        .accessibilityElement(children: .contain)
    }
}
