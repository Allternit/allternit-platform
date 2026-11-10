import Foundation

/// The App Group the app shares with its widget, notification and share
/// extensions. Compiled into all four targets (project.yml), so the id
/// can't drift. `Shared/` is the screen extension's folder and stays
/// separate; it keeps its own copy (`AppPhoneLink.appGroup`).
enum AppGroup {
    static let identifier = "group.com.allternit.mobile"

    /// The group's shared defaults. Nil only when the target is missing the
    /// App Group entitlement.
    static var defaults: UserDefaults? {
        UserDefaults(suiteName: identifier)
    }
}
