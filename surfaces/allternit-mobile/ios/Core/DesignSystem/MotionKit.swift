import SwiftUI
import UIKit

/// `@ReducedMotion private var reduced` is true when system Reduce Motion is
/// on OR the in-app Reduce motion preference (`allternit.reduceMotion`) is on.
@propertyWrapper
struct ReducedMotion: DynamicProperty {
    static let appKey = "allternit.reduceMotion"

    @Environment(\.accessibilityReduceMotion) private var system
    @AppStorage(ReducedMotion.appKey) private var app = false

    var wrappedValue: Bool { system || app }

    /// Non-view contexts.
    static var current: Bool {
        UIAccessibility.isReduceMotionEnabled || UserDefaults.standard.bool(forKey: appKey)
    }
}

/// Named animations from the calm pass motion table.
enum Motion {
    static let drawer: Animation = .spring(response: 0.38, dampingFraction: 1.0)
    static let fan: Animation = .spring(response: 0.26, dampingFraction: 0.8)
    static let fanStagger: Double = 0.025
    static let deck: Animation = .spring(response: 0.3, dampingFraction: 0.85)
    static let fade: Animation = .easeInOut(duration: 0.12)

    /// Returns `fade` when reduced, else `normal`.
    static func pick(_ normal: Animation, reduced: Bool) -> Animation {
        reduced ? fade : normal
    }
}
