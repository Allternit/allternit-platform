import SwiftUI

/// Worktree card: a title row and one line of metadata. Stub: G4-G7 fill it in.
struct WorktreeCard: View {
    let gap: GapToolMetadata

    private var line: String {
        [gap.action ?? (gap.result == nil ? "enter" : nil), gap.result, gap.branch, gap.path.map { ($0 as NSString).lastPathComponent }].compactMap { $0 }.joined(separator: " \u{00B7} ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Worktree")
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
