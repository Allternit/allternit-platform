import SwiftUI

/// "N need you" over the Workspace floor (top left): the bots waiting on the
/// person as stacked colour dots. A tap opens the first one's thread sheet
/// with its approval card docked at the top.
struct OfficeNeedPill: View {
    /// The bots that need the person, first one first.
    let bots: [AgentRecord]
    let onTap: () -> Void

    @ObservedObject private var colors = BotColorStore.shared

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 8) {
                HStack(spacing: -5) {
                    ForEach(Array(bots.prefix(3).enumerated()), id: \.element.id) { index, bot in
                        Circle()
                            .fill(Color(hex: colors.hex(for: bot) ?? "#9CA3AF"))
                            .frame(width: 14, height: 14)
                            .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 2))
                            .zIndex(Double(3 - index))
                    }
                }
                .accessibilityHidden(true)
                Text("\(bots.count) need you")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(Capsule().fill(Color("BgPanel")))
            .overlay(Capsule().stroke(Color("BorderSubtle")))
            .shadow(color: .black.opacity(0.08), radius: 6, y: 2)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(bots.count) need you: \(bots.prefix(3).map(\.botDisplayName).joined(separator: ", "))")
        .accessibilityHint("Opens \(bots.first?.botDisplayName ?? "the first one")")
    }
}
