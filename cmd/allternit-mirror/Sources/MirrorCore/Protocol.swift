// The wire protocol: one JSON object per line on stdin, one per line on stdout.
//
// Request:  {"id":"7","cmd":"tap","x":120,"y":640}
// Reply:    {"id":"7","ok":true,...}  or  {"id":"7","ok":false,"error":"<code>","message":"<sentence>"}
//
// Commands (coordinates are pixels of the last `capture` image, top-left origin):
//   version
//   probe      {"ocr"?:bool}                     app running, window, Mac lock, permissions, health
//   capture    {"png"?:bool,"gray"?:bool}        PNG of the Mirroring window only, plus its size
//   read       {}                                Vision text lines with [x1,y1,x2,y2] pixel boxes
//   tap        {"x","y"}
//   type       {"text"}                          max 500 characters
//   swipe      {"x1","y1","x2","y2","duration_ms"?} or {"direction":"up|down|left|right"}
//   key        {"combo"}                         "enter", "cmd+space", "home", "spotlight" ...
//   home | back | app_switcher | spotlight
//
// Desktop side: surfaces/allternit-desktop/src/main/phone-mirror.ts (keep both in sync).

import Foundation

public let protocolVersion = 1
public let toolVersion = "0.1.0"

public enum Direction: String, Sendable { case up, down, left, right }

public enum Command: Equatable, Sendable {
    case version
    case probe(ocr: Bool)
    case capture(png: Bool, gray: Bool)
    case read
    case tap(x: Double, y: Double)
    case type(text: String)
    case swipe(x1: Double, y1: Double, x2: Double, y2: Double, durationMs: Int)
    case swipeDirection(Direction, durationMs: Int)
    case key(combo: String)
    case home
    case back
    case appSwitcher
    case spotlight

    /// Seconds before the command is abandoned and the process exits (Desktop respawns it).
    public var timeLimit: Double {
        switch self {
        case .version: return 2
        case .probe(let ocr): return ocr ? 6 : 3
        case .capture: return 6
        case .read: return 10
        case .type(let text): return 5 + Double(text.count) * 0.03
        case .swipe(_, _, _, _, let d), .swipeDirection(_, let d): return 5 + Double(d) / 1000
        default: return 5
        }
    }

    /// Commands that send input to the window.
    public var isInput: Bool {
        switch self {
        case .version, .probe, .capture, .read: return false
        default: return true
        }
    }
}

public struct MirrorError: Error, Equatable, Sendable {
    public let code: String
    public let message: String
    public var extra: [String: String] = [:]
    public init(_ code: String, _ message: String, extra: [String: String] = [:]) {
        self.code = code
        self.message = message
        self.extra = extra
    }
}

public let maxTypeLength = 500

/// A request line → its id and command. Bad input is a `bad_request` error that still carries the id when one was readable.
public func parseRequest(_ line: String) -> (id: String?, result: Result<Command, MirrorError>) {
    guard let data = line.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data),
          let dict = obj as? [String: Any]
    else {
        return (nil, .failure(MirrorError("bad_request", "Each line must be one JSON object.")))
    }
    let id: String? = (dict["id"] as? String) ?? (dict["id"] as? NSNumber).map { $0.stringValue }
    return (id, parseCommand(dict))
}

public func parseCommand(_ d: [String: Any]) -> Result<Command, MirrorError> {
    guard let cmd = d["cmd"] as? String else { return .failure(MirrorError("bad_request", "cmd is required.")) }
    func num(_ k: String) -> Double? {
        // Booleans decode as NSNumber too; refuse them as coordinates.
        guard let n = d[k] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
        let v = n.doubleValue
        return v.isFinite ? v : nil
    }
    func flag(_ k: String, _ dflt: Bool) -> Bool { (d[k] as? Bool) ?? dflt }
    func duration() -> Result<Int, MirrorError> {
        guard d["duration_ms"] != nil else { return .success(300) }
        guard let v = num("duration_ms"), v >= 50, v <= 5000 else {
            return .failure(MirrorError("bad_request", "duration_ms must be 50–5000."))
        }
        return .success(Int(v))
    }
    switch cmd {
    case "version": return .success(.version)
    case "probe": return .success(.probe(ocr: flag("ocr", false)))
    case "capture": return .success(.capture(png: flag("png", true), gray: flag("gray", false)))
    case "read": return .success(.read)
    case "tap":
        guard let x = num("x"), let y = num("y") else { return .failure(MirrorError("bad_request", "tap needs numeric x and y.")) }
        return .success(.tap(x: x, y: y))
    case "type":
        guard let text = d["text"] as? String, !text.isEmpty else { return .failure(MirrorError("bad_request", "type needs text.")) }
        guard text.count <= maxTypeLength else { return .failure(MirrorError("bad_request", "text is longer than \(maxTypeLength) characters.")) }
        if text.unicodeScalars.contains(where: { $0.properties.generalCategory == .control && $0 != "\n" && $0 != "\t" }) {
            return .failure(MirrorError("bad_request", "text may not hold control characters other than newline and tab."))
        }
        return .success(.type(text: text))
    case "swipe":
        let dur: Int
        switch duration() {
        case .success(let v): dur = v
        case .failure(let e): return .failure(e)
        }
        if let raw = d["direction"] {
            guard let s = raw as? String, let dir = Direction(rawValue: s) else {
                return .failure(MirrorError("bad_request", "direction must be up, down, left or right."))
            }
            return .success(.swipeDirection(dir, durationMs: dur))
        }
        guard let x1 = num("x1"), let y1 = num("y1"), let x2 = num("x2"), let y2 = num("y2") else {
            return .failure(MirrorError("bad_request", "swipe needs x1, y1, x2, y2 or a direction."))
        }
        return .success(.swipe(x1: x1, y1: y1, x2: x2, y2: y2, durationMs: dur))
    case "key":
        guard let combo = d["combo"] as? String, !combo.isEmpty, combo.count <= 40 else {
            return .failure(MirrorError("bad_request", "key needs combo (up to 40 characters)."))
        }
        if case .failure(let e) = parseCombo(combo) { return .failure(e) }
        return .success(.key(combo: combo))
    case "home": return .success(.home)
    case "back": return .success(.back)
    case "app_switcher": return .success(.appSwitcher)
    case "spotlight": return .success(.spotlight)
    default: return .failure(MirrorError("unknown_command", "Unknown command \(cmd)."))
    }
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

public func errorReply(id: String?, _ e: MirrorError) -> String {
    var body: [String: Any] = ["ok": false, "error": e.code, "message": e.message]
    for (k, v) in e.extra { body[k] = v }
    return encodeReply(id: id, body)
}
