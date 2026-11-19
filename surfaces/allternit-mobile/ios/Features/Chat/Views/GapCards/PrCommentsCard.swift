import SwiftUI

/// PR comments card: a title row and one line of metadata. Stub: G4-G7 fill it in.
struct PrCommentsCard: View {
    let gap: GapToolMetadata

    private var line: String {
        let target = gap.pr.map { "#\($0)" } ?? ""
        if gap.action == "reply" { return target.isEmpty ? "Replied" : "Replied on \(target)" }
        let count = gap.commentCount ?? 0
        let noun = count == 1 ? "comment" : "comments"
        return target.isEmpty ? "\(count) \(noun)" : "\(count) \(noun) on \(target)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "text.bubble")
                    .foregroundColor(Color("AccentPrimary"))
                Text("PR comments")
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
