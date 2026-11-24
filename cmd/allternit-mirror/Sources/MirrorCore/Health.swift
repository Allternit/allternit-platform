// probe → one health state. The three MIRROR/MAC codes are contract §5 codes
// (docs/contracts/CONTRACT-phone-use-2026-11-16.md); the others are this
// tool's own and Desktop maps them (phone-mirror.ts).

import Foundation

public enum Health: String, Sendable {
    case healthy
    /// Contract §5.
    case macLocked = "MAC_LOCKED"
    case windowHidden = "MIRROR_WINDOW_HIDDEN"
    case phoneInUse = "MIRROR_PHONE_IN_USE"
    /// This Mac has no iPhone Mirroring app (older macOS, region, or removed).
    case notInstalled = "MIRROR_NOT_INSTALLED"
    /// The user must allow the app in System Settings; this tool never asks or grants.
    case needsScreenRecording = "NEEDS_SCREEN_RECORDING"
    case needsAccessibility = "NEEDS_ACCESSIBILITY"
    /// More than one Mirroring window is showing: we can't tell which one is the phone.
    case windowAmbiguous = "MIRROR_WINDOW_AMBIGUOUS"
}

public struct ProbeFacts: Equatable, Sendable {
    public var installed: Bool
    public var running: Bool
    /// Visible, normal-layer windows the Mirroring app owns.
    public var windowCount: Int
    public var macLocked: Bool
    public var onConsole: Bool
    public var screenRecording: Bool
    public var accessibility: Bool
    /// From reading the window; nil when not read.
    public var phoneInUse: Bool?

    public init(installed: Bool, running: Bool, windowCount: Int, macLocked: Bool, onConsole: Bool, screenRecording: Bool, accessibility: Bool, phoneInUse: Bool? = nil) {
        self.installed = installed
        self.running = running
        self.windowCount = windowCount
        self.macLocked = macLocked
        self.onConsole = onConsole
        self.screenRecording = screenRecording
        self.accessibility = accessibility
        self.phoneInUse = phoneInUse
    }
}

/// First match wins: nothing below a failed check can be trusted.
public func health(_ f: ProbeFacts) -> Health {
    if !f.installed { return .notInstalled }
    if f.macLocked || !f.onConsole { return .macLocked }
    if !f.screenRecording { return .needsScreenRecording }
    if !f.accessibility { return .needsAccessibility }
    if !f.running || f.windowCount == 0 { return .windowHidden }
    if f.windowCount > 1 { return .windowAmbiguous }
    if f.phoneInUse == true { return .phoneInUse }
    return .healthy
}

public struct SettingsPane: Equatable, Sendable {
    public let permission: String
    public let pane: String
    public let url: String
}

/// The System Settings pane the user opens for a missing permission. We only name it.
public func settingsPane(for h: Health) -> SettingsPane? {
    switch h {
    case .needsScreenRecording:
        return SettingsPane(
            permission: "screen_recording",
            pane: "System Settings > Privacy & Security > Screen & System Audio Recording",
            url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        )
    case .needsAccessibility:
        return SettingsPane(
            permission: "accessibility",
            pane: "System Settings > Privacy & Security > Accessibility",
            url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        )
    default:
        return nil
    }
}

/// True when the text read from the Mirroring window says the phone is in use
/// (unlocked in someone's hand). The exact wording is UNVERIFIED until plan check P1.
public func saysPhoneInUse(_ lines: [String]) -> Bool {
    let text = lines.joined(separator: "\n").lowercased()
    let patterns = ["iphone in use", "iphone is in use", "lock your iphone", "unlocked iphone"]
    return patterns.contains { text.contains($0) }
}

