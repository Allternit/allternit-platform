// Report values and the input state that produces them. Pure: the
// CoreBluetooth wrapper only sends what this file returns.
//
// Contract (docs/contracts/CONTRACT-phone-use-2026-11-16.md §6, §9.2):
//   - modifier bits 1 ctrl, 2 shift, 4 alt, 8 cmd = the left-hand keys of the
//     HID modifier byte, so they go into the report unchanged
//   - pointer moves are relative, -127..127 per report; longer moves split
//   - scroll lines, positive = down (a HID wheel counts up as positive)
//   - corner reset = (-127, -127) moves adding up past the core's
//     CORNER_REACH (20 000 units, phone-hid-core.ts)

import Foundation

/// One report to send, by the characteristic it goes out on.
public enum HIDOutput: Equatable, Sendable {
    case keyboard(Data)
    case mouse(Data)
}

public enum HIDReports {
    public static let keyboardSize = 8
    public static let mouseSize = 4
    /// A mouse report carries one signed byte per axis.
    public static let maxStep = 127
    /// 158 × 127 = 20 066 > CORNER_REACH (phone-hid-core.ts), same as the Android sender.
    public static let cornerSteps = 158

    public static func clamp(_ v: Int) -> Int { min(max(v, -maxStep), maxStep) }

    /// [modifiers, reserved, key1…key6]. Keys past six are dropped.
    public static func keyboard(modifiers: UInt8, keys: [UInt8]) -> Data {
        var out = Data(count: keyboardSize)
        out[0] = modifiers
        for (i, k) in keys.prefix(HIDReportMap.maxKeys).enumerated() { out[2 + i] = k }
        return out
    }

    /// [buttons, dx, dy, wheel], each axis clamped to -127..127.
    public static func mouse(buttons: UInt8, dx: Int, dy: Int, wheel: Int) -> Data {
        Data([buttons & 0x07, UInt8(bitPattern: Int8(clamp(dx))), UInt8(bitPattern: Int8(clamp(dy))), UInt8(bitPattern: Int8(clamp(wheel)))])
    }

    /// A signed total as steps within ±127 that add up to it exactly. Zero is no steps.
    public static func steps(_ total: Int) -> [Int] {
        var out: [Int] = []
        var left = total
        while left != 0 {
            let s = clamp(left)
            out.append(s)
            left -= s
        }
        return out
    }

    /// One relative move as (dx, dy) pairs within ±127, spread evenly so the
    /// pointer travels a straight line; they add up exactly. Zero is no pairs.
    public static func moveSteps(dx: Int, dy: Int) -> [(Int, Int)] {
        let n = max(Int((Double(abs(dx)) / Double(maxStep)).rounded(.up)), Int((Double(abs(dy)) / Double(maxStep)).rounded(.up)))
        if n == 0 { return [] }
        var out: [(Int, Int)] = []
        var sx = 0
        var sy = 0
        for i in 1...n {
            let x = Int((Double(dx * i) / Double(n)).rounded())
            let y = Int((Double(dy * i) / Double(n)).rounded())
            out.append((x - sx, y - sy))
            sx = x
            sy = y
        }
        return out
    }

    /// Contract: scroll lines, positive = down; a HID wheel counts up as positive.
    public static func wheel(forLines lines: Int) -> Int { -lines }
}

public enum MouseButton: String, Sendable, CaseIterable {
    case left, right, middle

    public var bit: UInt8 {
        switch self {
        case .left: return 1
        case .right: return 2
        case .middle: return 4
        }
    }
}

public struct HIDInputError: Error, Equatable, Sendable {
    public let message: String
    public init(_ message: String) { self.message = message }
}

/**
 * What is held down right now, and the reports each action needs. Every
 * report carries the whole state (held buttons ride along on moves, so a
 * drag is press, moves, release).
 */
public struct HIDInputState: Equatable, Sendable {
    public private(set) var modifiers: UInt8 = 0
    public private(set) var keys: [UInt8] = []
    public private(set) var buttons: UInt8 = 0

    public init() {}

    private var keyboardReport: HIDOutput { .keyboard(HIDReports.keyboard(modifiers: modifiers, keys: keys)) }
    private func mouseReport(dx: Int = 0, dy: Int = 0, wheel: Int = 0) -> HIDOutput {
        .mouse(HIDReports.mouse(buttons: buttons, dx: dx, dy: dy, wheel: wheel))
    }

    /// Contract hid.key: one usage (page 0x07) down or up with modifiers.
    /// Usage 0 with down=false lets go of every key.
    public mutating func key(usage: Int, modifiers mods: Int, down: Bool) throws -> [HIDOutput] {
        guard usage >= 0, usage <= Int(HIDReportMap.maxKeyUsage) else {
            throw HIDInputError("usage must be 0–\(HIDReportMap.maxKeyUsage) (USB HID usage page 0x07).")
        }
        guard mods >= 0, mods <= 0xFF else { throw HIDInputError("modifiers must be 0–255.") }
        let u = UInt8(usage)
        if down {
            modifiers = UInt8(mods)
            if u != 0, !keys.contains(u) {
                if keys.count >= HIDReportMap.maxKeys { throw HIDInputError("Six keys are already held.") }
                keys.append(u)
            }
        } else {
            if u == 0 { keys.removeAll() } else { keys.removeAll { $0 == u } }
            if keys.isEmpty { modifiers = 0 }
        }
        return [keyboardReport]
    }

    /// Each character pressed and let go (so a doubled letter types twice);
    /// any key still held is let go too. Nil when the US layout can't send a character.
    public mutating func type(_ text: String) -> [HIDOutput]? {
        guard let strokes = USLayout.strokes(text) else { return nil }
        var out: [HIDOutput] = []
        for s in strokes {
            out.append(.keyboard(HIDReports.keyboard(modifiers: s.modifiers, keys: [s.usage])))
            out.append(.keyboard(HIDReports.keyboard(modifiers: 0, keys: [])))
        }
        modifiers = 0
        keys = []
        return out
    }

    /// Relative move, split into ±127 reports; buttons held stay held.
    public func move(dx: Int, dy: Int) -> [HIDOutput] {
        HIDReports.moveSteps(dx: dx, dy: dy).map { mouseReport(dx: $0.0, dy: $0.1) }
    }

    /// Contract hid.pointer.click: down and/or up (both by default = a click).
    public mutating func button(_ b: MouseButton, down: Bool, up: Bool) -> [HIDOutput] {
        var out: [HIDOutput] = []
        if down {
            buttons |= b.bit
            out.append(mouseReport())
        }
        if up {
            buttons &= ~b.bit
            out.append(mouseReport())
        }
        return out
    }

    /// Wheel lines, positive = down.
    public func scroll(lines: Int) -> [HIDOutput] {
        HIDReports.steps(HIDReports.wheel(forLines: lines)).map { mouseReport(wheel: $0) }
    }

    /// Drive the pointer into the top-left corner from anywhere.
    public func cornerReset() -> [HIDOutput] {
        Array(repeating: mouseReport(dx: -HIDReports.maxStep, dy: -HIDReports.maxStep), count: HIDReports.cornerSteps)
    }

    /// Let go of every key and button.
    public mutating func releaseAll() -> [HIDOutput] {
        modifiers = 0
        keys = []
        buttons = 0
        return [keyboardReport, mouseReport()]
    }
}
