import SwiftUI

/// The motion vocabulary of the Settings icon tiles (web `AnimatedIcon` MOTION
/// map) reduced to what SF Symbol effects can do: ring/wiggle, spin/rotate,
/// breathe, halo/pulse; every other motion (pop, bob, rise, ...) is a bounce.
enum GlyphMotion {
    case bounce, wiggle, rotate, breathe, pulse

    /// The motion for an SF Symbol name. Unlisted symbols bob (default D:
    /// the quietest motion, so no row sits still beside moving ones).
    static func forSymbol(_ name: String) -> GlyphMotion {
        switch name {
        case "bell": return .wiggle
        case "gearshape", "gearshape.2": return .rotate
        case "brain": return .breathe
        case "mic", "waveform.path.ecg", "cpu", "heart.text.square": return .pulse
        case "iphone.radiowaves.left.and.right": return .wiggle
        default: return .bounce
        }
    }
}

/// An SF Symbol that plays one effect each time `trigger` changes. Reduce
/// motion (system or the in-app preference) means no effect at all.
struct AnimatedGlyph: View {
    let systemImage: String
    var motion: GlyphMotion? = nil
    var trigger: Int

    @ReducedMotion private var reduced

    var body: some View {
        let image = Image(systemName: systemImage)
        let kind = motion ?? GlyphMotion.forSymbol(systemImage)
        if reduced {
            image
        } else {
            effect(image, kind)
        }
    }

    @ViewBuilder
    private func effect(_ image: Image, _ kind: GlyphMotion) -> some View {
        switch kind {
        case .pulse:
            image.symbolEffect(.pulse, options: .nonRepeating, value: trigger)
        case .wiggle:
            if #available(iOS 18.0, *) {
                image.symbolEffect(.wiggle, options: .nonRepeating, value: trigger)
            } else {
                image.symbolEffect(.bounce, options: .nonRepeating, value: trigger)
            }
        case .rotate:
            if #available(iOS 18.0, *) {
                image.symbolEffect(.rotate, options: .nonRepeating, value: trigger)
            } else {
                image.symbolEffect(.bounce, options: .nonRepeating, value: trigger)
            }
        case .breathe:
            if #available(iOS 18.0, *) {
                image.symbolEffect(.breathe, options: .nonRepeating, value: trigger)
            } else {
                image.symbolEffect(.pulse, options: .nonRepeating, value: trigger)
            }
        case .bounce:
            image.symbolEffect(.bounce, options: .nonRepeating, value: trigger)
        }
    }
}

/// True while the row holding a tile is pressed; set by the Settings row
/// button styles so the tile can play on press.
private struct SettingsRowPressedKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    var settingsRowPressed: Bool {
        get { self[SettingsRowPressedKey.self] }
        set { self[SettingsRowPressedKey.self] = newValue }
    }
}

/// White tile (graphite-safe: the primary background, a hairline border) with
/// an animated SF Symbol in neutral ink. Plays on appear and on row press.
struct SettingsIconTile: View {
    let systemImage: String
    var size: CGFloat = 30

    @Environment(\.settingsRowPressed) private var pressed
    @State private var tick = 0

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
            .fill(Color("BgPrimary"))
            .overlay(
                RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
                    .strokeBorder(Color("BorderSubtle"), lineWidth: 1)
            )
            .frame(width: size, height: size)
            .overlay(
                AnimatedGlyph(systemImage: systemImage, trigger: tick)
                    .font(.system(size: size * 0.54, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
            )
            .task {
                // Let the row settle before the one-shot plays.
                try? await Task.sleep(nanoseconds: 250_000_000)
                tick += 1
            }
            .onChange(of: pressed) { _, isPressed in
                if isPressed { tick += 1 }
            }
            .accessibilityHidden(true)
    }
}
