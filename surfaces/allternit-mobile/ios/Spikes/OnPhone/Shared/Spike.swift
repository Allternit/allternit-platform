import Foundation

/// Constants and app-group storage shared by the app and its three extensions.
/// Every file is JSON or JSON lines in the app group container.
enum Spike {
    static let appGroup = "group.com.allternit.mobile.onphone-spike"
    static let tunnelBundleID = "com.allternit.mobile.onphone-spike.tunnel"

    /// Loopback tunnel: the app connects to `deviceIP`; the tunnel swaps the IPv4
    /// source and destination so the packet comes back into this phone's own stack,
    /// where lockdownd listens on 62078 for every interface.
    static let deviceIP = "10.7.0.1"
    static let tunnelLocalIP = "10.7.0.0"
    static let lockdownPort: UInt16 = 62078

    // File names in the app group.
    static let tunnelLog = "tunnel-log.jsonl"      // start/stop/beat from the tunnel
    static let readsLog = "reads.jsonl"            // OCR timings from screen capture
    static let shotsLog = "shots.jsonl"            // frame encode timings from screen capture
    static let measureFile = "measure.json"        // {app, until} read window
    static let typeJob = "type-job.json"           // queued typing job (written only after the guard allows)
    static let typeResult = "type-result.json"     // keyboard's result
    static let marksLog = "marks.jsonl"            // person's marks: locked, restarted, other VPN
    static let pairRecord = "pair-record.plist"

    static var container: URL {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
            ?? FileManager.default.temporaryDirectory
    }

    static func url(_ name: String) -> URL { container.appendingPathComponent(name) }

    static var now: Double { Date().timeIntervalSince1970 }

    static func appendLine(_ name: String, _ object: [String: Any]) {
        var obj = object
        if obj["ts"] == nil { obj["ts"] = now }
        guard var data = try? JSONSerialization.data(withJSONObject: obj) else { return }
        data.append(0x0A)
        let u = url(name)
        if let h = try? FileHandle(forWritingTo: u) {
            defer { try? h.close() }
            _ = try? h.seekToEnd()
            try? h.write(contentsOf: data)
        } else {
            try? data.write(to: u, options: .atomic)
        }
    }

    static func readLines(_ name: String) -> [[String: Any]] {
        guard let text = try? String(contentsOf: url(name), encoding: .utf8) else { return [] }
        return text.split(separator: "\n").compactMap {
            (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any]
        }
    }

    static func writeJSON(_ name: String, _ object: Any) {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]) else { return }
        try? data.write(to: url(name), options: .atomic)
    }

    static func readJSON(_ name: String) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url(name)) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    static func remove(_ name: String) { try? FileManager.default.removeItem(at: url(name)) }

    static func median(_ xs: [Double]) -> Double? {
        guard !xs.isEmpty else { return nil }
        let s = xs.sorted()
        return s.count % 2 == 1 ? s[s.count / 2] : (s[s.count / 2 - 1] + s[s.count / 2]) / 2
    }

    /// The 200-character typing sample: letters, digits, punctuation, spaces.
    static let typingSample: String = {
        let base = "The quick brown fox jumps over the lazy dog 0123456789. Pack my box with five dozen liquor jugs! "
        var s = ""
        while s.count < 200 { s += base }
        return String(s.prefix(200))
    }()
}
