import SwiftUI

/// An artifact attached from the deck's Artifacts menu. It shows as a
/// removable chip in the composer; on Send the message carries one text line
/// (web artifact-launch.store.ts `attachedArtifactLine`), so the agent knows
/// which artifact is meant. One attachment at a time, like the web.
struct AttachedArtifact: Equatable, Sendable {
    let id: String
    let title: String
    let kind: String?

    init(summary: ArtifactV2Summary) {
        id = summary.id
        title = summary.title
        kind = summary.kind
    }

    /// `[Attached artifact: <title> → artifact <id> (<kind>).]`
    var line: String {
        "[Attached artifact: \(title) → artifact \(id)\(kind.map { " (\($0))" } ?? "").]"
    }

    /// The message text with the line after the typed text and a blank line.
    func message(typed: String) -> String {
        let text = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? line : "\(text)\n\n\(line)"
    }
}

@MainActor
final class AttachedArtifactStore: ObservableObject {
    static let shared = AttachedArtifactStore()
    @Published private(set) var attached: AttachedArtifact?

    func attach(_ artifact: AttachedArtifact) { attached = artifact }
    func detach() { attached = nil }

    /// The text to send, and clears the chip: the attachment goes out once.
    func consume(typed: String) -> String {
        guard let attached else { return typed }
        self.attached = nil
        return attached.message(typed: typed)
    }
}

/// The composer chip: artifact icon, title, ✕.
struct AttachedArtifactChip: View {
    let artifact: AttachedArtifact
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Image(ArtifactV2Kind.iconAsset(for: artifact.kind ?? "doc"))
                .resizable().scaledToFit().frame(width: 16, height: 16)
            Text(artifact.title).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            Button(action: onRemove) {
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                    .frame(width: 22, height: 22)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove attached artifact \(artifact.title)")
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.leading, 10)
        .padding(.trailing, 4)
        .padding(.vertical, 3)
        .background(Color("FillNeutral"), in: Capsule())
        .overlay(Capsule().stroke(Color("BorderSubtle")))
    }
}
