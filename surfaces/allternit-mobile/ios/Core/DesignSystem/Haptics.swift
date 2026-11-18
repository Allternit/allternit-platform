import SwiftUI
import UIKit

/// Calm-pass haptics. Names mirror the web `haptic()` helper: light, medium,
/// soft, selection. Gated by the Haptic feedback setting (UserDefaults key
/// `allternit.haptics`, default on). Kept here rather than in SettingsStore
/// so the key can be bound with `@AppStorage(Haptics.enabledKey)`.
enum Haptics {
    static let enabledKey = "allternit.haptics"

    static var isEnabled: Bool {
        get { UserDefaults.standard.object(forKey: enabledKey) as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: enabledKey) }
    }

    // Prepared generators, reused across calls.
    @MainActor private static let lightGen = UIImpactFeedbackGenerator(style: .light)
    @MainActor private static let mediumGen = UIImpactFeedbackGenerator(style: .medium)
    @MainActor private static let softGen = UIImpactFeedbackGenerator(style: .soft)
    @MainActor private static let selectionGen = UISelectionFeedbackGenerator()
    @MainActor private static let notifyGen = UINotificationFeedbackGenerator()

    @MainActor static func light() { guard isEnabled else { return }; lightGen.impactOccurred(); lightGen.prepare() }
    @MainActor static func medium() { guard isEnabled else { return }; mediumGen.impactOccurred(); mediumGen.prepare() }
    @MainActor static func soft() { guard isEnabled else { return }; softGen.impactOccurred(); softGen.prepare() }
    @MainActor static func selection() { guard isEnabled else { return }; selectionGen.selectionChanged(); selectionGen.prepare() }
    @MainActor static func success() { guard isEnabled else { return }; notifyGen.notificationOccurred(.success); notifyGen.prepare() }
    @MainActor static func warning() { guard isEnabled else { return }; notifyGen.notificationOccurred(.warning); notifyGen.prepare() }
    @MainActor static func error() { guard isEnabled else { return }; notifyGen.notificationOccurred(.error); notifyGen.prepare() }

    /// Warm the generators (call when a gesture begins).
    @MainActor static func prepare() {
        guard isEnabled else { return }
        lightGen.prepare(); mediumGen.prepare(); softGen.prepare(); selectionGen.prepare()
    }
}
