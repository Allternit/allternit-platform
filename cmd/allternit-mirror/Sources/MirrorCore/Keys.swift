// Key combos → macOS virtual key codes and modifier flags (ANSI layout).
// iPhone Mirroring passes a hardware-keyboard combo through to the phone,
// and has its own menu shortcuts for Home, the App Switcher and Spotlight
// (Cmd+1/2/3; UNVERIFIED until plan check P1).

import Foundation

public struct KeyStroke: Equatable, Sendable {
    public let keyCode: UInt16
    /// CGEventFlags raw value.
    public let flags: UInt64
    public init(keyCode: UInt16, flags: UInt64) {
        self.keyCode = keyCode
        self.flags = flags
    }
}

public enum Modifier {
    public static let shift: UInt64 = 0x0002_0000
    public static let control: UInt64 = 0x0004_0000
    public static let option: UInt64 = 0x0008_0000
    public static let command: UInt64 = 0x0010_0000
}

/// Mirroring's own menu shortcuts.
public let mirrorAliases: [String: String] = [
    "home": "cmd+1",
    "app_switcher": "cmd+2",
    "appswitcher": "cmd+2",
    "spotlight": "cmd+3",
]

let namedKeys: [String: UInt16] = [
    "enter": 36, "return": 36, "tab": 48, "space": 49, "backspace": 51, "delete": 51,
    "esc": 53, "escape": 53, "forwarddelete": 117, "pageup": 116, "pagedown": 121,
    "end": 119, "left": 123, "right": 124, "down": 125, "up": 126,
]

let charKeys: [Character: UInt16] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
    "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
    "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "`": 50,
]

let modifierNames: [String: UInt64] = [
    "cmd": Modifier.command, "command": Modifier.command, "meta": Modifier.command,
    "shift": Modifier.shift,
    "alt": Modifier.option, "opt": Modifier.option, "option": Modifier.option,
    "ctrl": Modifier.control, "control": Modifier.control,
]

/// "cmd+space" / "Enter" / "home" → one key stroke. Exactly one non-modifier key.
public func parseCombo(_ raw: String) -> Result<KeyStroke, MirrorError> {
    var combo = raw.trimmingCharacters(in: .whitespaces).lowercased()
    if let alias = mirrorAliases[combo] { combo = alias }
    let parts = combo.split(separator: "+", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
    var flags: UInt64 = 0
    var key: UInt16?
    for part in parts {
        if let m = modifierNames[part] {
            flags |= m
            continue
        }
        guard key == nil else { return .failure(MirrorError("bad_request", "\(raw) has more than one key.")) }
        if let k = namedKeys[part] {
            key = k
        } else if part.count == 1, let c = part.first, let k = charKeys[c] {
            key = k
        } else {
            return .failure(MirrorError("bad_request", "Unknown key \"\(part)\" in \(raw)."))
        }
    }
    guard let key else { return .failure(MirrorError("bad_request", "\(raw) has no key, only modifiers.")) }
    return .success(KeyStroke(keyCode: key, flags: flags))
}
