import SwiftUI

/// Second opinion card: the verdict badge, then the reasons.
struct SecondOpinionCard: View {
    let gap: GapToolMetadata

    private var verdictColor: Color {
        switch gap.verdict {
        case "agree": return .green
        case "disagree": return .red
        default: return .orange
        }
    }

    private var verdictText: String? {
        gap.verdict.map { $0.prefix(1).uppercased() + $0.dropFirst() }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "checkmark.seal")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Second opinion")
                    .foregroundColor(Color("TextSecondary"))
                if let kind = gap.kind {
                    Text(kind == "plan" ? "on plan" : "on diff")
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 0)
                if let verdictText {
                    Text(verdictText)
                        .foregroundColor(verdictColor)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(verdictColor.opacity(0.15)))
                }
            }
            .font(.caption.weight(.semibold))
            if let reasons = gap.reasons, !reasons.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(Array(reasons.prefix(5).enumerated()), id: \.offset) { _, reason in
                        HStack(alignment: .top, spacing: 6) {
                            Text("\u{2022}")
                            Text(reason).lineLimit(3)
                        }
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                    }
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
