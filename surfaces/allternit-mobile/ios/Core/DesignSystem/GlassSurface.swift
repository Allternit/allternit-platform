import SwiftUI

/// Glass surface. iOS 26+ with the 6.2 compiler: `glassEffect`; otherwise
/// `.ultraThinMaterial` + hairline. Reduce Transparency: solid fill.
/// Guard pattern: `#if compiler(>=6.2)` + `#available(iOS 26.0, *)`
/// (repo precedent: `#available(iOS 26.0, *)` in Core/Voice).
private struct GlassSurfaceModifier: ViewModifier {
    let cornerRadius: CGFloat
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.colorScheme) private var scheme

    private var shape: RoundedRectangle { RoundedRectangle(cornerRadius: cornerRadius, style: .continuous) }

    func body(content: Content) -> some View {
        if reduceTransparency {
            content.background(shape.fill(scheme == .dark ? Color(white: 0.13) : Color.white))
                .overlay(shape.strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5))
        } else {
            modern(content)
        }
    }

    @ViewBuilder private func modern(_ content: Content) -> some View {
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            content.glassEffect(.regular, in: shape)
        } else {
            fallback(content)
        }
        #else
        fallback(content)
        #endif
    }

    private func fallback(_ content: Content) -> some View {
        content.background(.ultraThinMaterial, in: shape)
            .overlay(shape.strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5))
    }
}

extension View {
    func glassSurface(cornerRadius: CGFloat = 20) -> some View {
        modifier(GlassSurfaceModifier(cornerRadius: cornerRadius))
    }
}

/// Groups glass surfaces so they blend on iOS 26; a plain container before.
struct GlassContainer<Content: View>: View {
    var spacing: CGFloat = 12
    @ViewBuilder var content: () -> Content

    var body: some View {
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: spacing) { content() }
        } else {
            content()
        }
        #else
        content()
        #endif
    }
}
