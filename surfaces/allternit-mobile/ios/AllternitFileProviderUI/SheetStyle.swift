import SwiftUI
import UIKit

/// The app's tokens (Assets.xcassets: BgPrimary, BgPanel, BgTertiary =
/// `--neutral-fill`, TextPrimary, TextSecondary, BorderSubtle), copied as
/// light + graphite dark values because an extension can't read the app's
/// catalog. White surfaces and the neutral fill, never the tan tokens.
enum SheetColor {
    static let background = dynamic(light: 0xFFFFFF, dark: 0x1F1F1E)
    static let panel = dynamic(light: 0xFFFFFF, dark: 0x262625)
    static let fill = dynamic(light: 0xF4F4F4, dark: 0x2C2C2B)
    static let text = dynamic(light: 0x1F1E1D, dark: 0xECECEC)
    static let secondary = dynamic(light: 0x6B6A68, dark: 0x9B9B9B)
    static let hairline = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark ? UIColor(white: 0.925, alpha: 0.08) : UIColor(red: 0.12, green: 0.118, blue: 0.114, alpha: 0.08)
    })
    /// Design 5C badge hues (Badges.xcassets in the File Provider).
    static let warning = Color(red: 0.91, green: 0.576, blue: 0.047)
    static let info = Color(red: 0.231, green: 0.51, blue: 0.965)
    static let success = Color(red: 0.204, green: 0.698, blue: 0.408)
    static let error = Color(red: 0.86, green: 0.27, blue: 0.27)

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            let v = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: CGFloat((v >> 16) & 0xFF) / 255, green: CGFloat((v >> 8) & 0xFF) / 255,
                           blue: CGFloat(v & 0xFF) / 255, alpha: 1)
        })
    }
}

/// One sheet's frame: a symbol in a neutral-fill tile, a title, a subtitle,
/// the body, and the buttons pinned to the bottom.
struct SheetScaffold<Content: View, Actions: View>: View {
    let symbol: String
    var tint: Color = SheetColor.text
    let title: String
    var subtitle: String?
    @ViewBuilder var content: Content
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(alignment: .center, spacing: 12) {
                        Image(systemName: symbol)
                            .font(.system(size: 19, weight: .semibold))
                            .foregroundStyle(tint)
                            .frame(width: 44, height: 44)
                            .background(SheetColor.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(title)
                                .font(.system(size: 19, weight: .semibold))
                                .foregroundStyle(SheetColor.text)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityAddTraits(.isHeader)
                            if let subtitle {
                                Text(subtitle)
                                    .font(.system(size: 14))
                                    .foregroundStyle(SheetColor.secondary)
                                    .lineLimit(2)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    content
                }
                .padding(.horizontal, 20)
                .padding(.top, 28)
                .padding(.bottom, 12)
            }
            VStack(spacing: 8) { actions }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 16)
        }
        .background(SheetColor.background.ignoresSafeArea())
    }
}

/// The 50 pt full-width button of the design (primary = ink on white,
/// white on ink in graphite dark).
struct SheetButton: View {
    let title: String
    var symbol: String?
    var primary = true
    var busy = false
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if busy {
                    ProgressView().tint(primary ? SheetColor.background : SheetColor.text)
                } else if let symbol {
                    Image(systemName: symbol).font(.system(size: 15, weight: .semibold))
                }
                Text(title).font(.system(size: 17, weight: .semibold))
            }
            .frame(maxWidth: .infinity, minHeight: 50)
            .foregroundStyle(primary ? SheetColor.background : SheetColor.text)
            .background(primary ? SheetColor.text : SheetColor.fill,
                        in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .opacity(enabled ? 1 : 0.45)
        }
        .buttonStyle(PressStyle())
        .disabled(!enabled || busy)
    }
}

/// Press feedback: a quick dim, no travel (fine with reduced motion).
struct PressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.78 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

/// A grouped card on the neutral fill.
struct SheetCard<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) { content }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(SheetColor.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

/// Body text at the sheet's reading size.
struct SheetText: View {
    let text: String
    var secondary = true

    init(_ text: String, secondary: Bool = true) {
        self.text = text
        self.secondary = secondary
    }

    var body: some View {
        Text(text)
            .font(.system(size: 15))
            .foregroundStyle(secondary ? SheetColor.secondary : SheetColor.text)
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// A one-line status with a coloured dot (the design's chip).
struct StatusLine: View {
    let text: String
    let color: Color

    var body: some View {
        HStack(spacing: 8) {
            Circle().fill(color).frame(width: 8, height: 8).accessibilityHidden(true)
            Text(text).font(.system(size: 14, weight: .medium)).foregroundStyle(SheetColor.text)
        }
    }
}

/// A segmented choice drawn like the design's `Seg` (fill track, panel
/// thumb), with each option a real button for VoiceOver.
struct SheetSegment<Value: Hashable>: View {
    let label: String
    let options: [(Value, String)]
    @Binding var value: Value

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label).font(.system(size: 13, weight: .medium)).foregroundStyle(SheetColor.secondary)
            HStack(spacing: 2) {
                ForEach(options, id: \.0) { option in
                    let selected = option.0 == value
                    Button { value = option.0 } label: {
                        Text(option.1)
                            .font(.system(size: 14, weight: selected ? .semibold : .regular))
                            .foregroundStyle(SheetColor.text)
                            .frame(maxWidth: .infinity, minHeight: 34)
                            .background(selected ? SheetColor.panel : .clear,
                                        in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                            .shadow(color: .black.opacity(selected ? 0.08 : 0), radius: 2, y: 1)
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(selected ? .isSelected : [])
                }
            }
            .padding(2)
            .background(SheetColor.fill, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
            .accessibilityElement(children: .contain)
            .accessibilityLabel(label)
        }
    }
}

/// An error line under the content.
struct ErrorLine: View {
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.circle.fill").foregroundStyle(SheetColor.error).accessibilityHidden(true)
            Text(text).font(.system(size: 14)).foregroundStyle(SheetColor.text).fixedSize(horizontal: false, vertical: true)
        }
    }
}
