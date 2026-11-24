import Foundation
import UIKit

/// Builds the S0 report (JSON + Markdown) from what the app and its extensions
/// logged in the app group. Helper numbers are merged on the Mac by
/// Scripts/compare.py, which applies the pass bar.
enum Report {
    static let targets = ["Settings", "Messages", "Safari", "Notes", "Store app"]

    /// Why taps are blocked on this route. Stays in the report until a public
    /// actuation path exists.
    static let tapBlocked = """
    No public API lets an app tap inside another app. The routes checked: \
    (1) HID event injection exists only in testmanagerd, which is XCTest (excluded by the brief); \
    (2) the developer accessibility service (com.apple.accessibility.axAuditDaemon.remoteserver) \
    has a press action, but pymobiledevice3 documents it as working only for processes that carry \
    the task_for_pid-allow entitlement (accessibilityaudit.py perform_press), which Settings, \
    Messages, Safari, Notes and store apps do not carry and Apple does not grant to third parties; \
    (3) a Bluetooth HID peripheral is not possible: iOS does not let apps advertise the HID service. \
    Not verified on a device yet.
    """

    @MainActor static func build(extra: [String: Any]) -> [String: Any] {
        var items: [[String: Any]] = []
        func item(_ id: String, _ status: String, _ value: Any?, _ unit: String, _ note: String, samples: [Double] = []) {
            var d: [String: Any] = ["id": id, "status": status, "unit": unit, "note": note, "samples": samples]
            d["value"] = value ?? NSNull()
            items.append(d)
        }

        // Tunnel + lockdown reach.
        if let q = extra["lockdown"] as? [String: Any] {
            let ms = (q["ms"] as? [Double]) ?? []
            let ok = (q["ok"] as? Int) ?? 0
            item("tunnel.lockdown_query", ok > 0 ? "ok" : "fail", Spike.median(ms), "ms",
                 "\(ok)/\(ms.count) QueryType round trips to lockdownd at \(Spike.deviceIP):62078. \((q["error"] as? String) ?? "")", samples: ms)
        } else {
            item("tunnel.lockdown_query", "not_run", nil, "ms", "Start the tunnel, then Probe lockdown.")
        }
        if let p = extra["pairing"] as? [String: Any] {
            item("pairing.start_session", (p["ok"] as? Bool) == true ? "ok" : "fail", p["ms"], "ms", (p["note"] as? String) ?? "")
        } else {
            item("pairing.start_session", "not_run", nil, "ms", "Import the phone's pair record, then Check pairing.")
        }

        // Screenshot (captured frame encode).
        let shots = Spike.readLines(Spike.shotsLog).compactMap { $0["ms"] as? Double }
        item("screen.screenshot", shots.isEmpty ? "needs_person" : "ok", Spike.median(shots), "ms",
             shots.isEmpty ? "Start screen capture (pick Display) in the app." : "\(shots.count) frames (\(ScreenCapture.shared.bgFrames) while another app was in front), JPEG encode at full size.",
             samples: Array(shots.suffix(50)))

        // Screen read per target app.
        let reads = Spike.readLines(Spike.readsLog)
        for app in targets {
            let ms = reads.filter { ($0["app"] as? String) == app }.compactMap { $0["ms"] as? Double }
            item("read.\(app.lowercased().replacingOccurrences(of: " ", with: "_"))", ms.isEmpty ? "needs_person" : "ok",
                 Spike.median(ms), "ms", ms.isEmpty ? "Run Measure read in \(app) with screen capture on." : "\(ms.count) reads (Vision OCR of the captured frame).",
                 samples: ms)
        }
        if let own = extra["ownRead"] as? [Double] {
            item("read.own_window", own.isEmpty ? "fail" : "ok", Spike.median(own), "ms", "Control: OCR of this app's own window.", samples: own)
        }

        // Taps.
        item("tap.landed", "blocked", nil, "of 20", tapBlocked)

        // Typing.
        if let t = Spike.readJSON(Spike.typeResult) {
            let exact = extra["selfTypedExact"] as? Bool
            let tail = (t["tailMatches"] as? Bool) == true
            let ok = exact ?? tail
            item("type.exact", ok ? "ok" : "fail", t["insertMs"], "ms",
                 "target \(t["target"] ?? "?"): \(t["chars"] ?? 0) chars; self-field exact=\(exact.map(String.init) ?? "n/a"); context tail match=\(tail) over \(t["contextChars"] ?? 0) chars")
        } else {
            item("type.exact", (extra["typeGuard"] as? String).map { $0 == "allow" ? "needs_person" : "blocked_by_guard" } ?? "not_run",
                 nil, "ms", "Guard: \((extra["typeGuard"] as? String) ?? "not asked"). Enable OnPhone Keys with Full Access.")
        }

        // Resilience: tunnel lifecycle and heartbeat gaps around the person's marks.
        let log = Spike.readLines(Spike.tunnelLog)
        let beats = log.filter { ($0["event"] as? String) == "beat" }.compactMap { $0["ts"] as? Double }
        var maxGap = 0.0
        for (a, b) in zip(beats, beats.dropFirst()) { maxGap = max(maxGap, b - a) }
        let events = log.filter { ($0["event"] as? String) != "beat" }
        let marks = Spike.readLines(Spike.marksLog)
        item("resilience.tunnel", events.isEmpty ? "not_run" : "observed", maxGap, "s max heartbeat gap",
             "\(beats.count) beats; events: " + events.map { "\($0["event"] ?? "?")\(($0["reasonName"] as? String).map { "(\($0))" } ?? "")" }.joined(separator: ", ")
             + "; marks: " + marks.map { "\($0["mark"] ?? "?")" }.joined(separator: ", "))

        // Battery soak.
        if let b = extra["battery"] as? [String: Any] {
            item("battery.30min", "observed", b["dropPct"], "% per 30 min", "start \(b["start"] ?? "?") end \(b["end"] ?? "?") over \(b["minutes"] ?? "?") min")
        } else {
            item("battery.30min", "not_run", nil, "%", "Start the soak with tunnel + screen capture on; end it after 30 min.")
        }

        let dev = UIDevice.current
        return [
            "spike": "S0 on-phone", "bundleId": Bundle.main.bundleIdentifier ?? "",
            "device": ["model": dev.model, "system": "\(dev.systemName) \(dev.systemVersion)", "name": dev.name],
            "writtenAt": ISO8601DateFormatter().string(from: Date()),
            "passBar": ["tapsLandedMin": 0.95, "medianReadMaxRatioToHelper": 1.5, "typingExact": true],
            "items": items,
        ]
    }

