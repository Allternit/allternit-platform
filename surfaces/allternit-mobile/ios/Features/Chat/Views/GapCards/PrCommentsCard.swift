import SwiftUI

/// PR comments card: a comments list (author, file:line, short text).
struct PrCommentsCard: View {
    let gap: GapToolMetadata

    private var summary: String {
        let target = gap.pr.map { "#\($0)" } ?? ""
        if gap.action == "reply" { return target.isEmpty ? "Replied" : "Replied on \(target)" }
        let count = gap.commentCount ?? 0
        let noun = count == 1 ? "comment" : "comments"
        return target.isEmpty ? "\(count) \(noun)" : "\(count) \(noun) on \(target)"
    }

    private func place(_ c: PrCommentItem) -> String? {
        guard let path = c.path, !path.isEmpty else { return nil }
        let file = (path as NSString).lastPathComponent
        return c.line.map { "\(file):\($0)" } ?? file
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "text.bubble")
                    .foregroundColor(Color("AccentPrimary"))
                Text("PR comments")
                    .foregroundColor(Color("TextSecondary"))
                Spacer(minLength: 0)
                Text(summary)
                    .foregroundColor(Color("TextPrimary"))
            }
            .font(.caption.weight(.semibold))
            if gap.action != "reply", let comments = gap.comments, !comments.isEmpty {
                ForEach(Array(comments.prefix(6).enumerated()), id: \.offset) { _, c in
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 6) {
                            Text(c.author ?? "unknown")
                                .font(.caption.weight(.semibold))
                                .foregroundColor(Color("TextPrimary"))
                            if let place = place(c) {
                                Text(place)
                                    .font(.caption.monospaced())
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(1)
                                    .truncationMode(.head)
                            }
                        }
                        if let body = c.body, !body.isEmpty {
                            Text(body)
                                .font(.footnote)
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(2)
                        }
                    }
                }
                if comments.count > 6 {
                    Text("+\(comments.count - 6) more")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
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
