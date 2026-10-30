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
    }

    static let shared = AppearanceStore()
    private static let key = "allternit-appearance-theme"

    @Published var theme: Mode {
        didSet {
            UserDefaults.standard.set(theme.rawValue, forKey: Self.key)
            apply()
        }
    }

    private static let reduceMotionKey = "allternit-appearance-reduce-motion"
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

    /// Settings → Appearance → Reduce motion (the PWA's
    /// APPEARANCE_PREF_KEYS.reduceMotion): turns off UIKit view animations
    /// app-wide; SwiftUI views read `reduceMotion` to skip their own.
    @Published var reduceMotion: Bool {
        didSet {
            UserDefaults.standard.set(reduceMotion, forKey: Self.reduceMotionKey)
            apply()
        }
    }

    private init() {
        theme = Mode(rawValue: UserDefaults.standard.string(forKey: Self.key) ?? "") ?? .system
        reduceMotion = UserDefaults.standard.bool(forKey: Self.reduceMotionKey)
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

/// Light / Dark / System as picture tiles (phone.css `.phone-theme-tiles`):
/// a mini page with two ink lines and an accent dot; the picked tile gets a
/// 2pt accent border and an accent label.
struct AppearanceTiles: View {
    @ObservedObject private var store = AppearanceStore.shared

    var body: some View {
        HStack(spacing: 10) {
            ForEach(AppearanceStore.Mode.allCases) { option in
                let picked = store.theme == option
                Button(action: {
                    UISelectionFeedbackGenerator().selectionChanged()
                    store.theme = option
                }) {
                    VStack(spacing: 6) {
                        ThemeTilePicture(mode: option)
                            .frame(height: 62)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .overlay(
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .strokeBorder(picked ? Color("BrandAccent") : Color("BorderSubtle"),
                                                  lineWidth: picked ? 2 : 1)
                            )
                        Text(option.label)
                            .font(.system(size: 13, weight: picked ? .semibold : .regular))
                            .foregroundColor(picked ? Color("BrandAccent") : Color("TextPrimary"))
                    }
                    .frame(maxWidth: .infinity)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(option.label)
                .accessibilityAddTraits(picked ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(12)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Appearance")
    }
}

/// `.phone-theme-pic`: white, graphite (#1F1F1E), or split on the diagonal.
private struct ThemeTilePicture: View {
    let mode: AppearanceStore.Mode

    private static let graphite = Color(red: 0x1F / 255, green: 0x1F / 255, blue: 0x1E / 255)

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                background(size: geo.size)
                let ink = mode == .dark ? Color(red: 0.91, green: 0.91, blue: 0.90) : Color(red: 0.165, green: 0.165, blue: 0.157)
                RoundedRectangle(cornerRadius: 2)
                    .fill(ink.opacity(0.7))
                    .frame(width: geo.size.width * 0.44, height: 4)
                    .offset(x: 10, y: 12)
                RoundedRectangle(cornerRadius: 2)
                    .fill(ink.opacity(0.4))
                    .frame(width: geo.size.width * 0.26, height: 4)
                    .offset(x: 10, y: 21)
                Circle()
                    .fill(Color("BrandAccent"))
                    .frame(width: 10, height: 10)
                    .offset(x: geo.size.width - 20, y: geo.size.height - 20)
            }
        }
    }

    @ViewBuilder
    private func background(size: CGSize) -> some View {
        switch mode {
        case .light:
            Color.white
        case .dark:
            Self.graphite
        case .system:
            ZStack {
                Self.graphite
                Path { path in
                    path.move(to: .zero)
                    path.addLine(to: CGPoint(x: size.width, y: 0))
                    path.addLine(to: CGPoint(x: 0, y: size.height))
                    path.closeSubpath()
                }
                .fill(Color.white)
            }
        }
    }
}

