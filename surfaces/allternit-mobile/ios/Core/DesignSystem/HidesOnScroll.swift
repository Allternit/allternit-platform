import SwiftUI

/// Floating "New …" buttons show near the top of a page and slide away while
/// the page is scrolled down; they return at the top.
///
/// Three pieces:
/// - the scrolled content reports its offset (`reportsScrollOffset()` inside a
///   plain `ScrollView`; `DockingTitleScrollView` / `PolishedTabPage` already do),
/// - the screen holds `@State var barHidden` and `.tracksScrollForHide($barHidden)`
///   on a view above the scroll,
/// - the floating button gets `.hidesOnScroll(barHidden)`.
struct ScrollOffsetKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

extension View {
    /// Put on the content INSIDE a `ScrollView` (iOS 17 `.scrollView` space).
    /// Publishes how far the content is scrolled down (0 at the top).
    func reportsScrollOffset() -> some View {
        background(
            GeometryReader { g in
                Color.clear.preference(key: ScrollOffsetKey.self,
                                       value: max(0, -g.frame(in: .scrollView(axis: .vertical)).minY))
            }
        )
    }

    /// Sets `hidden` once the scroll is more than a few points from the top.
    func tracksScrollForHide(_ hidden: Binding<Bool>, threshold: CGFloat = 24) -> some View {
        onPreferenceChange(ScrollOffsetKey.self) { offset in
            let next = offset > threshold
            if hidden.wrappedValue != next { hidden.wrappedValue = next }
        }
    }

    /// Slides the floating button down and fades it while `hidden`.
    func hidesOnScroll(_ hidden: Bool) -> some View {
        modifier(HidesOnScroll(hidden: hidden))
    }
}

private struct HidesOnScroll: ViewModifier {
    let hidden: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .opacity(hidden ? 0 : 1)
            .offset(y: hidden && !reduceMotion ? 24 : 0)
            .allowsHitTesting(!hidden)
            .animation(reduceMotion ? .easeOut(duration: 0.15) : .spring(response: 0.3, dampingFraction: 0.85), value: hidden)
    }
}
