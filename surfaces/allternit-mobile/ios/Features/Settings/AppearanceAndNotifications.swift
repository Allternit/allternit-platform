import SwiftUI
import UIKit
@preconcurrency import UserNotifications

// MARK: - Appearance

/// Light / Dark / System, the PWA's Appearance tiles. Dark is the graphite
/// theme (the color assets' dark variants). Applied by overriding every
/// window's interface style, so no view needs `.preferredColorScheme`.
@MainActor
final class AppearanceStore: ObservableObject {
    enum Mode: String, CaseIterable, Identifiable {
        case light, dark, system
        var id: String { rawValue }
        var label: String {
            switch self {
            case .light: return "Light"
            case .dark: return "Dark"
            case .system: return "System"
            }
        }
        var style: UIUserInterfaceStyle {
            switch self {
            case .light: return .light
            case .dark: return .dark
            case .system: return .unspecified
            }
        }

        /// The theme for a saved value: Light for anyone who never picked
        /// one (phone calm I13, "Light default"); a saved pick is kept.
        static func initial(saved: String?) -> Mode {
            saved.flatMap(Mode.init(rawValue:)) ?? .light
        }
    }

    static let shared = AppearanceStore()
    private static let key = "allternit-appearance-theme"

    @Published var theme: Mode {
        didSet {
            UserDefaults.standard.set(theme.rawValue, forKey: Self.key)
            apply()
        }
    }

    /// Reduce motion lives under `ReducedMotion.appKey` (`allternit.reduceMotion`)
    /// so `@ReducedMotion` views follow this one switch; builds before the
    /// calm pass saved it here and it is copied over once.
    private static let legacyReduceMotionKey = "allternit-appearance-reduce-motion"
    private static let chatFontKey = "allternit-appearance-chat-font"

    /// Settings → General → Chat font (the PWA's `appearance.chatFont`):
    /// the typeface of assistant replies in the transcript.
    enum ChatFont: String, CaseIterable, Identifiable {
        case `default`, serif, system
        var id: String { rawValue }
        var label: String {
            switch self {
            case .default: return "Default"
            case .serif: return "Serif"
            case .system: return "System"
            }
        }
    }

    @Published var chatFont: ChatFont {
        didSet { UserDefaults.standard.set(chatFont.rawValue, forKey: Self.chatFontKey) }
    }

    /// Settings → App → Reduce motion (the PWA's
    /// APPEARANCE_PREF_KEYS.reduceMotion): turns off UIKit view animations
    /// app-wide; SwiftUI views read `@ReducedMotion` (same key) to skip their own.
    @Published var reduceMotion: Bool {
        didSet {
            UserDefaults.standard.set(reduceMotion, forKey: ReducedMotion.appKey)
            apply()
        }
    }

    private init() {
        let defaults = UserDefaults.standard
        theme = Mode.initial(saved: defaults.string(forKey: Self.key))
        if let saved = defaults.object(forKey: ReducedMotion.appKey) as? Bool {
            reduceMotion = saved
        } else {
            let legacy = defaults.bool(forKey: Self.legacyReduceMotionKey)
            reduceMotion = legacy
            if legacy { defaults.set(true, forKey: ReducedMotion.appKey) }
        }
        chatFont = ChatFont(rawValue: UserDefaults.standard.string(forKey: Self.chatFontKey) ?? "") ?? .default
        // Windows created later (new scenes) pick the saved theme up too.
        NotificationCenter.default.addObserver(
            forName: UIScene.didActivateNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated { AppearanceStore.shared.apply() }
        }
    }

    /// Call at launch (and it runs on every change) to apply the saved theme.
    func apply() {
        UIView.setAnimationsEnabled(!reduceMotion)
        for scene in UIApplication.shared.connectedScenes {
            guard let windowScene = scene as? UIWindowScene else { continue }
            for window in windowScene.windows {
                window.overrideUserInterfaceStyle = theme.style
            }
        }
    }
}

