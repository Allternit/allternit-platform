import SwiftUI

// Polish controls (handoffs/mobile-refresh-2026-11-23, step I4).
// Neutral glass (D8), press springs from PolishTokens, haptics through the
// Haptics enum (D10). No tan: white .72 light, graphite dark.

/// Neutral glass backing: built on `glassSurface`, with the contract's fill,
/// hairline border and soft shadow on top.
private struct PolishGlass: ViewModifier {
    let cornerRadius: CGFloat
    @Environment(\.colorScheme) private var scheme

    private var shape: RoundedRectangle { RoundedRectangle(cornerRadius: cornerRadius, style: .continuous) }

    func body(content: Content) -> some View {
        content
            .background(shape.fill(scheme == .dark ? Color(white: 0.14).opacity(0.78) : PolishTokens.glassFill))
            .glassSurface(cornerRadius: cornerRadius)
            .overlay(shape.strokeBorder(scheme == .dark ? Color.white.opacity(0.10) : PolishTokens.glassBorder, lineWidth: 0.5))
            .shadow(color: Color.black.opacity(scheme == .dark ? 0.35 : 0.08), radius: 10, x: 0, y: 3)
    }
}

extension View {
    /// Neutral glass surface from the polish contract.
    func polishGlass(cornerRadius: CGFloat = 20) -> some View {
        modifier(PolishGlass(cornerRadius: cornerRadius))
    }
}

/// Press scale with the polish press spring.
private struct PolishPressStyle: ButtonStyle {
    let scale: CGFloat
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
    }
}

/// Round glass icon button (hamburger, back, plus).
struct GlassCircleButton: View {
    let systemImage: String
    let label: String
    var size: CGFloat = 40
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.light()
            action()
        } label: {
            Image(systemName: systemImage)
                .font(.system(size: size * 0.42, weight: .semibold))
                .foregroundStyle(PolishTokens.ink)
                .frame(width: size, height: size)
                .polishGlass(cornerRadius: size / 2)
                .contentShape(Circle())
        }
        .buttonStyle(PolishPressStyle(scale: PolishTokens.pressButton))
        .accessibilityLabel(label)
        .accessibilityAddTraits(.isButton)
    }
}

/// Glass pill (the right pill: a short label, optional icon).
struct GlassPill: View {
    let title: String
    var systemImage: String?
    var accessibilityLabelText: String?
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.light()
            action()
        } label: {
            HStack(spacing: 6) {
                if let systemImage { Image(systemName: systemImage).font(.system(size: 13, weight: .semibold)) }
                Text(title).font(.system(size: 14, weight: .semibold))
            }
            .foregroundStyle(PolishTokens.ink)
            .padding(.horizontal, 14)
            .frame(height: 36)
            .polishGlass(cornerRadius: 18)
            .contentShape(Capsule())
        }
        .buttonStyle(PolishPressStyle(scale: PolishTokens.pressPill))
        .accessibilityLabel(accessibilityLabelText ?? title)
        .accessibilityAddTraits(.isButton)
    }
}

/// Filter pills: the picked pill is an ink chip that slides to the new pick.
struct FilterPills<ID: Hashable>: View {
    struct Item: Identifiable {
        let id: ID
        let title: String
        init(id: ID, title: String) { self.id = id; self.title = title }
    }

    let items: [Item]
    @Binding var selection: ID
    @Namespace private var ns
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(items) { item in
                    let picked = item.id == selection
                    Button {
                        guard !picked else { return }
                        Haptics.selection()
                        withAnimation(reduceMotion ? nil : PolishTokens.base) { selection = item.id }
                    } label: {
                        Text(item.title)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(picked ? Color("BgPrimary") : PolishTokens.ink)
                            .padding(.horizontal, 14)
                            .frame(height: 34)
                            .background {
                                if picked {
                                    Capsule().fill(PolishTokens.ink)
                                        .matchedGeometryEffect(id: "filterPick", in: ns)
                                } else {
                                    Capsule().fill(PolishTokens.neutralFill)
                                }
                            }
                            .contentShape(Capsule())
                    }
                    .buttonStyle(PolishPressStyle(scale: PolishTokens.pressPill))
                    .accessibilityLabel(item.title)
                    .accessibilityAddTraits(picked ? [.isButton, .isSelected] : .isButton)
                }
            }
            .padding(.horizontal, 14)
        }
    }
}

/// Sliding segmented control: a white thumb on a grey track (List / Graph /
/// Calendar, Yours / Discover).
struct SlidingSegmented<ID: Hashable>: View {
    struct Item: Identifiable {
        let id: ID
        let title: String
        init(id: ID, title: String) { self.id = id; self.title = title }
    }

    let items: [Item]
    @Binding var selection: ID
    @Namespace private var ns
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        HStack(spacing: 0) {
            ForEach(items) { item in
                let picked = item.id == selection
                Button {
                    guard !picked else { return }
                    Haptics.selection()
                    withAnimation(reduceMotion ? nil : PolishTokens.lens) { selection = item.id }
                } label: {
                    Text(item.title)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(picked ? PolishTokens.ink : Color("TextSecondary"))
                        .frame(maxWidth: .infinity)
                        .frame(height: 32)
                        .background {
                            if picked {
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .fill(scheme == .dark ? Color(white: 0.28) : Color.white)
                                    .shadow(color: Color.black.opacity(0.10), radius: 3, x: 0, y: 1)
                                    .matchedGeometryEffect(id: "segThumb", in: ns)
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.title)
                .accessibilityAddTraits(picked ? [.isButton, .isSelected] : .isButton)
            }
        }
        .padding(3)
        .background(RoundedRectangle(cornerRadius: 13, style: .continuous).fill(PolishTokens.neutralFill))
        .accessibilityElement(children: .contain)
    }
}
