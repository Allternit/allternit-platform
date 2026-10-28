import SwiftUI
import UIKit

/// The drawer's mode dial (allternit-ai src/fabric-session/phone/PhoneModeWheel.tsx,
/// ai #863): the active mode as a round filled icon — the wordmark beside it
/// names the mode. Tap it and all four modes fan out in a labelled
/// quarter-moon down and to the right; tap one to switch.
///
/// The fan itself is `ModeDialMenu`, drawn by the drawer above its list so
/// the cards can overlap the rail rows.
struct ModeDialButton: View {
    @Binding var isExpanded: Bool
    @EnvironmentObject private var modeStore: AppModeStore

    var body: some View {
        let mode = modeStore.shellMode
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            withAnimation(.spring(response: 0.3, dampingFraction: 0.82)) {
                isExpanded.toggle()
            }
        } label: {
            Image(systemName: mode.icon)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("BgPrimary"))
                .frame(width: 34, height: 34)
                .background(Circle().fill(Color("TextPrimary")))
                .scaleEffect(isExpanded ? 0.92 : 1)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(mode.label) mode. Switch mode")
        .accessibilityHint(isExpanded ? "Closes the mode picker" : "Opens the mode picker")
        .accessibilityIdentifier("modeDial")
    }
}

/// The quarter-moon of mode cards. Lay it out with its top-leading corner at
/// the dial button's top-leading corner (a 44pt square).
struct ModeDialMenu: View {
    @Binding var isExpanded: Bool
    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var fanned = false

    /// Quarter-moon spots from the dial's centre (PhoneModeWheel SPOTS):
    /// radius 128, Home straight down, ACI straight right, nudged 24 down.
    private static let spots: [CGPoint] = {
        let count = ShellMode.allCases.count
        return (0..<count).map { index in
            let angle = (Double.pi / 2) * (1 - Double(index) / Double(count - 1))
            return CGPoint(x: (cos(angle) * 128).rounded(), y: (sin(angle) * 128 + 24).rounded())
        }
    }()

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(Array(ShellMode.allCases.enumerated()), id: \.element) { index, option in
                let spot = Self.spots[index]
                let show = fanned || reduceMotion
                optionCard(option)
                    // Card centre on the dial centre (22,22), then out to its spot.
                    .offset(x: 22 - 29 + (show ? spot.x : 0), y: 22 - 29 + (show ? spot.y : 0))
                    .scaleEffect(show ? 1 : 0.6)
                    .opacity(show ? 1 : 0)
                    .animation(
                        reduceMotion ? nil : .spring(response: 0.26, dampingFraction: 0.8).delay(Double(index) * 0.025),
                        value: fanned
                    )
            }
        }
        .frame(width: 44, height: 44, alignment: .topLeading)
        .onAppear { fanned = true }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Modes")
    }

    private func optionCard(_ option: ShellMode) -> some View {
        let isActive = option == modeStore.shellMode
        return Button {
            UIImpactFeedbackGenerator(style: isActive ? .light : .medium).impactOccurred()
            withAnimation(.spring(response: 0.3, dampingFraction: 0.86)) {
                isExpanded = false
            }
            if !isActive {
                modeStore.setShellMode(option)
            }
        } label: {
            VStack(spacing: 2) {
                Image(systemName: option.icon)
                    .font(.system(size: 14, weight: isActive ? .bold : .regular))
                    .foregroundColor(isActive ? Color("BgPrimary") : Color("TextPrimary"))
                    .frame(width: 32, height: 32)
                    .background(Circle().fill(isActive ? Color("TextPrimary") : Color("BgTertiary")))
                Text(option.label)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
            .frame(width: 58, height: 58)
            .background(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(Color("BgPrimary"))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
            .shadow(color: Color.black.opacity(0.14), radius: 12, x: 0, y: 8)
            .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(option.label)
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
        .accessibilityIdentifier("modeDialOption\(option.rawValue)")
    }
}

/// The mode's wordmark beside the dial (PhoneDrawer `phone-wordmark`):
/// A://TERNIT, A://TERNIT BOTS, A://TERNIT ACI, or the gizzi code mark.
struct ShellWordmark: View {
    let mode: ShellMode

    var body: some View {
        Group {
            if mode == .code {
                Text("gizzi code")
                    .font(.system(size: 16, weight: .bold, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel("gizzi code")
            } else if let suffix = mode.wordmarkSuffix {
                // Suffixed marks run smaller so they fit beside the dial and search.
                HStack(spacing: 5) {
                    WordmarkView(height: 12)
                    Text(suffix)
                        .font(.system(size: 12, weight: .heavy))
                        .tracking(1.2)
                        .foregroundColor(Color("TextPrimary"))
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Allternit \(mode.label)")
            } else {
                WordmarkView(height: 16)
            }
        }
        .lineLimit(1)
        .accessibilityIdentifier("shellWordmark\(mode.rawValue)")
    }
}
