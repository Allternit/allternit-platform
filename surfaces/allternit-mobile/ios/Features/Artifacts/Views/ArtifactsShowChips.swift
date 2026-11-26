import SwiftUI

/// "Show" chips on the Artifacts page: Everything, Apps, Plans, Shared
/// (web src/views/library/parts/ShowChips.tsx; look R2-C-phone.png).
/// One radio-style group; replaces the owner part of the filter popover.
enum ArtifactShowChip: String, CaseIterable, Identifiable {
    case all, apps, plans, shared

    var id: String { rawValue }

    var label: String {
        switch self {
        case .all: return "Everything"
        case .apps: return "Apps"
        case .plans: return "Plans"
        case .shared: return "Shared"
        }
    }
}

struct ArtifactsShowChips: View {
    @Binding var value: ArtifactShowChip
    var onChange: (ArtifactShowChip) -> Void = { _ in }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(ArtifactShowChip.allCases) { chip in
                    let active = chip == value
                    Button {
                        guard chip != value else { return }
                        Haptics.light()
                        value = chip
                        onChange(chip)
                    } label: {
                        Text(chip.label)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(active ? Color("BgPanel") : Color("TextSecondary"))
                            .padding(.horizontal, 14)
                            .frame(minHeight: 36)
                            .background(Capsule().fill(active ? Color("TextPrimary") : Color("BgPanel")))
                            .overlay(Capsule().stroke(active ? Color.clear : Color("BorderSubtle"), lineWidth: 1))
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(active ? [.isButton, .isSelected] : .isButton)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Show")
    }
}
