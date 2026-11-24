// Mouse and keyboard events into the iPhone Mirroring window, the same events
// the user's own mouse and keyboard make (CGEvent, public API).
//
// Mouse: posted at screen points that System.reach checked are inside the
// window and not covered by another app. The pointer goes back where the user
// left it afterwards.
// Keyboard: posted to the Mirroring app's process only, so a key can never
// land in another app even if focus moves.

import CoreGraphics
import Foundation
import MirrorCore

enum Input {
    static let source = CGEventSource(stateID: .hidSystemState)

    static func pause(_ ms: Int) {
        usleep(useconds_t(ms * 1000))
    }

    /// Run a pointer gesture, then put the pointer back where the user had it.
    static func keepingPointer(_ body: () -> Void) {
        let saved = CGEvent(source: nil)?.location
        body()
        if let saved {
            CGWarpMouseCursorPosition(saved)
            CGAssociateMouseAndMouseCursorPosition(1)
        }
    }

    static func mouse(_ type: CGEventType, _ p: CGPoint) {
        CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    }

    static func click(_ p: CGPoint) {
        keepingPointer {
            mouse(.mouseMoved, p)
            pause(40)
            mouse(.leftMouseDown, p)
            pause(60)
            mouse(.leftMouseUp, p)
            pause(30)
        }
    }

    static func drag(_ a: CGPoint, _ b: CGPoint, durationMs: Int) {
        keepingPointer {
            mouse(.mouseMoved, a)
            pause(40)
            mouse(.leftMouseDown, a)
            let steps = max(8, durationMs / 16)
            for i in 1...steps {
                let t = Double(i) / Double(steps)
                mouse(.leftMouseDragged, CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t))
                pause(max(1, durationMs / steps))
            }
            mouse(.leftMouseUp, b)
            pause(30)
        }
    }

    static func key(_ k: KeyStroke, pid: pid_t) {
        let flags = CGEventFlags(rawValue: k.flags)
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: k.keyCode, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: k.keyCode, keyDown: false)
        else { return }
        down.flags = flags
        up.flags = flags
        down.postToPid(pid)
        pause(25)
        up.postToPid(pid)
        pause(25)
    }

    /// Characters as Unicode key events; newline and tab as their keys.
    static func type(_ text: String, pid: pid_t) {
        for ch in text {
            if ch == "\n" || ch == "\r\n" {
                key(KeyStroke(keyCode: 36, flags: 0), pid: pid)
                continue
            }
            if ch == "\t" {
                key(KeyStroke(keyCode: 48, flags: 0), pid: pid)
                continue
            }
            let units = Array(String(ch).utf16)
            guard let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
                  let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false)
            else { continue }
            down.flags = []
            up.flags = []
            down.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
            up.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
            down.postToPid(pid)
            pause(8)
            up.postToPid(pid)
            pause(14)
        }
    }
}
