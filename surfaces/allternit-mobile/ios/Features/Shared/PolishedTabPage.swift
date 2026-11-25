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
    /// Line art above the title (replaces the icon circle). Nil keeps today's look.
    var art: EmptyArtKind? = nil
    /// Connect scene in place of the art (no-computer empties). Nil keeps the art/icon.
    var scene: ConnectSceneState? = nil
    /// Optional pill under the line.
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 10) {
            if let scene {
                ConnectScene(state: scene)
                    .padding(.bottom, 2)
            } else if let art {
                EmptyArt(kind: art, paper: Color("BgPrimary"))
                    .padding(.bottom, 2)
            } else {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(PolishTokens.ink)
                    .frame(width: 44, height: 44)
                    .background(Circle().fill(Color.white))
                    .overlay(Circle().strokeBorder(PolishTokens.glassBorder, lineWidth: 0.5))
                    .accessibilityHidden(true)
            }
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(PolishTokens.ink)
                .multilineTextAlignment(.center)
            Text(line)
                .font(.system(size: 14))
                .foregroundStyle(Color("TextSecondary"))
                .multilineTextAlignment(.center)
            if let actionTitle, let action {
                Button {
                    Haptics.light()
                    action()
                } label: {
                    Text(actionTitle)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(PolishTokens.ink)
                        .padding(.horizontal, 18)
                        .frame(height: 40)
                        .background(Capsule().fill(Color("BgPanel")))
                        .overlay(Capsule().strokeBorder(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 28)
        .padding(.horizontal, 20)
        .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(PolishTokens.neutralFill))
        .accessibilityElement(children: action == nil ? .combine : .contain)
    }
}

/// The page every polished tab uses: docking block title, description, then
/// content with 14 pt gutters and room for a floating button. Pass `introKey`
/// and `hero` to show the first-visit intro.
struct PolishedTabPage<Content: View, Hero: View>: View {
    let title: String
    var description: String?
    var taller = false
    /// `.navigationBar` inside a NavigationStack; a page under its own header
    /// (Artifacts' LibrarySectionHeader) passes `.overlay(barCenter:)`.
    var chrome: DockChrome = .navigationBar
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
        DockingTitleScrollView(title: title, description: description, taller: taller, chrome: chrome) {
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
