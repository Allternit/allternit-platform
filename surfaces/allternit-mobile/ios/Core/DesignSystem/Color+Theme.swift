import SwiftUI

extension Color {
    init(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            (a, r, g, b) = (1, 1, 1, 1)
        }
        
        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}

// Global Theme Colors matching surfaces/ai.allternit.com design system.
// All values are taken from theme.css light (:root) and dark ([data-theme='dark'])
// ramps. Asset-catalog colors (Color("…")) adapt automatically; the static
// constants below are for code-path fallbacks and non-adaptive contexts.
struct Theme {
    // MARK: - Surfaces and text (m.allternit.com phone tokens, light + graphite
    // dark; adaptive through the asset catalog). No tan: white + neutral fill.
    static let bgPrimary = Color("BgPrimary")
    static let bgSecondary = Color("BgSecondary")
    static let accentPrimary = Color("AccentPrimary")
    static let textPrimary = Color("TextPrimary")
    static let textSecondary = Color("TextSecondary")
    static let borderSubtle = Color("BorderSubtle")

    /// `--bg-elevated` — cards and sheets.
    static let bgPanel = Color("BgPanel")
    /// `--neutral-fill` (opaque) — chips, trays, rows.
    static let bgTertiary = Color("BgTertiary")

    /// Translucent bars and sheets over the page.
    static let glassBg = Color("BgPrimary").opacity(0.82)
    static let glassBgThick = Color("BgPrimary").opacity(0.95)

    /// `--polish-hairline` ramp (text at 8% / 11% / 14%).
    static let borderWarmSubtle = Color("BorderSubtle")
    static let borderWarmDefault = Color("TextPrimary").opacity(0.11)
    static let borderWarmStrong = Color("BorderStrong")
    static let shadowGlow = Color.black.opacity(0.06)

    /// Per-mode accents (theme.css `--accent-*`).
    /// These are adaptive via Color("AccentChat") etc.; statics are dark fallbacks.
    static let accentChat = Color(hex: "#D97757")
    static let accentCowork = Color(hex: "#B08BC0")
    static let accentCode = Color(hex: "#7BB08B")
    static let accentBrowser = Color(hex: "#69A8C8")

    /// Status hues used by agent-mode tiles (theme.css:437-443).
    static let statusSuccess = Color(hex: "#4ADE80")
    static let statusWarning = Color(hex: "#FBBF24")
    static let statusInfo = Color(hex: "#60A5FA")
    static let statusError = Color(hex: "#F87171")

    // MARK: - Radius scale (theme.css:35-42)

    static let radiusSM: CGFloat = 8
    static let radiusMD: CGFloat = 12
    static let radiusLG: CGFloat = 16
    static let radiusXL: CGFloat = 20
}

// SwiftUI Asset Catalog Color Adaptations — these automatically resolve
// light/dark appearance via the asset catalog.
extension Color {
    static var themeBgPrimary: Color { Color("BgPrimary") }
    static var themeBgSecondary: Color { Color("BgSecondary") }
    static var themeBgPanel: Color { Color("BgPanel") }
    static var themeAccentPrimary: Color { Color("AccentPrimary") }
    static var themeTextPrimary: Color { Color("TextPrimary") }
    static var themeTextSecondary: Color { Color("TextSecondary") }
    static var themeBorderSubtle: Color { Color("BorderSubtle") }
}