    static func markdown(_ r: [String: Any]) -> String {
        var s = "# S0 on-phone spike report\n\n"
        if let d = r["device"] as? [String: Any] { s += "Device: \(d["name"] ?? "") (\(d["system"] ?? ""))  \nWritten: \(r["writtenAt"] ?? "")\n\n" }
        s += "| Item | Status | Value | Unit | Note |\n|---|---|---|---|---|\n"
        for i in (r["items"] as? [[String: Any]]) ?? [] {
            var v = "-"
            if let x = i["value"] as? Double { v = String(format: "%.1f", x) } else if let x = i["value"], !(x is NSNull) { v = "\(x)" }
            let note = "\(i["note"] ?? "")".replacingOccurrences(of: "|", with: "/").replacingOccurrences(of: "\n", with: " ")
            s += "| \(i["id"] ?? "") | \(i["status"] ?? "") | \(v) | \(i["unit"] ?? "") | \(note) |\n"
        }
        s += "\nPass bar: 95% of taps land, median read at most 1.5x the helper, typing exact. Scripts/compare.py applies it against the helper baseline.\n"
        return s
    }

    @MainActor static func write(extra: [String: Any]) -> URL? {
        let r = build(extra: extra)
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        guard let data = try? JSONSerialization.data(withJSONObject: r, options: [.prettyPrinted, .sortedKeys]) else { return nil }
        let json = docs.appendingPathComponent("onphone-report.json")
        try? data.write(to: json, options: .atomic)
        try? markdown(r).write(to: docs.appendingPathComponent("onphone-report.md"), atomically: true, encoding: .utf8)
        return json
    }
}
