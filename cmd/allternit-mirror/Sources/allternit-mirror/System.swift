// The Mac side: find the iPhone Mirroring app and its one window, the Mac's
// lock state and this process's permissions, and bring the window forward.
// Public APIs only (AppKit, CoreGraphics, ApplicationServices).

import AppKit
import ApplicationServices
import CoreGraphics
import MirrorCore

/// Apple's iPhone Mirroring app.
let mirrorBundleID = "com.apple.ScreenContinuity"

struct MirrorWindow {
    let id: CGWindowID
    let pid: pid_t
    let geometry: WindowGeometry
}

enum System {
    static func installed() -> Bool {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: mirrorBundleID) != nil
    }

    static func runningApp() -> NSRunningApplication? {
        NSRunningApplication.runningApplications(withBundleIdentifier: mirrorBundleID).first { !$0.isTerminated }
    }

    /// The app's visible, normal-layer windows of a real size (not menus, tooltips or the hover toolbar).
    static func windows(of pid: pid_t) -> [(id: CGWindowID, bounds: CGRect)] {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
            return []
        }
        return list.compactMap { info in
            guard (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
                  (info[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
                  ((info[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1) > 0.01,
                  let raw = info[kCGWindowBounds as String],
                  let rect = CGRect(dictionaryRepresentation: raw as! CFDictionary),
                  rect.width >= 100, rect.height >= 100,
                  let number = (info[kCGWindowNumber as String] as? NSNumber)?.uint32Value
            else { return nil }
            return (CGWindowID(number), rect)
        }
    }

    /// Pixels per point of the display under the window's centre.
    static func scale(for rect: CGRect) -> Double {
        var display: CGDirectDisplayID = 0
        var count: UInt32 = 0
        let centre = CGPoint(x: rect.midX, y: rect.midY)
        guard CGGetDisplaysWithPoint(centre, 1, &display, &count) == .success, count > 0,
              let mode = CGDisplayCopyDisplayMode(display), mode.width > 0
        else { return 2 }
        return Double(mode.pixelWidth) / Double(mode.width)
    }

    /// Screen lock and fast-user-switch state. Unknown counts as locked: never assume the Mac is usable.
    static func session() -> (locked: Bool, onConsole: Bool) {
        guard let d = CGSessionCopyCurrentDictionary() as? [String: Any] else { return (true, false) }
        let locked = (d["CGSSessionScreenIsLocked"] as? Bool) ?? false
        let onConsole = (d[kCGSessionOnConsoleKey as String] as? Bool) ?? false
        return (locked, onConsole)
    }

    /// Preflight only: these never show a prompt and never grant anything.
    static func permissions() -> (screenRecording: Bool, accessibility: Bool) {
        (CGPreflightScreenCaptureAccess(), AXIsProcessTrusted() && CGPreflightPostEventAccess())
    }

    static func needsYouError(_ h: Health, _ message: String) -> MirrorError {
        var extra = ["health": h.rawValue]
        if let pane = settingsPane(for: h) {
            extra["permission"] = pane.permission
            extra["pane"] = pane.pane
            extra["settings_url"] = pane.url
        }
        return MirrorError(h == .needsScreenRecording || h == .needsAccessibility ? "permission_missing" : "not_ready", message, extra: extra)
    }

    static func requireScreenRecording() throws {
        guard permissions().screenRecording else {
            throw needsYouError(.needsScreenRecording, "Allow Allternit in System Settings > Privacy & Security > Screen & System Audio Recording so it can see iPhone Mirroring.")
        }
    }

    static func requireAccessibility() throws {
        guard permissions().accessibility else {
            throw needsYouError(.needsAccessibility, "Allow Allternit in System Settings > Privacy & Security > Accessibility so it can click and type in iPhone Mirroring.")
        }
    }

    /// The Mac is usable and iPhone Mirroring shows exactly one window. Anything else is refused.
    static func theWindow() throws -> (NSRunningApplication, MirrorWindow) {
        guard installed() else {
            throw MirrorError("not_ready", "This Mac has no iPhone Mirroring app.", extra: ["health": Health.notInstalled.rawValue])
        }
        let s = session()
        if s.locked || !s.onConsole {
            throw needsYouError(.macLocked, "The Mac is locked.")
        }
        guard let app = runningApp() else {
            throw needsYouError(.windowHidden, "iPhone Mirroring is not open.")
        }
        let found = windows(of: app.processIdentifier)
        if found.isEmpty {
            throw needsYouError(.windowHidden, "iPhone Mirroring has no window showing.")
        }
        if found.count > 1 {
            throw needsYouError(.windowAmbiguous, "iPhone Mirroring shows \(found.count) windows, so the phone's window is not certain. Close the extra windows.")
        }
        let w = found[0]
        return (app, MirrorWindow(id: w.id, pid: app.processIdentifier, geometry: WindowGeometry(bounds: w.bounds, scale: scale(for: w.bounds))))
    }

    /// Windows of other apps in front of ours, front to back.
    static func windowsAbove(_ window: MirrorWindow) -> [OverlappingWindow] {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenAboveWindow], window.id) as? [[String: Any]] else { return [] }
        return list.compactMap { info in
            guard (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value != window.pid,
                  let raw = info[kCGWindowBounds as String],
                  let rect = CGRect(dictionaryRepresentation: raw as! CFDictionary)
            else { return nil }
            return OverlappingWindow(
                owner: (info[kCGWindowOwnerName as String] as? String) ?? "another app",
                layer: (info[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0,
                alpha: (info[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1,
                bounds: rect
            )
        }
    }

    /// Ask the app to come forward and raise its window (cooperative activation, then Accessibility).
    static func bringForward(_ app: NSRunningApplication) {
        app.activate(options: [.activateAllWindows])
        let element = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetAttributeValue(element, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
        var value: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, kAXWindowsAttribute as CFString, &value) == .success,
           let windows = value as? [AXUIElement] {
            for w in windows { AXUIElementPerformAction(w, kAXRaiseAction as CFString) }
        }
    }

    /// Bring the window forward and wait until no other app's window covers any of `pixels`.
    /// Returns the window as it is now (it may have moved) and the points in screen space.
    static func reach(_ pixels: [(Double, Double)]) async throws -> (MirrorWindow, [CGPoint]) {
        let (app, before) = try theWindow()
        // Validate against the window first: a point outside is refused before anything moves.
        for (x, y) in pixels { _ = try before.geometry.screenPoint(x: x, y: y) }
        bringForward(app)
        var cover: OverlappingWindow?
        for _ in 0..<20 {
            let (_, now) = try theWindow()
            guard now.geometry.pixelWidth == before.geometry.pixelWidth, now.geometry.pixelHeight == before.geometry.pixelHeight else {
                throw MirrorError("window_changed", "The iPhone Mirroring window changed size. Take a new screenshot first.")
            }
            let points = try pixels.map { try now.geometry.screenPoint(x: $0.0, y: $0.1) }
            let above = windowsAbove(now)
            cover = points.lazy.compactMap { coveringWindow(at: $0, above: above) }.first
            if cover == nil { return (now, points) }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw MirrorError("window_covered", "A window of \(cover?.owner ?? "another app") is over iPhone Mirroring there, so the click could land in the wrong app.")
    }
}
