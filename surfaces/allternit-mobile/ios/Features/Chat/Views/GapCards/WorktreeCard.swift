import SwiftUI

/// Worktree card: entered or left, with the branch and path.
struct WorktreeCard: View {
    let gap: GapToolMetadata

    private var left: Bool { gap.result != nil || gap.action == "exit" }

    private var state: String {
        if left {
            switch gap.result {
            case "removed": return "Left, removed"
            case "refused": return "Exit refused"
            default: return "Left, kept"
            }
        }
        return "Entered"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Worktree")
                    .foregroundColor(Color("TextSecondary"))
                Spacer(minLength: 0)
                Text(state)
                    .foregroundColor(gap.result == "refused" ? Color.orange : Color("TextPrimary"))
            }
            .font(.caption.weight(.semibold))
            if let branch = gap.branch, !branch.isEmpty {
                Label(branch, systemImage: "arrow.triangle.branch")
                    .font(.footnote.weight(.medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let path = gap.path, !path.isEmpty {
                Text(path)
                    .font(.caption.monospaced())
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
                    .truncationMode(.head)
            }
            if let reason = gap.reason, !reason.isEmpty {
                Text(reason)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
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
