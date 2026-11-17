// The wire protocol: one JSON object per line on stdin, one per line on stdout.
//
// Request:  {"id":"7","cmd":"move","dx":40,"dy":-12}
// Reply:    {"id":"7","ok":true,...}  or  {"id":"7","ok":false,"error":"<code>","message":"<sentence>"}
// Event:    {"event":"state",...}       (no id; sent whenever the Bluetooth state changes)
//
// Commands:
//   version
//   state                                   Bluetooth power and permission, advertising, connected, last error.
//                                           Never asks for a permission.
//   pair                                    publish the keyboard and advertise as "Allternit Keyboard"
//                                           (asks for Bluetooth permission the first time)
//   unpair                                  stop advertising and remove the keyboard (the iPhone disconnects)
//   key     {"usage","modifiers"?,"down"}   USB HID usage page 0x07; modifiers 1 ctrl, 2 shift, 4 alt, 8 cmd
//   type    {"text"}                        up to 500 printable ASCII characters (US layout)
//   move    {"dx","dy"}                     relative, whole units, |d| ≤ 32767 (split into ±127 reports)
//   button  {"button"?,"down"?,"up"?}       left (default) | right | middle; down and up default true
//   scroll  {"lines"}                       -100..100, positive = down
//   reset                                   drive the pointer into the top-left corner
//   release                                 let go of every key and button
//
// The tool exits when stdin closes, letting go of every key and button and
// removing the keyboard first.
//
// Desktop side: surfaces/allternit-desktop/src/main/phone-bt-mac.ts (keep both in sync).

import BTHIDCore
import Foundation

public let protocolVersion = 1
public let toolVersion = "0.1.0"
public let maxTypeLength = 500
public let maxMove = 32_767

public enum Command: Equatable, Sendable {
    case version
    case state
    case pair
    case unpair
    case key(usage: Int, modifiers: Int, down: Bool)
    case type(text: String)
    case move(dx: Int, dy: Int)
    case button(MouseButton, down: Bool, up: Bool)
    case scroll(lines: Int)
    case reset
    case release

    /// Seconds before the command is abandoned. Every wait inside is bounded
    /// below this, so the limit is a last resort: past it the process answers
    /// `timeout` and exits, and Desktop starts a fresh one (and pairs again).
    public var timeLimit: Double {
        switch self {
        case .version, .state: return 2
        case .pair: return 20
        case .unpair, .key, .button, .scroll, .release: return 5
        case .type(let text): return 5 + Double(text.count) * 0.1
        case .move(let dx, let dy): return 5 + Double(HIDReports.moveSteps(dx: dx, dy: dy).count) * 0.05
        case .reset: return 20
        }
    }

    /// Commands that send input to the iPhone.
    public var isInput: Bool {
        switch self {
        case .version, .state, .pair, .unpair: return false
        default: return true
        }
    }
}

/// A request line → its id and command. Bad input is a `bad_request` error that still carries the id when one was readable.
public func parseRequest(_ line: String) -> (id: String?, result: Result<Command, BTHIDError>) {
    guard let data = line.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data),
          let dict = obj as? [String: Any]
    else {
        return (nil, .failure(BTHIDError("bad_request", "Each line must be one JSON object.")))
    }
    let id: String? = (dict["id"] as? String) ?? (dict["id"] as? NSNumber).map { $0.stringValue }
    return (id, parseCommand(dict))
}

