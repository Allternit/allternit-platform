import SwiftUI

/// First-visit intro sheet (mobile refresh contract): a scrim with a 3 pt
/// blur, a sheet that slides up on the intro spring (320/32), inset 6,
/// radius 38, a hero slot, a coral block-type eyebrow, a 23 pt bold title,
/// one line, and a Continue that unlocks after 1.2 s with a 350 ms colour fade.
struct IntroSheet<Hero: View>: View {
    let intro: IntroKey
    let onContinue: () -> Void
    let hero: Hero

    @State private var ready = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AccessibilityFocusState private var titleFocused: Bool

    init(intro: IntroKey, onContinue: @escaping () -> Void, @ViewBuilder hero: () -> Hero) {
        self.intro = intro
        self.onContinue = onContinue
        self.hero = hero()
    }

    var body: some View {
        let copy = intro.copy
        ZStack(alignment: .bottom) {
            PolishTokens.introScrim
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture {}
                .accessibilityHidden(true)
                .transition(.opacity)

            VStack(alignment: .leading, spacing: 0) {
                hero
                    .frame(maxWidth: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 0) {
                    BlockText(copy.eyebrow, height: 9, color: PolishTokens.coral)
                        .accessibilityHidden(true)
                    Text(copy.title)
                        .font(.system(size: 23, weight: .bold))
                        .lineSpacing(23 * 0.2 - 4)
                        .tracking(-0.3)
                        .padding(.top, 6)
                        .padding(.bottom, 8)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityLabel("\(copy.eyebrow.capitalized). \(copy.title)")
                        .accessibilityFocused($titleFocused)
                    Text(copy.line)
                        .font(.system(size: 14))
                        .foregroundStyle(.secondary)
                        .lineSpacing(14 * 0.45 - 14 * 0.2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 8)
                .padding(.top, 16)
                .frame(maxWidth: .infinity, alignment: .leading)

                Button(action: proceed) {
                    Text("Continue")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(Color.white)
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .background(Capsule().fill(ready ? PolishTokens.ink : PolishTokens.introDisabledGrey))
                }
                .buttonStyle(IntroContinueStyle(enabled: ready))
                .disabled(!ready)
                .padding(.horizontal, 4)
                .padding(.top, 18)
                .accessibilityHint(ready ? "" : "Available in a moment")
            }
            .padding(EdgeInsets(top: 14, leading: 14, bottom: 18, trailing: 14))
            .background(
                RoundedRectangle(cornerRadius: PolishTokens.introRadius, style: .continuous)
                    .fill(Color(.systemBackground))
                    .shadow(color: .black.opacity(0.14), radius: 20, x: 0, y: -5)
            )
            .padding(.horizontal, PolishTokens.introInset)
            .padding(.bottom, PolishTokens.introInset)
            .transition(reduceMotion ? .opacity : .offset(x: 0, y: PolishTokens.introEnterY))
            .accessibilityElement(children: .contain)
            .accessibilityAddTraits(.isModal)
            .accessibilityAction(.escape) { if ready { proceed() } }

            // Hardware keyboard Escape.
            Button("", action: { if ready { proceed() } })
                .keyboardShortcut(.cancelAction)
                .frame(width: 0, height: 0)
                .opacity(0)
                .accessibilityHidden(true)
        }
        .task {
            try? await Task.sleep(nanoseconds: UInt64(PolishTokens.introUnlockMs) * 1_000_000)
            withAnimation(.easeInOut(duration: Double(PolishTokens.introUnlockFadeMs) / 1000)) { ready = true }
        }
        .task {
            try? await Task.sleep(nanoseconds: 400_000_000)
            titleFocused = true
        }
    }

    private func proceed() {
        guard ready else { return }
        Haptics.medium()
        onContinue()
    }
}

private struct IntroContinueStyle: ButtonStyle {
    let enabled: Bool
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && enabled ? PolishTokens.pressContinue : 1)
            .animation(PolishTokens.press, value: configuration.isPressed)
    }
}

/// True while a first-visit intro is presented below this view. A screen reads
/// it (`.onPreferenceChange(IntroShowingKey.self)`) to hide its floating
/// buttons and mute its header so nothing sits above the scrim.
struct IntroShowingKey: PreferenceKey {
    static let defaultValue = false
    static func reduce(value: inout Bool, nextValue: () -> Bool) { value = value || nextValue() }
}

private struct FirstVisitIntro<Hero: View>: ViewModifier {
    let key: IntroKey
    let hero: () -> Hero

    @State private var shown = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .blur(radius: shown && !reduceMotion ? PolishTokens.introScrimBlur : 0)
            .accessibilityHidden(shown)
            .preference(key: IntroShowingKey.self, value: shown)
            .overlay {
                if shown {
                    IntroSheet(intro: key, onContinue: dismiss, hero: hero)
                }
            }
            .animation(reduceMotion ? .easeOut(duration: 0.2) : PolishTokens.intro, value: shown)
            .task {
                if await IntroSeenStore.shared.shouldShow(key) { shown = true }
            }
    }

    private func dismiss() {
        IntroSeenStore.shared.markSeen(key)
        shown = false
    }
}

extension View {
    /// Shows the intro for `key` once per account (and once per device).
    func firstVisitIntro<Hero: View>(key: IntroKey, @ViewBuilder hero: @escaping () -> Hero) -> some View {
        modifier(FirstVisitIntro(key: key, hero: hero))
    }
}
