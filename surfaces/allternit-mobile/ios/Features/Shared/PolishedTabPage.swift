import SwiftUI

/// Section label: small caps-style title with an optional trailing view.
struct SectionLabel<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: () -> Trailing

    init(_ title: String, @ViewBuilder trailing: @escaping () -> Trailing) {
        self.title = title
        self.trailing = trailing
    }

    var body: some View {
        HStack {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Color("TextSecondary"))
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            trailing()
        }
        .padding(.horizontal, 4)
    }
}

extension SectionLabel where Trailing == EmptyView {
    init(_ title: String) {
        self.title = title
        self.trailing = { EmptyView() }
    }
}

/// Empty state box: neutral fill, radius 20, a white circle with a plus.
struct EmptyBox: View {
    var icon: String = "plus"
    let title: String
    let line: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(PolishTokens.ink)
                .frame(width: 44, height: 44)
                .background(Circle().fill(Color.white))
                .overlay(Circle().strokeBorder(PolishTokens.glassBorder, lineWidth: 0.5))
                .accessibilityHidden(true)
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(PolishTokens.ink)
                .multilineTextAlignment(.center)
            Text(line)
                .font(.system(size: 14))
                .foregroundStyle(Color("TextSecondary"))
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 28)
        .padding(.horizontal, 20)
        .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(PolishTokens.neutralFill))
        .accessibilityElement(children: .combine)
    }
}

/// The page every polished tab uses: docking block title, description, then
/// content with 14 pt gutters and room for a floating button. Pass `introKey`
/// and `hero` to show the first-visit intro.
struct PolishedTabPage<Content: View, Hero: View>: View {
    let title: String
    var description: String?
    var taller = false
    var introKey: IntroKey?
    @ViewBuilder var hero: () -> Hero
    @ViewBuilder var content: () -> Content

    /// Space under the content so a floating button never covers the last row.
    static var bottomSpace: CGFloat { 96 }

    var body: some View {
        if let introKey {
            page.firstVisitIntro(key: introKey, hero: hero)
        } else {
            page
        }
    }

    private var page: some View {
        DockingTitleScrollView(title: title, description: description, taller: taller) {
            VStack(alignment: .leading, spacing: 16) {
                content()
            }
            .padding(.horizontal, 14)
            .padding(.bottom, Self.bottomSpace)
        }
    }
}

extension PolishedTabPage where Hero == EmptyView {
    init(title: String, description: String? = nil, taller: Bool = false,
         @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.description = description
        self.taller = taller
        self.introKey = nil
        self.hero = { EmptyView() }
        self.content = content
    }
}