public func parseCommand(_ d: [String: Any]) -> Result<Command, BTHIDError> {
    guard let cmd = d["cmd"] as? String else { return .failure(BTHIDError("bad_request", "cmd is required.")) }
    /// Whole numbers only; booleans (which decode as NSNumber too) and fractions are refused.
    func int(_ k: String) -> Int? {
        guard let n = d[k] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
        let v = n.doubleValue
        guard v.isFinite, v.rounded() == v, abs(v) <= 1e9 else { return nil }
        return Int(v)
    }
    func flag(_ k: String, _ dflt: Bool) -> Bool? {
        guard let v = d[k] else { return dflt }
        guard let n = v as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
        return n.boolValue
    }
    func bad(_ m: String) -> Result<Command, BTHIDError> { .failure(BTHIDError("bad_request", m)) }

    switch cmd {
    case "version": return .success(.version)
    case "state": return .success(.state)
    case "pair": return .success(.pair)
    case "unpair": return .success(.unpair)
    case "reset": return .success(.reset)
    case "release": return .success(.release)
    case "key":
        guard let usage = int("usage"), (0...Int(HIDReportMap.maxKeyUsage)).contains(usage) else {
            return bad("key needs usage 0–\(HIDReportMap.maxKeyUsage) (USB HID usage page 0x07).")
        }
        let mods = d["modifiers"] == nil ? 0 : int("modifiers")
        guard let mods, (0...0xFF).contains(mods) else { return bad("modifiers must be 0–255.") }
        guard let down = flag("down", true) else { return bad("down must be true or false.") }
        return .success(.key(usage: usage, modifiers: mods, down: down))
    case "type":
        guard let text = d["text"] as? String, !text.isEmpty else { return bad("type needs text.") }
        guard text.count <= maxTypeLength else { return bad("text is longer than \(maxTypeLength) characters.") }
        if let c = USLayout.untypable(text) {
            return bad("The keyboard can’t type “\(c)”: only printable ASCII on a US layout can be sent.")
        }
        return .success(.type(text: text))
    case "move":
        guard let dx = int("dx"), let dy = int("dy"), abs(dx) <= maxMove, abs(dy) <= maxMove else {
            return bad("move needs whole dx and dy within ±\(maxMove).")
        }
        return .success(.move(dx: dx, dy: dy))
    case "button":
        let name = d["button"] == nil ? "left" : d["button"] as? String
        guard let name, let b = MouseButton(rawValue: name) else { return bad("button must be left, right or middle.") }
        guard let down = flag("down", true), let up = flag("up", true) else { return bad("down and up must be true or false.") }
        guard down || up else { return bad("button needs down, up or both.") }
        return .success(.button(b, down: down, up: up))
    case "scroll":
        guard let lines = int("lines"), (-100...100).contains(lines) else { return bad("scroll needs whole lines from -100 to 100.") }
        return .success(.scroll(lines: lines))
    default:
        return .failure(BTHIDError("unknown_command", "Unknown command \(cmd)."))
    }
}

/// The `state` reply body (also the `event:"state"` line).
public func stateJSON(_ s: HIDPeripheralStatus, localName: String) -> [String: Any] {
    var out: [String: Any] = [
        "ok": true,
        "bluetooth": s.radio.rawValue,
        "authorization": s.authorization.rawValue,
        "services_ready": s.servicesReady,
        "extra_services": s.extraServices,
        "advertising": s.advertising,
        "connected": s.connected,
        "subscribers": s.subscribers,
        "refused": s.refused,
        "local_name": localName,
        // A Bluetooth peripheral is not told its host's name.
        "name": NSNull(),
        "central_id": s.centralID.map { $0 as Any } ?? NSNull(),
        "last_error": NSNull(),
    ]
    if let e = s.lastError { out["last_error"] = ["code": e.code, "message": e.message] }
    return out
}

/// One reply line (sorted keys, so output is stable for tests and logs).
public func encodeReply(id: String?, _ body: [String: Any]) -> String {
    var out = body
    if let id { out["id"] = id }
    guard let data = try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys, .withoutEscapingSlashes]),
          let s = String(data: data, encoding: .utf8)
    else { return "{\"ok\":false,\"error\":\"encode_failed\",\"message\":\"The reply could not be encoded.\"}" }
    return s
}

public func errorReply(id: String?, _ e: BTHIDError) -> String {
    encodeReply(id: id, ["ok": false, "error": e.code, "message": e.message])
}