/// Light / Dark / System as window thumbnails (phone.css `.phone-theme-tiles`):
/// a mini app window (sidebar, text lines, composer pill; System is split on
/// the diagonal). One ring in TextPrimary, set off by a gap, slides to the
/// picked tile; the picked label goes bold.
struct AppearanceTiles: View {
    @ObservedObject private var store = AppearanceStore.shared
    @Namespace private var ringSpace
    @ReducedMotion private var reduced

    var body: some View {
        HStack(spacing: 10) {
            ForEach(AppearanceStore.Mode.allCases) { option in
                let picked = store.theme == option
                Button(action: {
                    Haptics.selection()
                    withAnimation(reduced ? .easeOut(duration: 0.12) : .spring(response: 0.34, dampingFraction: 0.78)) {
                        store.theme = option
                    }
                }) {
                    VStack(spacing: 8) {
                        ThemeTilePicture(mode: option)
                            .aspectRatio(128.0 / 78.0, contentMode: .fit)
                            .clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
                            .overlay(
                                RoundedRectangle(cornerRadius: 9, style: .continuous)
                                    .strokeBorder(Color("BorderSubtle"), lineWidth: 1)
                            )
                            .background(
                                ZStack {
                                    if picked {
                                        RoundedRectangle(cornerRadius: 13, style: .continuous)
                                            .strokeBorder(Color("TextPrimary"), lineWidth: 2)
                                            .padding(-4)
                                            .matchedGeometryEffect(id: "themeRing", in: ringSpace)
                                    }
                                }
                            )
                        Text(option.label)
                            .font(.system(size: 13, weight: picked ? .bold : .regular))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .frame(maxWidth: .infinity)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(option.label)
                .accessibilityAddTraits(picked ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(EdgeInsets(top: 16, leading: 16, bottom: 12, trailing: 16))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Appearance")
    }
}

/// A mini app window: sidebar, two text lines, a composer pill. System draws
/// the light window with the dark one across the lower-right diagonal half.
private struct ThemeTilePicture: View {
    let mode: AppearanceStore.Mode

    var body: some View {
        switch mode {
        case .light:
            WindowThumb(dark: false)
        case .dark:
            WindowThumb(dark: true)
        case .system:
            ZStack {
                WindowThumb(dark: false)
                WindowThumb(dark: true)
                    .mask(DiagonalHalf())
            }
        }
    }
}

/// The lower-right triangle (corner to corner).
private struct DiagonalHalf: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.maxX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY))
        path.closeSubpath()
        return path
    }
}

private struct WindowThumb: View {
    let dark: Bool

    private var page: Color { dark ? Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1E / 255) : .white }
    private var side: Color { dark ? Color(red: 0x2A / 255, green: 0x2A / 255, blue: 0x28 / 255) : Color(red: 0xF1 / 255, green: 0xF1 / 255, blue: 0xEF / 255) }
    private var ink: Color { dark ? Color(red: 0.91, green: 0.91, blue: 0.90) : Color(red: 0.165, green: 0.165, blue: 0.157) }

    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width
            let h = geo.size.height
            ZStack(alignment: .topLeading) {
                page
                side.frame(width: w * 0.27, height: h)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.35))
                    .frame(width: w * 0.16, height: 3).offset(x: w * 0.05, y: h * 0.16)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.25))
                    .frame(width: w * 0.12, height: 3).offset(x: w * 0.05, y: h * 0.30)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.7))
                    .frame(width: w * 0.40, height: 4).offset(x: w * 0.34, y: h * 0.20)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.4))
                    .frame(width: w * 0.28, height: 4).offset(x: w * 0.34, y: h * 0.36)
                Capsule().fill(ink.opacity(0.14))
                    .frame(width: w * 0.58, height: h * 0.17).offset(x: w * 0.34, y: h * 0.74)
                Circle().fill(Color("BrandAccent"))
                    .frame(width: 6, height: 6).offset(x: w * 0.34 + w * 0.58 - 10, y: h * 0.74 + (h * 0.17 - 6) / 2)
            }
        }
    }
}
