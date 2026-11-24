// Text to key strokes on the US English layout. The same table as
// phone-hid-keys.ts (Desktop) and HidReports.charStroke (Android).
//
// A keyboard sends key positions, not characters: the phone turns them into
// text with its own hardware-keyboard layout (Settings > General > Keyboard >
// Hardware Keyboard). Only printable ASCII can be sent, and it comes out right
// only on a US-compatible layout. Non-ASCII typing is UNVERIFIED (contract §3.2).

public struct KeyStroke: Equatable, Sendable {
    public let usage: UInt8
    public let modifiers: UInt8
    public init(usage: UInt8, modifiers: UInt8) {
        self.usage = usage
        self.modifiers = modifiers
    }
}

public enum Modifier {
    public static let ctrl: UInt8 = 1
    public static let shift: UInt8 = 2
    public static let alt: UInt8 = 4
    public static let cmd: UInt8 = 8
}

public enum USLayout {
    private static let symbols: [Character: (UInt8, Bool)] = [
        " ": (0x2C, false),
        "-": (0x2D, false), "_": (0x2D, true),
        "=": (0x2E, false), "+": (0x2E, true),
        "[": (0x2F, false), "{": (0x2F, true),
        "]": (0x30, false), "}": (0x30, true),
        "\\": (0x31, false), "|": (0x31, true),
        ";": (0x33, false), ":": (0x33, true),
        "'": (0x34, false), "\"": (0x34, true),
        "`": (0x35, false), "~": (0x35, true),
        ",": (0x36, false), "<": (0x36, true),
        ".": (0x37, false), ">": (0x37, true),
        "/": (0x38, false), "?": (0x38, true),
        "!": (0x1E, true), "@": (0x1F, true), "#": (0x20, true), "$": (0x21, true),
        "%": (0x22, true), "^": (0x23, true), "&": (0x24, true), "*": (0x25, true),
        "(": (0x26, true), ")": (0x27, true),
    ]

    /// One character → its stroke, or nil when the layout can't send it.
    public static func stroke(_ c: Character) -> KeyStroke? {
        guard let a = c.asciiValue else { return nil }
        switch a {
        case 0x61...0x7A: return KeyStroke(usage: 0x04 + (a - 0x61), modifiers: 0) // a-z
        case 0x41...0x5A: return KeyStroke(usage: 0x04 + (a - 0x41), modifiers: Modifier.shift) // A-Z
        case 0x31...0x39: return KeyStroke(usage: 0x1E + (a - 0x31), modifiers: 0) // 1-9
        case 0x30: return KeyStroke(usage: 0x27, modifiers: 0) // 0
        default:
            guard let s = symbols[c] else { return nil }
            return KeyStroke(usage: s.0, modifiers: s.1 ? Modifier.shift : 0)
        }
    }

    /// The first character the layout can't send, or nil when all of `text` can be typed.
    public static func untypable(_ text: String) -> Character? {
        text.first { stroke($0) == nil }
    }

    public static func strokes(_ text: String) -> [KeyStroke]? {
        var out: [KeyStroke] = []
        for c in text {
            guard let s = stroke(c) else { return nil }
            out.append(s)
        }
        return out
    }
}
