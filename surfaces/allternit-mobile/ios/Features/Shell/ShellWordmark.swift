import SwiftUI

/// The drawer header's wordmark for the current mode (moved out of the removed
/// ModeDialView when Shelves replaced the dial; the sidebar header still shows it).
struct ShellWordmark: View {
    let mode: ShellMode

    var body: some View {
        Group {
            if mode == .code {
                // The pixel GIZZI CODE lockup the web drawer shows.
                GizziCodeWordmark(height: 15)
            } else if let suffix = mode.wordmarkSuffix {
                // Bots and ACI show their word alone (no Allternit in front; Eoj 2026-11-24).
                PixelWordmark(height: 16, suffix: suffix, wordOnly: true)
            } else {
                PixelWordmark(height: 16)
            }
        }
        .lineLimit(1)
        .accessibilityIdentifier("shellWordmark\(mode.rawValue)")
    }
}
