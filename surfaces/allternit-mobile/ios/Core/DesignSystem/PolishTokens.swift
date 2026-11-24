import SwiftUI

/// Mobile polish contract (handoffs/mobile-refresh-2026-11-23/CONTRACT.md),
/// ported exactly from the web contract. Do not change a number here; if one
/// is wrong, every lane needs the same fix.
enum PolishTokens {
    // MARK: Docking title
    /// Scroll points for the full dock.
    nonisolated static let dock: CGFloat = 170
    /// Big and docked letter height (one block = height / 5).
    nonisolated static let bigHeight: CGFloat = 22
    nonisolated static let smallHeight: CGFloat = 11
    /// Line pitch of a wrapped title (BH x 1.36).
    nonisolated static let lineGap: CGFloat = 22 * 1.36
    nonisolated static let titleMaxWidth: CGFloat = 316
    nonisolated static let bandPad: CGFloat = 22
    nonisolated static let descriptionFadeFraction: CGFloat = 0.6
    nonisolated static let descriptionBlur: CGFloat = 3
    nonisolated static let descriptionDrift: CGFloat = 0.35
    nonisolated static let flashDuration: Double = 0.9
    nonisolated static let flashDelayPerColumn: Double = 0.012

    // MARK: Intro
    /// Continue stays disabled grey for this long (ms), then fades to ink in 350 ms.
    static let introUnlockMs = 1200
    static let introUnlockFadeMs = 350
    static let introDisabledGrey = Color(hex: "#CFCFCF")

    // MARK: Layout
    static let pushX: CGFloat = 300
    static let pushScale: CGFloat = 0.92
    static let pushCornerRadius: CGFloat = 38
    static let pushDim = Color(red: 31 / 255, green: 30 / 255, blue: 29 / 255).opacity(0.06)
    static let sheetInset: CGFloat = 6
    static let sheetRadius: CGFloat = 36
    static let sheetEnterY: CGFloat = 620
    static let introInset: CGFloat = 6
    static let introRadius: CGFloat = 38
    static let introEnterY: CGFloat = 560
    static let introScrim = Color(red: 20 / 255, green: 20 / 255, blue: 20 / 255).opacity(0.22)
    static let introScrimBlur: CGFloat = 3

    // MARK: Press scales
    static let pressButton: CGFloat = 0.88
    static let pressRow: CGFloat = 0.98
    static let pressPill: CGFloat = 0.94
    static let pressContinue: CGFloat = 0.97
    static let pressIcon: CGFloat = 0.85

    // MARK: Colours
    /// Coral `#D97757`.
    static let coral = Color(hex: "#D97757")
    static let ink = Color("TextPrimary")
    static let glassFill = Color.white.opacity(0.72)
    static let glassBorder = Color(red: 31 / 255, green: 30 / 255, blue: 29 / 255).opacity(0.09)
    static let neutralFill = Color(red: 31 / 255, green: 30 / 255, blue: 29 / 255).opacity(0.05)
    static let neutralFill2 = Color(hex: "#F4F4F4")

    // MARK: Springs
    /// The contract gives framer-motion style stiffness/damping (mass 1).
    /// SwiftUI's `.spring(response:dampingFraction:)` is the same system:
    /// response = 2 pi / sqrt(stiffness), dampingFraction = damping / (2 sqrt(stiffness)).
    static func spring(stiffness: Double, damping: Double) -> Animation {
        .spring(response: 2 * .pi / stiffness.squareRoot(),
                dampingFraction: damping / (2 * stiffness.squareRoot()))
    }
    static let press = spring(stiffness: 600, damping: 24)
    static let push = spring(stiffness: 380, damping: 36)
    static let sheet = spring(stiffness: 340, damping: 34)
    static let intro = spring(stiffness: 320, damping: 32)
    static let base = spring(stiffness: 420, damping: 34)
    static let team = spring(stiffness: 260, damping: 24)
    static let arrivalBig = spring(stiffness: 200, damping: 22)
    static let lens = spring(stiffness: 500, damping: 34)
    /// Docking letter i: stiffness 520 - (i % 9) * 25, damping 28.
    static func letters(_ i: Int) -> Animation { spring(stiffness: Double(520 - (i % 9) * 25), damping: 28) }
    /// Team avatar delay: .08 + i * .07 s.
    static func teamDelay(_ i: Int) -> Double { 0.08 + Double(i) * 0.07 }
}
